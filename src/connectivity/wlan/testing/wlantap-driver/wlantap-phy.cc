// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "wlantap-phy.h"

#include <fidl/fuchsia.wlan.common/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fidl/cpp/wire/channel.h>
#include <lib/fidl_driver/cpp/wire_messaging_declarations.h>
#include <zircon/types.h>

#include <wlan/drivers/log.h>

#include "wlantap-phy-impl.h"

namespace wlan {

namespace {

fuchsia_wlan_tap::SetKeyArgs ToSetKeyArgs(
    const fuchsia_wlan_softmac::WlanKeyConfiguration& config) {
  WLAN_TRACE_DURATION();
  ZX_ASSERT(config.protection().has_value() && config.cipher_oui().has_value() &&
            config.cipher_type().has_value() && config.key_type().has_value() &&
            config.peer_addr().has_value() && config.key_idx().has_value() &&
            config.key().has_value());

  auto set_key_args = fuchsia_wlan_tap::SetKeyArgs{{
      .config = fuchsia_wlan_tap::WlanKeyConfig{{
          .protection = static_cast<uint8_t>(config.protection().value()),
          .cipher_oui = config.cipher_oui().value(),
          .cipher_type = config.cipher_type().value(),
          .key_type = static_cast<uint8_t>(config.key_type().value()),
          .peer_addr = config.peer_addr().value(),
          .key_idx = config.key_idx().value(),
          .key = config.key().value(),
      }},
  }};
  return set_key_args;
}

fuchsia_wlan_tap::TxArgs ToTxArgs(const fuchsia_wlan_softmac::WlanTxPacket pkt) {
  WLAN_TRACE_DURATION();
  if (pkt.info().phy() < fuchsia_wlan_ieee80211::WlanPhyType::kDsss ||
      pkt.info().phy() > fuchsia_wlan_ieee80211::WlanPhyType::kHe) {
    ZX_PANIC("Unknown PHY in wlan_tx_packet_t: %u.", static_cast<uint32_t>(pkt.info().phy()));
  }

  auto cbw = static_cast<uint32_t>(pkt.info().bandwidth());
  fuchsia_wlan_tap::WlanTxInfo tap_info = {{
      .tx_flags = pkt.info().tx_flags(),
      .valid_fields = pkt.info().valid_fields(),
      .tx_vector_idx = pkt.info().tx_vector_idx(),
      .phy = pkt.info().phy(),
      .cbw = static_cast<uint8_t>(cbw),
      .mcs = pkt.info().mcs(),
  }};
  auto tx_args = fuchsia_wlan_tap::TxArgs{{
      .packet = fuchsia_wlan_tap::WlanTxPacket{{.data = pkt.mac_frame(), .info = tap_info}},
  }};

  return tx_args;
}

}  // namespace

WlantapPhy::WlantapPhy(
    zx::channel user_channel, const fuchsia_wlan_tap::WlantapPhyConfig& phy_config,
    std::function<zx::result<>(WlantapPhy::ShutdownCompleter::Async)> phy_impl_shutdown_callback)
    : phy_config_(phy_config),
      name_("wlantap-phy:" + std::string(phy_config_.name())),
      user_binding_{fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                    fidl::ServerEnd<fuchsia_wlan_tap::WlantapPhy>(std::move(user_channel)), this,
                    [](fidl::UnbindInfo) {}},
      phy_impl_shutdown_callback_(std::move(phy_impl_shutdown_callback)) {
  WLAN_TRACE_DURATION();
}

zx_status_t WlantapPhy::SetCountry(fuchsia_wlan_tap::SetCountryArgs args) {
  WLAN_TRACE_DURATION();
  auto status = fidl::SendEvent(user_binding_)->SetCountry(args);
  if (status.is_error()) {
    fdf::error("{}: SetCountry() failed: user_binding not bound",
               status.error_value().status_string());

    return status.error_value().status();
  }
  return ZX_OK;
}

// fuchsia_wlan_tap::WlantapPhy impl

// Passes the |completer| to the |phy_impl_shutdown_callback_|
// initialized during construction of WlantapPhy. WlantapPhy expects
// |completer.Reply()| to called when the phy node and any iface node
// no longer exists.
void WlantapPhy::Shutdown(ShutdownCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: Shutdown", name_);
  fdf::info("{}: PHY device shutdown initiated.", name_);

  auto phy_impl_shutdown_status = phy_impl_shutdown_callback_(completer.ToAsync());
  if (phy_impl_shutdown_status.is_error()) {
    fdf::error("{}: Failed to shutdown the PHY: {}", name_,
               zx_status_get_string(phy_impl_shutdown_status.error_value()));
  }
}

void WlantapPhy::Rx(RxRequest& request, RxCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: Rx({} bytes)", name_, request.data().size());
  if (!wlan_softmac_ifc_client_.is_valid()) {
    fdf::error("{}: No WlantapMac present.", name_);
    return;
  }
  auto rx_flags =
      fuchsia_wlan_softmac::WlanRxInfoFlags::TruncatingUnknown(request.info().rx_flags());
  auto valid_fields =
      fuchsia_wlan_softmac::WlanRxInfoValid::TruncatingUnknown(request.info().valid_fields());
  fuchsia_wlan_softmac::WlanRxInfo converted_info{{
      .rx_flags = rx_flags,
      .valid_fields = valid_fields,
      .phy = request.info().phy(),
      .data_rate = request.info().data_rate(),
      .primary = request.info().primary(),
      .bandwidth = request.info().bandwidth(),
      .vht_secondary_80_channel = request.info().vht_secondary_80_channel(),
      .mcs = request.info().mcs(),
      .rssi_dbm = request.info().rssi_dbm(),
      .snr_dbh = request.info().snr_dbh(),
  }};

  fuchsia_wlan_softmac::WlanRxPacket rx_packet{
      {.mac_frame = request.data(), .info = converted_info}};
  wlan_softmac_ifc_client_->Recv(rx_packet).ThenExactlyOnce(
      [completer = completer.ToAsync(),
       this](fdf::Result<::fuchsia_wlan_softmac::WlanSoftmacIfc::Recv>& result) {
        fdf::info("{}: Recv completed", name_);
      });
  fdf::debug("{}: Rx done", name_);
}

void WlantapPhy::ReportTxResult(ReportTxResultRequest& request,
                                ReportTxResultCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  if (!phy_config_.quiet() || report_tx_status_count_ < 32) {
    fdf::info("{}: ReportTxResult {}", name_, report_tx_status_count_);
  }

  if (!wlan_softmac_ifc_client_.is_valid()) {
    fdf::error("{}: WlantapMacStart() not called.", name_);
    return;
  }

  ++report_tx_status_count_;

  wlan_softmac_ifc_client_->ReportTxResult(request.txr())
      .ThenExactlyOnce(
          [this, current_count = report_tx_status_count_](
              fdf::Result<::fuchsia_wlan_softmac::WlanSoftmacIfc::ReportTxResult>& result) {
            if (result.is_error()) {
              fdf::error("{}: Failed to report tx status up", name_);
              return;
            }

            fdf::debug("{}: ScanComplete done", name_);
            if (!phy_config_.quiet() || current_count <= 32) {
              fdf::debug("{}: ReportTxResult {} done", name_, current_count);
            }
          });
}

void WlantapPhy::ScanComplete(ScanCompleteRequest& request,
                              ScanCompleteCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: ScanComplete({})", name_, request.status());
  if (!wlan_softmac_ifc_client_.is_valid()) {
    fdf::error("{}: WlantapMacStart() not called.", name_);
    return;
  }

  fidl::Arena fidl_arena;

  fuchsia_wlan_softmac::WlanSoftmacIfcBaseNotifyScanCompleteRequest scan_complete_req{{
      .status = request.status(),
      .scan_id = request.scan_id(),
  }};

  wlan_softmac_ifc_client_->NotifyScanComplete(scan_complete_req)
      .ThenExactlyOnce(
          [this](fdf::Result<::fuchsia_wlan_softmac::WlanSoftmacIfc::NotifyScanComplete>& result) {
            if (result.is_error()) {
              fdf::error("{}: Failed to send scan complete notification up. Status: {}", name_,
                         zx_status_get_string(result.error_value().status()));
            } else {
              fdf::info("{}: ScanComplete done", name_);
            }
          });
}

// WlantapMac::Listener impl

void WlantapPhy::WlantapMacStart(fdf::ClientEnd<fuchsia_wlan_softmac::WlanSoftmacIfc> ifc_client) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: WlantapMacStart", name_);
  wlan_softmac_ifc_client_.Bind(std::move(ifc_client), fdf::Dispatcher::GetCurrent()->get());

