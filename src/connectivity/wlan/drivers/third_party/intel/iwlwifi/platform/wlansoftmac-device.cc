// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "third_party/iwlwifi/platform/wlansoftmac-device.h"

#include <zircon/assert.h>
#include <zircon/status.h>

#include <memory>

#include "third_party/iwlwifi/platform/kernel.h"

extern "C" {
#include "third_party/iwlwifi/mvm/mvm.h"
}  // extern "C"

#include "third_party/iwlwifi/platform/ieee80211.h"
#include "third_party/iwlwifi/platform/mvm-mlme.h"
#include "third_party/iwlwifi/platform/mvm-sta.h"
#include "third_party/iwlwifi/platform/scoped_utils.h"
#include "third_party/iwlwifi/platform/stats.h"

#define CHECK_DELETE_IN_PROGRESS_WITHOUT_ERRSYNTAX(mvmvif, resp)        \
  do {                                                                  \
    if (mvmvif->delete_in_progress) {                                   \
      IWL_WARN(mvmvif, "Interface is in the process of being deleted"); \
      completer.buffer(arena).Reply(resp);                              \
      return;                                                           \
    }                                                                   \
  } while (0)

#define CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif)                 \
  do {                                                                  \
    if (mvmvif->delete_in_progress) {                                   \
      IWL_WARN(mvmvif, "Interface is in the process of being deleted"); \
      completer.buffer(arena).ReplyError(ZX_ERR_BAD_STATE);             \
      return;                                                           \
    }                                                                   \
  } while (0)

namespace wlan::iwlwifi {

WlanSoftmacDevice::WlanSoftmacDevice(iwl_trans* drvdata, uint16_t iface_id,
                                     struct iwl_mvm_vif* mvmvif)
    : mvmvif_(mvmvif), drvdata_(drvdata) {
  vif_.type = NL80211_IFTYPE_STATION;
  vif_.drv_priv = mvmvif;
  mac_init(mvmvif_, drvdata_, iface_id);
}

WlanSoftmacDevice::~WlanSoftmacDevice() { mac_release(mvmvif_); }

// Max size of WlanSoftmacQueryResponse.
constexpr size_t kWlanSoftmacQueryResponseBufferSize =
    fidl::MaxSizeInChannel<fuchsia_wlan_softmac::wire::WlanSoftmacQueryResponse,
                           fidl::MessageDirection::kSending>();

void WlanSoftmacDevice::Query(fdf::Arena& arena, QueryCompleter::Sync& completer) {
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);

  fidl::Arena<kWlanSoftmacQueryResponseBufferSize> table_arena;
  fuchsia_wlan_softmac::wire::WlanSoftmacQueryResponse resp;
  zx_status_t status = mac_query(mvmvif_, &resp, table_arena);

