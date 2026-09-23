// Copyright (c) 2019 The Fuchsia Authors
//
// Permission to use, copy, modify, and/or distribute this software for any purpose with or without
// fee is hereby granted, provided that the above copyright notice and this permission notice
// appear in all copies.
//
// THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES WITH REGARD TO THIS
// SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE
// AUTHOR BE LIABLE FOR ANY SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
// WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION OF CONTRACT,
// NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE
// OF THIS SOFTWARE.

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/wlan_interface.h"

#include <zircon/errors.h>
#include <zircon/status.h>

#include <cstdio>
#include <cstring>

#include <bind/fuchsia/cpp/bind.h>

#include "fidl/fuchsia.wlan.fullmac/cpp/wire_types.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/cfg80211.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/common.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/debug.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/device.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/feature.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/fwil.h"
#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/linuxisms.h"

namespace wlan {
namespace brcmfmac {
namespace {

constexpr uint32_t kEthernetMtu = 1514;

}  // namespace

WlanInterface::WlanInterface(
    wlan::brcmfmac::Device* device,
    fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkDeviceIfc>&& netdev_ifc,
    uint8_t port_id, const char* name, uint16_t iface_id)
    : NetworkPort(std::move(netdev_ifc), *this, port_id), name_(name), iface_id_(iface_id) {}

void WlanInterface::Create(
    wlan::brcmfmac::Device* device, const char* name, wireless_dev* wdev,
    fuchsia_wlan_common::WlanMacRole role, uint16_t iface_id,
    fit::callback<void(zx::result<std::unique_ptr<WlanInterface>>)>&& on_complete) {
  std::unique_ptr<WlanInterface> interface(
      new WlanInterface(device, device->NetDev().NetDevIfcClient().Clone(),
                        ndev_to_if(wdev->netdev)->ifidx, name, iface_id));

  const zx_status_t status = [&] {
    interface->device_ = device;
    interface->wdev_ = wdev;
    interface->role_ = role;

    if (zx_status_t status = interface->AddWlanFullmacDevice(); status != ZX_OK) {
      BRCMF_ERR("Error while adding fullmac dev: %s", zx_status_get_string(status));
      return status;
    }
    NetworkPort::Role net_port_role;
    switch (role) {
      case fuchsia_wlan_common_wire::WlanMacRole::kClient:
        net_port_role = NetworkPort::Role::Client;
        break;
      case fuchsia_wlan_common_wire::WlanMacRole::kAp:
        net_port_role = NetworkPort::Role::Ap;
        break;
      default:
        BRCMF_ERR("Unsupported role %u", uint32_t(role));
        return ZX_ERR_INVALID_ARGS;
    }
    // Acquire a raw pointer since the smart pointer will be moved from.
    WlanInterface* interface_ptr = interface.get();
    interface_ptr->NetworkPort::Init(
        net_port_role, fdf::Dispatcher::GetCurrent()->get(),
        [interface = std::move(interface),
         on_complete = std::move(on_complete)](zx_status_t status) mutable {
          if (status != ZX_OK) {
            BRCMF_ERR("Failed to initialize port: %s", zx_status_get_string(status));
            on_complete(zx::error(status));
            return;
          }

          on_complete(zx::ok(std::move(interface)));
        });

    return ZX_OK;
  }();

  // Only call on_complete on failure. On success the NetworkPort::Init callback will call
  // on_complete asynchronously.
  if (status != ZX_OK) {
    on_complete(zx::error(status));
  }
}

zx_status_t WlanInterface::AddWlanFullmacDevice() {
  async_dispatcher_t* driver_dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
  auto wlanfullmacimpl =
      [this, driver_dispatcher](fidl::ServerEnd<fuchsia_wlan_fullmac::WlanFullmacImpl> server_end) {
        ServiceConnectHandler(driver_dispatcher, std::move(server_end));
      };

  // Add the service contains WlanFullmac protocol to outgoing directory.
  fuchsia_wlan_fullmac::Service::InstanceHandler wlanfullmac_service_handler(
      {.wlan_fullmac_impl = wlanfullmacimpl});

  auto status = device_->Outgoing()->AddService<fuchsia_wlan_fullmac::Service>(
      std::move(wlanfullmac_service_handler), GetName());
  if (status.is_error()) {
    BRCMF_ERR("Failed to add service to outgoing directory: %s", status.status_string());
    return status.status_value();
  }

  fidl::Arena arena;

  fidl::VectorView<fuchsia_driver_framework::wire::Offer> offers(arena, 1);
  offers[0] = fdf::MakeOffer2<fuchsia_wlan_fullmac::Service>(arena, GetName());
  auto property = fdf::MakeProperty2(arena, bind_fuchsia::SERVICE, "fuchsia.wlan.fullmac.Service");

  auto args =
      fdf::wire::NodeAddArgs::Builder(arena)
          .name(arena, GetName())
          .properties2(fidl::VectorView<fdf::wire::NodeProperty2>::FromExternal(&property, 1))
          .offers2(offers)
          .Build();

  auto endpoints = fidl::CreateEndpoints<fdf::NodeController>();
  if (endpoints.is_error()) {
    return endpoints.status_value();
  }

  wlanfullmac_controller_.Bind(std::move(endpoints->client),
                               fdf::Dispatcher::GetCurrent()->async_dispatcher(), this);
  // Add wlanfullmac child node for the node that this driver is binding to. Doing a sync version
  // here to reduce chaos.
  auto result =
      device_->GetParentNode().sync()->AddChild(std::move(args), std::move(endpoints->server), {});
  if (!result.ok()) {
    BRCMF_ERR("Add wlanfullmac node error due to FIDL error on protocol [Node]: %s",
              result.status_string());
    return result.status();
  }
  if (result->is_error()) {
    BRCMF_ERR("Add wlanfullmac node error: %u", static_cast<uint32_t>(result->error_value()));
    return ZX_ERR_INTERNAL;
  }
  return ZX_OK;
}

zx_status_t WlanInterface::RemoveWlanFullmacDevice() {
  if (!wlanfullmac_controller_.is_valid()) {
    BRCMF_ERR("Fullmac device for role %u cannot be removed because controller is invalid",
              static_cast<uint32_t>(Role()));
    return ZX_ERR_BAD_STATE;
  }
  auto result = wlanfullmac_controller_->Remove();
  if (!result.ok()) {
    BRCMF_ERR("Fullmac child remove failed for role %u, FIDL error: %s",
              static_cast<uint32_t>(Role()), result.status_string());
    return result.status();
  }
  wlanfullmac_controller_ = {};

  auto remove_result = device_->Outgoing()->RemoveService<fuchsia_wlan_fullmac::Service>(GetName());
  if (remove_result.is_error()) {
    BRCMF_ERR("Failed to remove wlanfullmac service from outgoing directory: %s.",
              remove_result.status_string());
    return remove_result.status_value();
  }
  return ZX_OK;
}

void WlanInterface::DestroyIface(fit::callback<void(zx_status_t)>&& on_complete) {
  // Use this to asynchronously call on_complete.
  auto call_on_complete = [on_complete = std::move(on_complete)](zx_status_t status) mutable {
    async::PostTask(
        fdf::Dispatcher::GetCurrent()->async_dispatcher(),
        [status, on_complete = std::move(on_complete)]() mutable { on_complete(status); });
  };

  {
    std::lock_guard lock(lock_);
    if (destroying_) {
      // Interface already destroyed or in the process of being destroyed, nothing to do.
      call_on_complete(ZX_ERR_NOT_FOUND);
      return;
    }
    destroying_ = true;
  }

  zx_status_t status = RemoveWlanFullmacDevice();
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to remove interface Fullmac Device: %s", zx_status_get_string(status));
    // If ZX_ERR_BAD_STATE is returned, we may have previously called RemoveWlanFullmacDevice
    // successfully but failed to delete the iface from firmware.
    // In that case, we don't return here to try deleting the iface from firmware again to avoid
    // having an iface in firmware that we can never delete.
    if (status != ZX_ERR_BAD_STATE) {
      call_on_complete(status);
      return;
    }
  }