  auto status = fidl::SendEvent(user_binding_)->WlanSoftmacStart();
  if (status.is_error()) {
    fdf::error("{}: WlanSoftmacStart() failed", status.error_value().status_string());
    return;
  }

  fdf::info("{}: WlantapMacStart done", name_);
}

void WlantapPhy::WlantapMacStop() {
  WLAN_TRACE_DURATION();
  fdf::info("{}: WlantapMacStop", name_);
}

void WlantapPhy::WlantapMacQueueTx(const fuchsia_wlan_softmac::WlanTxPacket& pkt) {
  WLAN_TRACE_DURATION();
  size_t pkt_size = pkt.mac_frame().size();
  if (!phy_config_.quiet() || report_tx_status_count_ < 32) {
    fdf::info("{}: WlantapMacQueueTx, size={}, tx_report_count={}", name_, pkt_size,
              report_tx_status_count_);
  }

  auto status = fidl::SendEvent(user_binding_)->Tx(ToTxArgs(pkt));
  if (status.is_error()) {
    fdf::error("{}: Tx() failed", status.error_value().status_string());
    return;
  }
  if (!phy_config_.quiet() || report_tx_status_count_ < 32) {
    fdf::debug("{}: WlantapMacQueueTx done({} bytes), tx_report_count={}", name_, pkt_size,
               report_tx_status_count_);
  }
}