  if (status != ZX_OK) {
    IWL_ERR(this, "failed query: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess(resp);
}

void WlanSoftmacDevice::QueryDiscoverySupport(fdf::Arena& arena,
                                              QueryDiscoverySupportCompleter::Sync& completer) {
  fuchsia_wlan_softmac::wire::DiscoverySupport out_resp;
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  mac_query_discovery_support(&out_resp, arena);
  completer.buffer(arena).ReplySuccess(out_resp);
}

void WlanSoftmacDevice::QueryMacSublayerSupport(fdf::Arena& arena,
                                                QueryMacSublayerSupportCompleter::Sync& completer) {
  fuchsia_wlan_common::wire::MacSublayerSupport out_resp;
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  mac_query_mac_sublayer_support(&out_resp, arena);
  completer.buffer(arena).ReplySuccess(out_resp);
}

void WlanSoftmacDevice::QuerySecuritySupport(fdf::Arena& arena,
                                             QuerySecuritySupportCompleter::Sync& completer) {
  fuchsia_wlan_common::wire::SecuritySupport out_resp;
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  mac_query_security_support(&out_resp, arena);
  completer.buffer(arena).ReplySuccess(out_resp);
}

void WlanSoftmacDevice::QuerySpectrumManagementSupport(
    fdf::Arena& arena, QuerySpectrumManagementSupportCompleter::Sync& completer) {
  fuchsia_wlan_common::wire::SpectrumManagementSupport out_resp;
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  mac_query_spectrum_management_support(&out_resp, arena);
  completer.buffer(arena).ReplySuccess(out_resp);
}

void WlanSoftmacDevice::Start(StartRequestView request, fdf::Arena& arena,
                              StartCompleter::Sync& completer) {
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  zx::channel out_mlme_channel;

  client_ = fdf::WireSyncClient<fuchsia_wlan_softmac::WlanSoftmacIfc>(std::move(request->ifc));

  zx_status_t status = mac_start(mvmvif_, this, (zx_handle_t*)(&out_mlme_channel));
  if (status != ZX_OK) {
    IWL_ERR(this, "failed mac start: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);

    return;
  }

  completer.buffer(arena).ReplySuccess(std::move(out_mlme_channel));
}

void WlanSoftmacDevice::Stop(fdf::Arena& arena, StopCompleter::Sync& completer) {
  // Remove the stop logic from destructor if higher layer calls this function correctly.
  ap_mvm_sta_.reset();
  mac_stop(mvmvif_);
  completer.buffer(arena).Reply();
}

void WlanSoftmacDevice::QueueTx(QueueTxRequestView request, fdf::Arena& arena,
                                QueueTxCompleter::Sync& completer) {
  iwl_stats_inc(IWL_STATS_CNT_DATA_FROM_MLME);
  // Delayed transmission is never used right now.
  if (ap_mvm_sta_ == nullptr) {
    IWL_ERR(this, "No ap_sta is found.");
    completer.buffer(arena).ReplyError(ZX_ERR_BAD_STATE);
    return;
  }
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  zx_status_t status = ZX_OK;
  const auto& packet = request->packet;
  if (packet.mac_frame.size() > WLAN_MSDU_MAX_LEN) {
    IWL_ERR(mvmvif_, "Frame size is to large (%lu). expect less than %lu.\n",
            packet.mac_frame.size(), WLAN_MSDU_MAX_LEN);
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  ieee80211_mac_packet mac_packet = {};
  mac_packet.common_header =
      reinterpret_cast<const ieee80211_frame_header*>(packet.mac_frame.data());
  mac_packet.header_size = ieee80211_get_header_len(mac_packet.common_header);
  if (mac_packet.header_size > packet.mac_frame.size()) {
    IWL_ERR(mvmvif_, "TX packet header size %zu too large for data size %zu\n",
            mac_packet.header_size, packet.mac_frame.size());
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  mac_packet.body = packet.mac_frame.data() + mac_packet.header_size;
  mac_packet.body_size = packet.mac_frame.size() - mac_packet.header_size;
  if (ieee80211_pkt_is_protected(mac_packet.common_header)) {
    switch (ieee80211_get_frame_type(mac_packet.common_header)) {
      case ieee80211_frame_type::IEEE80211_FRAME_TYPE_MGMT:
        mac_packet.info.control.hw_key = ap_mvm_sta_->GetKey(WLAN_KEY_TYPE_IGTK);
        break;
      case ieee80211_frame_type::IEEE80211_FRAME_TYPE_DATA:
        mac_packet.info.control.hw_key = ap_mvm_sta_->GetKey(WLAN_KEY_TYPE_PAIRWISE);
        break;
      default:
        break;
    }
  }

  auto lock = std::lock_guard(mvmvif_->mvm->mutex);
  status = iwl_mvm_mac_tx(mvmvif_, ap_mvm_sta_->iwl_mvm_sta(), &mac_packet);
  if (status != ZX_OK) {
    IWL_ERR(this, "failed mac tx: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  // Delayed transmission is never used right now, setting enqueue_pending to false;
  completer.buffer(arena).ReplySuccess();
}

// Reject the request that firmware doesn't allow. See fxb/89911 for more context.
bool WlanSoftmacDevice::IsValidChannel(fuchsia_wlan_ieee80211::wire::ChannelNumber channel,
                                       fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw) {
  if (cbw == fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw40 ||
      cbw == fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw40Below) {
    if (channel.band == fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz && channel.number >= 10 &&
        channel.number <= 14) {
      IWL_WARN(mvmvif_, "The 40%sMHz bandwidth is not supported on the channel %d.\n",
               cbw == fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw40Below ? "-" : "",
               channel.number);
      return false;
    }
  }

  if (channel.band == fuchsia_wlan_ieee80211::wire::WlanBand::kTwoGhz &&
      cbw >= fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw80) {
    IWL_WARN(mvmvif_, "The 80+MHz bandwidth is not supported on the 2.4GHz band (channel %d).\n",
             channel.number);
    return false;
  }

  return true;
}

void WlanSoftmacDevice::SetChannel(SetChannelRequestView request, fdf::Arena& arena,
                                   SetChannelCompleter::Sync& completer) {
  zx_status_t status = ZX_OK;

  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);

  if (!request->has_primary() || !request->has_bandwidth()) {
    IWL_WARN(this, "Missing primary or bandwidth in SetChannel request.");
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (!IsValidChannel(request->primary(), request->bandwidth())) {
    IWL_WARN(this, "Invalid channel.");
    completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  wlan_channel_t channel = {
      .primary = request->primary().number,
      .vht_secondary_80_channel = request->has_vht_secondary_80_channel()
                                      ? request->vht_secondary_80_channel().number
                                      : static_cast<uint8_t>(0),
  };

  switch (request->bandwidth()) {
    case fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw20:
      channel.bandwidth = CHANNEL_BANDWIDTH_CBW20;
      break;
    case fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw40:
      channel.bandwidth = CHANNEL_BANDWIDTH_CBW40;
      break;
    case fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw40Below:
      channel.bandwidth = CHANNEL_BANDWIDTH_CBW40BELOW;
      break;
    case fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw80:
      channel.bandwidth = CHANNEL_BANDWIDTH_CBW80;
      break;
    case fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw160:
      channel.bandwidth = CHANNEL_BANDWIDTH_CBW160;
      break;
    case fuchsia_wlan_ieee80211::wire::ChannelBandwidth::kCbw80P80:
      channel.bandwidth = CHANNEL_BANDWIDTH_CBW80P80;
      break;
    default:
      IWL_ERR(this, "Bandwidth (%u) is not supported", static_cast<uint32_t>(request->bandwidth()));
      completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
      return;
  }

  status = mac_set_channel(mvmvif_, &channel);
  if (status != ZX_OK) {
    IWL_ERR(this, "failed mac set channel: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess();
}  // namespace wlan::iwlwifi

void WlanSoftmacDevice::JoinBss(JoinBssRequestView request, fdf::Arena& arena,
                                JoinBssCompleter::Sync& completer) {
  zx_status_t status = ZX_OK;
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);

  // Ensure the request has the necessary fields
  if (!request->join_request.has_bssid()) {
    IWL_ERR(this, "bssid info not available in join request\n");
    return;
  }

  // Unassoc if it was assocated.
  if (ap_mvm_sta_ != nullptr) {
    IWL_INFO(this, "AP sta already exist.  Unassociate it first.\n");

    if ((status = mac_leave_bss(mvmvif_)) != ZX_OK) {
      IWL_ERR(this, "failed mac unconfigure bss: %s\n", zx_status_get_string(status));
      completer.buffer(arena).ReplyError(status);
      return;
    }
    mvmvif_->bss_conf.assoc = false;
    ap_mvm_sta_.reset();
  }

  if ((status = mac_join_bss(&vif_, &request->join_request)) != ZX_OK) {
    IWL_ERR(this, "failed mac configure bss: %s\n", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  ZX_DEBUG_ASSERT(mvmvif_->mac_role == WLAN_MAC_ROLE_CLIENT);
  std::unique_ptr<MvmSta> ap_mvm_sta;
  if ((status = MvmSta::Create(mvmvif_, request->join_request.bssid().begin(), &ap_mvm_sta)) !=
      ZX_OK) {
    IWL_ERR(this, "failed creating MvmSta: %s\n", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  ap_mvm_sta_ = std::move(ap_mvm_sta);
  completer.buffer(arena).ReplySuccess();
}

void WlanSoftmacDevice::EnableBeaconing(EnableBeaconingRequestView request, fdf::Arena& arena,
                                        EnableBeaconingCompleter::Sync& completer) {
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  zx_status_t status = mac_enable_beaconing(mvmvif_, request);
  if (status != ZX_OK) {
    // Expected for now since this is not supported yet in iwlwifi driver.
    IWL_ERR(this, "failed mac enable beaconing: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess();
}

void WlanSoftmacDevice::DisableBeaconing(fdf::Arena& arena,
                                         DisableBeaconingCompleter::Sync& completer) {
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  zx_status_t status = mac_disable_beaconing(mvmvif_);
  if (status != ZX_OK) {
    // Expected for now since this is not supported yet in iwlwifi driver.
    IWL_ERR(this, "failed mac disable beaconing: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess();
}

void WlanSoftmacDevice::InstallKey(InstallKeyRequestView request, fdf::Arena& arena,
                                   InstallKeyCompleter::Sync& completer) {
  if (ap_mvm_sta_ == nullptr) {
    IWL_ERR(this, "Ap sta does not exist.");
    completer.buffer(arena).ReplyError(ZX_ERR_BAD_STATE);
    return;
  }
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  zx_status_t status = ap_mvm_sta_->InstallKey(request);
  if (status != ZX_OK) {
    IWL_ERR(this, "failed InstallKey: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess();
}

void WlanSoftmacDevice::NotifyAssociationComplete(
    NotifyAssociationCompleteRequestView request, fdf::Arena& arena,
    NotifyAssociationCompleteCompleter::Sync& completer) {
  if (ap_mvm_sta_ == nullptr) {
    IWL_ERR(this, "Ap sta does not exist.");
    completer.buffer(arena).ReplyError(ZX_ERR_BAD_STATE);
    return;
  }
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  zx_status_t status = mac_notify_association_complete(mvmvif_, &request->assoc_cfg);
  if (status != ZX_OK) {
    IWL_ERR(this, "failed mac configure assoc: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess();
}

void WlanSoftmacDevice::ClearAssociation(ClearAssociationRequestView request, fdf::Arena& arena,
                                         ClearAssociationCompleter::Sync& completer) {
  zx_status_t status = ZX_OK;

  if (!request->has_peer_addr()) {
    IWL_ERR(this, "Request does not contain peer address.");
    completer.buffer(arena).ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (ap_mvm_sta_ == nullptr) {
    IWL_ERR(this, "Ap sta does not exist.");
    completer.buffer(arena).ReplyError(ZX_ERR_BAD_STATE);
    return;
  }
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);

  // Mark the station is no longer associated. This must be set before we start operating on the STA
  // instance.
  mvmvif_->bss_conf.assoc = false;
  ap_mvm_sta_.reset();

  if ((status = mac_clear_association(mvmvif_, request->peer_addr().data())) != ZX_OK) {
    IWL_ERR(this, "failed clear assoc: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  completer.buffer(arena).ReplySuccess();
}

void WlanSoftmacDevice::StartPassiveScan(StartPassiveScanRequestView request, fdf::Arena& arena,
                                         StartPassiveScanCompleter::Sync& completer) {
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  uint64_t out_scan_id;
  zx_status_t status = mac_start_passive_scan(mvmvif_, request, &out_scan_id);
  if (status != ZX_OK) {
    IWL_ERR(this, "failed start passive scan: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  fidl::Arena fidl_arena;
  auto builder =
      fuchsia_wlan_softmac::wire::WlanSoftmacBaseStartPassiveScanResponse::Builder(fidl_arena);
  builder.scan_id(out_scan_id);
  completer.buffer(arena).ReplySuccess(builder.Build());
}

void WlanSoftmacDevice::StartActiveScan(StartActiveScanRequestView request, fdf::Arena& arena,
                                        StartActiveScanCompleter::Sync& completer) {
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  uint64_t out_scan_id;
  zx_status_t status = mac_start_active_scan(mvmvif_, request, &out_scan_id);
  if (status != ZX_OK) {
    IWL_ERR(this, "failed start active scan: %s", zx_status_get_string(status));
    completer.buffer(arena).ReplyError(status);
    return;
  }

  fidl::Arena fidl_arena;
  auto builder =
      fuchsia_wlan_softmac::wire::WlanSoftmacBaseStartActiveScanResponse::Builder(fidl_arena);
  builder.scan_id(out_scan_id);
  completer.buffer(arena).ReplySuccess(builder.Build());
}

void WlanSoftmacDevice::CancelScan(CancelScanRequestView request, fdf::Arena& arena,
                                   CancelScanCompleter::Sync& completer) {
  // TODO(fxbug.dev/107743): Implement.
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanSoftmacDevice::UpdateWmmParameters(UpdateWmmParametersRequestView request,
                                            fdf::Arena& arena,
                                            UpdateWmmParametersCompleter::Sync& completer) {
  IWL_ERR(this, "needs porting");
  CHECK_DELETE_IN_PROGRESS_WITH_ERRSYNTAX(mvmvif_);
  completer.buffer(arena).ReplyError(ZX_ERR_NOT_SUPPORTED);
}

void WlanSoftmacDevice::ServiceConnectHandler(
    fdf_dispatcher_t* dispatcher, fdf::ServerEnd<fuchsia_wlan_softmac::WlanSoftmac> server_end) {
  fdf::BindServer(dispatcher, std::move(server_end), this);
}

void WlanSoftmacDevice::Recv(fuchsia_wlan_softmac::wire::WlanRxPacket* rx_packet) {
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    IWL_ERR(this, "Failed to create Arena in WlanSoftmacDevice::Recv().\n");
    return;
  }
  auto result = client_.buffer(*std::move(arena))->Recv(*rx_packet);
  if (!result.ok()) {
    IWL_ERR(this, "Failed to send rx frames up in WlanSoftmacDevice::Recv(). Status: %d\n",
            result.status());
  }
}

void WlanSoftmacDevice::NotifyScanComplete(const zx_status_t status, const uint64_t scan_id) {
  auto arena = fdf::Arena::Create(0, 0);
  if (arena.is_error()) {
    IWL_ERR(this,
            "Failed to create Arena in WlanSoftmacDevice::NotifyScanComplete(). "
            "scan_id=%zu, status=%s\n",
            scan_id, zx_status_get_string(status));
    return;
  }

  fidl::Arena fidl_arena;
  auto builder =
      fuchsia_wlan_softmac::wire::WlanSoftmacIfcBaseNotifyScanCompleteRequest::Builder(fidl_arena);
  builder.status(status);
  builder.scan_id(scan_id);

  auto result = client_.buffer(*std::move(arena))->NotifyScanComplete(builder.Build());
  if (!result.ok()) {
    IWL_ERR(
        this,
        "Failed to send scan complete notification up in WlanSoftmacDevice::NotifyScanComplete(). "
        "result.status: %d, scan_id=%zu, status=%s\n",
        result.status(), scan_id, zx_status_get_string(status));
  }
}

void WlanSoftmacDevice::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_wlan_softmac::WlanSoftmac> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  IWL_ERR(this, "Unknown method ordinal: %lu\n", metadata.method_ordinal);
}

}  // namespace wlan::iwlwifi