  RemovePort([this, call_on_complete = std::move(call_on_complete)](zx_status_t status) mutable {
    // ZX_ERR_NOT_FOUND means the port was most likely already removed. This makes sense if the
    // request to destroy the interface came about as a result of the removal of the port or if
    // the netdevice child closed (thus calling Removed on the port) before interface
    // destruction was initiated. In that case continue with the interface removal.
    if (status != ZX_OK && status != ZX_ERR_NOT_FOUND) {
      BRCMF_ERR("Failed to remove port: %s", zx_status_get_string(status));
      call_on_complete(status);
      return;
    }
    wireless_dev* wdev = take_wdev();

    if (status = brcmf_cfg80211_del_iface(device_->drvr()->config, wdev); status != ZX_OK) {
      BRCMF_ERR("Failed to del iface, status: %s", zx_status_get_string(status));
      set_wdev(wdev);
      call_on_complete(status);
      return;
    }
    call_on_complete(ZX_OK);
  });
}

void WlanInterface::set_wdev(wireless_dev* wdev) {
  std::lock_guard<std::shared_mutex> guard(lock_);
  wdev_ = wdev;
}

wireless_dev* WlanInterface::take_wdev() {
  std::lock_guard<std::shared_mutex> guard(lock_);
  wireless_dev* wdev = wdev_;
  wdev_ = nullptr;
  return wdev;
}

void WlanInterface::ServiceConnectHandler(
    async_dispatcher_t* dispatcher,
    fidl::ServerEnd<fuchsia_wlan_fullmac::WlanFullmacImpl> server_end) {
  bindings_.AddBinding(dispatcher, std::move(server_end), this, [](fidl::UnbindInfo info) {
    if (!info.is_user_initiated()) {
      BRCMF_ERR("WlanFullmacImpl binding unexpectedly closed: %s", info.lossy_description());
    }
  });
}

zx_status_t WlanInterface::GetSupportedMacRoles(
    struct brcmf_pub* drvr,
    fuchsia_wlan_common::WlanMacRole
        out_supported_mac_roles_list[fuchsia_wlan_common::kMaxSupportedMacRoles],
    uint8_t* out_supported_mac_roles_count) {
  // The default client iface at bsscfgidx 0 is always assumed to exist by the driver.
  if (!drvr->iflist[0]) {
    BRCMF_ERR("drvr->iflist[0] is NULL. This should never happen.");
    return ZX_ERR_INTERNAL;
  }

  size_t len = 0;
  if (brcmf_feat_is_enabled(drvr, BRCMF_FEAT_STA)) {
    out_supported_mac_roles_list[len] = fuchsia_wlan_common::WlanMacRole::kClient;
    ++len;
  }
  if (brcmf_feat_is_enabled(drvr, BRCMF_FEAT_AP)) {
    out_supported_mac_roles_list[len] = fuchsia_wlan_common::WlanMacRole::kAp;
    ++len;
  }
  *out_supported_mac_roles_count = len;

  return ZX_OK;
}

zx_status_t WlanInterface::SetCountry(brcmf_pub* drvr, const std::array<uint8_t, 2>& country) {
  return brcmf_set_country(drvr, country);
}

zx_status_t WlanInterface::GetCountry(brcmf_pub* drvr, std::array<uint8_t, 2>* out_country) {
  return brcmf_get_country(drvr, out_country);
}

zx_status_t WlanInterface::ClearCountry(brcmf_pub* drvr) { return brcmf_clear_country(drvr); }

void WlanInterface::Init(InitRequestView request, InitCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ == nullptr) {
    BRCMF_ERR("Failed to initialize interface: wdev_ not found.");
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }

  if (!request->has_ifc()) {
    BRCMF_ERR("Failed to initialize interface: request missing ifc");
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  {
    std::lock_guard<std::shared_mutex> guard(wdev_->netdev->if_proto_lock);
    if (wdev_->netdev->if_proto.is_valid()) {
      BRCMF_ERR("Failed to initialize interface: if_proto already bound");
      completer.ReplyError(ZX_ERR_ALREADY_BOUND);
      return;
    }
    wdev_->netdev->if_proto =
        fidl::WireSyncClient<fuchsia_wlan_fullmac::WlanFullmacImplIfc>(std::move(request->ifc()));
  }

  zx::channel out_sme_channel;
  zx_status_t status = brcmf_if_start(wdev_->netdev, (zx_handle_t*)&out_sme_channel);
  if (status != ZX_OK) {
    BRCMF_ERR("Failed to start interface: %s", zx_status_get_string(status));
    completer.ReplyError(status);
    return;
  }

  fidl::Arena arena;
  auto response = fuchsia_wlan_fullmac::wire::WlanFullmacImplInitResponse::Builder(arena)
                      .sme_channel(std::move(out_sme_channel))
                      .Build();
  completer.ReplySuccess(response);
}

void WlanInterface::Query(QueryCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  fdf::Arena arena('WLAN');
  fuchsia_wlan_fullmac::wire::WlanFullmacImplQueryResponse info;
  if (wdev_ != nullptr) {
    fuchsia_wlan_fullmac::WlanFullmacImplQueryResponse natural_info{};
    brcmf_if_query(wdev_->netdev, natural_info);
    info = fidl::ToWire(arena, natural_info);
  }
  completer.ReplySuccess(info);
}

void WlanInterface::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_wlan_fullmac::WlanFullmacImpl> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  BRCMF_ERR("Unknown method in WlanFullmacImpl protocol: 0x%" PRIx64, metadata.method_ordinal);
}