void WlantapPhy::WlantapMacSetChannel(const fuchsia_wlan_ieee80211::ChannelNumber& channel,
                                      fuchsia_wlan_ieee80211::ChannelBandwidth cbw,
                                      const fuchsia_wlan_ieee80211::ChannelNumber& secondary80) {
  WLAN_TRACE_DURATION();
  if (!phy_config_.quiet()) {
    fdf::info("{}: WlantapMacSetChannel channel={}", name_, channel.number());
  }

  auto status =
      fidl::SendEvent(user_binding_)
          ->SetChannel(fuchsia_wlan_tap::WlantapPhySetChannelRequest{
              fuchsia_wlan_tap::SetChannelArgs{{.primary = channel,
                                                .bandwidth = cbw,
                                                .vht_secondary_80_channel = secondary80}}});
  if (status.is_error()) {
    fdf::error("{}: SetChannel() failed", status.error_value().status_string());
    return;
  }

  if (!phy_config_.quiet()) {
    fdf::debug("{}: WlantapMacSetChannel done", name_);
  }
}

void WlantapPhy::WlantapMacJoinBss(const fuchsia_wlan_driver::JoinBssRequest& config) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: WlantapMacJoinBss", name_);

  auto status = fidl::SendEvent(user_binding_)
                    ->JoinBss(fuchsia_wlan_tap::WlantapPhyJoinBssRequest{
                        fuchsia_wlan_tap::JoinBssArgs{{.config = config}}});
  if (status.is_error()) {
    fdf::error("{}: JoinBss() failed", status.error_value().status_string());
    return;
  }

  fdf::debug("{}: WlantapMacJoinBss done", name_);
}

void WlantapPhy::WlantapMacStartScan(const uint64_t scan_id) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: WlantapMacStartScan", name_);

  auto status = fidl::SendEvent(user_binding_)
                    ->StartScan(fuchsia_wlan_tap::WlantapPhyStartScanRequest(
                        fuchsia_wlan_tap::StartScanArgs{{.scan_id = scan_id}}));
  if (status.is_error()) {
    fdf::error("{}: StartScan() failed", status.error_value().status_string());
    return;
  }
  fdf::info("{}: WlantapMacStartScan done", name_);
}

void WlantapPhy::WlantapMacSetKey(const fuchsia_wlan_softmac::WlanKeyConfiguration& key_config) {
  WLAN_TRACE_DURATION();
  fdf::info("{}: WlantapMacSetKey", name_);

  auto status = fidl::SendEvent(user_binding_)->SetKey(ToSetKeyArgs(key_config));
  if (status.is_error()) {
    fdf::error("{}: SetKey() failed", status.error_value().status_string());
    return;
  }

  fdf::debug("{}: WlantapMacSetKey done", name_);
}

}  // namespace wlan