void WlanInterface::QuerySecuritySupport(QuerySecuritySupportCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  fuchsia_wlan_common::SecuritySupport resp;
  if (wdev_ != nullptr) {
    brcmf_if_query_security_support(wdev_->netdev, &resp);
  }
  fidl::Arena arena;
  completer.ReplySuccess(
      fuchsia_wlan_fullmac::wire::WlanFullmacImplQuerySecuritySupportResponse::Builder(arena)
          .resp(fidl::ToWire(arena, resp))
          .Build());
}

void WlanInterface::QuerySpectrumManagementSupport(
    QuerySpectrumManagementSupportCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  fuchsia_wlan_common::SpectrumManagementSupport resp;
  if (wdev_ != nullptr) {
    brcmf_if_query_spectrum_management_support(wdev_->netdev, &resp);
  }
  fidl::Arena arena;
  completer.ReplySuccess(
      fuchsia_wlan_fullmac::wire::WlanFullmacImplQuerySpectrumManagementSupportResponse::Builder(
          arena)
          .resp(fidl::ToWire(arena, resp))
          .Build());
}

void WlanInterface::QueryTelemetrySupport(QueryTelemetrySupportCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  constexpr size_t kTelemetrySupportBufferSize =
      fidl::MaxSizeInChannel<fuchsia_wlan_stats::wire::TelemetrySupport,
                             fidl::MessageDirection::kSending>();
  fidl::Arena<kTelemetrySupportBufferSize> table_arena;
  fuchsia_wlan_stats::wire::TelemetrySupport resp{};
  if (wdev_ != nullptr) {
    brcmf_if_query_telemetry_support(wdev_->netdev, &resp, table_arena);
  }
  completer.ReplySuccess(
      fuchsia_wlan_fullmac::wire::WlanFullmacImplQueryTelemetrySupportResponse::Builder(table_arena)
          .resp(resp)
          .Build());
}

void WlanInterface::StartScan(StartScanRequestView request, StartScanCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_start_scan(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::StartScheduledScan(StartScheduledScanRequestView request,
                                       StartScheduledScanCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanInterface::StopScheduledScan(StopScheduledScanRequestView request,
                                      StopScheduledScanCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanInterface::Connect(ConnectRequestView request, ConnectCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_connect_req(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::Roam(RoamRequestView request, RoamCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_roam_req(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::Reconnect(ReconnectRequestView request, ReconnectCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_reconnect_req(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::AuthResp(AuthRespRequestView request, AuthRespCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_auth_resp(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::Deauth(DeauthRequestView request, DeauthCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_deauth_req(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::AssocResp(AssocRespRequestView request, AssocRespCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_assoc_resp(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::Disassoc(DisassocRequestView request, DisassocCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_disassoc_req(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::StartBss(StartBssRequestView request, StartBssCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_start_req(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::StopBss(StopBssRequestView request, StopBssCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_stop_req(wdev_->netdev, request);
  }
  completer.Reply();
}

void WlanInterface::SetKeys(SetKeysRequestView request, SetKeysCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  std::vector<zx_status_t> statuslist;
  if (wdev_ != nullptr) {
    statuslist = brcmf_if_set_keys_req(wdev_->netdev, request);
  }

  fidl::Arena arena;

  fuchsia_wlan_fullmac_wire::WlanFullmacSetKeysResp resp;
  resp.statuslist = fidl::VectorView(arena, statuslist);
  completer.Reply(resp);
}

void WlanInterface::EapolTx(EapolTxRequestView request, EapolTxCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_eapol_req(wdev_->netdev, request);
  }
  completer.Reply();
}

// Max size of IfaceHistogramStats.
constexpr size_t kIfaceStatsBufferSize =
    fidl::MaxSizeInChannel<fuchsia_wlan_stats::wire::IfaceStats,
                           fidl::MessageDirection::kSending>();

void WlanInterface::GetIfaceStats(GetIfaceStatsCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  fidl::Arena<kIfaceStatsBufferSize> table_arena;
  fuchsia_wlan_stats::wire::IfaceStats out_stats;
  if (wdev_ == nullptr) {
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }
  zx_status_t status = brcmf_if_get_iface_stats(wdev_->netdev, &out_stats, table_arena);
  if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess(out_stats);
  }
}

// Max size of IfaceHistogramStats.
constexpr size_t kIfaceHistogramStatsBufferSize =
    fidl::MaxSizeInChannel<fuchsia_wlan_stats::wire::IfaceHistogramStats,
                           fidl::MessageDirection::kSending>();

void WlanInterface::GetIfaceHistogramStats(GetIfaceHistogramStatsCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  fidl::Arena<kIfaceHistogramStatsBufferSize> table_arena;
  fuchsia_wlan_stats::wire::IfaceHistogramStats out_stats;
  if (wdev_ == nullptr) {
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }
  zx_status_t status = brcmf_if_get_iface_histogram_stats(wdev_->netdev, &out_stats, table_arena);
  if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess(out_stats);
  }
}

// Max size of SignalReport.
constexpr size_t kSignalReportBufferSize =
    fidl::MaxSizeInChannel<fuchsia_wlan_stats::wire::SignalReport,
                           fidl::MessageDirection::kSending>();

void WlanInterface::GetSignalReport(GetSignalReportCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  fidl::Arena<kSignalReportBufferSize> table_arena;
  fuchsia_wlan_stats::wire::SignalReport out_signal_report;
  if (wdev_ == nullptr) {
    completer.ReplyError(ZX_ERR_BAD_STATE);
    return;
  }
  zx_status_t status = brcmf_if_get_signal_report(wdev_->netdev, &out_signal_report, table_arena);
  if (status != ZX_OK) {
    completer.ReplyError(status);
  } else {
    completer.ReplySuccess(out_signal_report);
  }
}

void WlanInterface::SaeHandshakeResp(SaeHandshakeRespRequestView request,
                                     SaeHandshakeRespCompleter::Sync& completer) {
  brcmf_if_sae_handshake_resp(wdev_->netdev, request);
  completer.Reply();
}

void WlanInterface::SaeFrameTx(SaeFrameTxRequestView request,
                               SaeFrameTxCompleter::Sync& completer) {
  const fuchsia_wlan_fullmac::wire::SaeFrame frame = request->frame;
  brcmf_if_sae_frame_tx(wdev_->netdev, &frame);
  completer.Reply();
}

void WlanInterface::WmmStatusReq(WmmStatusReqCompleter::Sync& completer) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ != nullptr) {
    brcmf_if_wmm_status_req(wdev_->netdev);
  }
  completer.Reply();
}

void WlanInterface::OnLinkStateChanged(OnLinkStateChangedRequestView request,
                                       OnLinkStateChangedCompleter::Sync& completer) {
  {
    std::shared_lock<std::shared_mutex> guard(lock_);
    if (!request->has_online()) {
      BRCMF_ERR("Reset req does not contain required field, online: %d", request->has_online());
      completer.Reply();
      return;
    }

    bool online = request->online();
    SetPortOnline(online);
  }
  completer.Reply();
}

void WlanInterface::SetMacAddress(SetMacAddressRequestView request,
                                  SetMacAddressCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanInterface::QueryApfPacketFilterSupport(
    QueryApfPacketFilterSupportCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanInterface::InstallApfPacketFilter(InstallApfPacketFilterRequestView request,
                                           InstallApfPacketFilterCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanInterface::ReadApfPacketFilterData(ReadApfPacketFilterDataCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanInterface::SetApfPacketFilterEnabled(SetApfPacketFilterEnabledRequestView request,
                                              SetApfPacketFilterEnabledCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanInterface::GetApfPacketFilterEnabled(GetApfPacketFilterEnabledCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanInterface::GetScheduledScanEnabled(GetScheduledScanEnabledCompleter::Sync& completer) {
  completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
}

uint32_t WlanInterface::PortGetMtu() { return kEthernetMtu; }

void WlanInterface::MacGetAddress(fuchsia_net::MacAddress* out_mac) {
  std::shared_lock<std::shared_mutex> guard(lock_);
  if (wdev_ == nullptr) {
    BRCMF_WARN("Interface not available, returning empty MAC address");
    out_mac->octets().fill(0);
    return;
  }
  memcpy(out_mac->octets().data(), ndev_to_if(wdev_->netdev)->mac_addr, out_mac->octets().size());
}

void WlanInterface::MacGetFeatures(fuchsia_hardware_network_driver::Features* out_features) {
  out_features->multicast_filter_count() = 0;
  out_features->supported_modes() =
      fuchsia_hardware_network_driver::wire::SupportedMacFilterMode::kMulticastFilter |
      fuchsia_hardware_network_driver::wire::SupportedMacFilterMode::kMulticastPromiscuous;
}

void WlanInterface::MacSetMode(fuchsia_hardware_network::wire::MacFilterMode mode,
                               cpp20::span<const ::fuchsia_net::wire::MacAddress> multicast_macs) {
  zx_status_t status = ZX_OK;
  std::shared_lock<std::shared_mutex> guard(lock_);
  switch (mode) {
    case fuchsia_hardware_network::wire::MacFilterMode::kMulticastFilter:
      status = brcmf_if_set_multicast_promisc(wdev_->netdev, false);
      break;
    case fuchsia_hardware_network::wire::MacFilterMode::kMulticastPromiscuous:
      status = brcmf_if_set_multicast_promisc(wdev_->netdev, true);
      break;
    default:
      BRCMF_ERR("Unsupported MAC mode: %u", static_cast<uint32_t>(mode));
      break;
  }

  if (status != ZX_OK) {
    BRCMF_ERR("MacSetMode failed: %s", zx_status_get_string(status));
  }
}

void WlanInterface::PortRemoved() {
  // The network device port was unexpectedly removed. Destroy the interface to signal that it can
  // no longer be used.
  device_->DestroyIface(iface_id_, [](zx_status_t status) {});
}

}  // namespace brcmfmac
}  // namespace wlan
