// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "wlantap-mac.h"

#include <lib/driver/logging/cpp/logger.h>

#include <wlan/common/channel.h>
#include <wlan/drivers/log.h>

#include "lib/fidl/cpp/wire/channel.h"
#include "lib/fidl_driver/cpp/wire_messaging_declarations.h"
#include "utils.h"

namespace {
// Large enough to back a full WlanSoftmacQueryResponse FIDL struct.
constexpr size_t kWlanSoftmacQueryResponseBufferSize = 5120;
}  // namespace

namespace wlan {

WlantapMac::WlantapMac(Listener* listener, fuchsia_wlan_common::WlanMacRole role,
                       const fuchsia_wlan_tap::WlantapPhyConfig& config, zx::channel sme_channel)
    : listener_(listener), role_(role), phy_config_(config), sme_channel_(std::move(sme_channel)) {}

void WlantapMac::Query(QueryCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::info("Query(): {}", phy_config_.hardware_capability());
  fidl::Arena<kWlanSoftmacQueryResponseBufferSize> table_arena;
  fuchsia_wlan_softmac::WlanSoftmacQueryResponse resp;
  ConvertTapPhyConfig(&resp, phy_config_);
  completer.Reply(fit::ok(resp));
}

fidl::ProtocolHandler<fuchsia_wlan_softmac::WlanSoftmac> WlantapMac::ProtocolHandler() {
  WLAN_TRACE_DURATION();
  return bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->get(),
                                 fidl::kIgnoreBindingClosure);
}

void WlantapMac::QueryDiscoverySupport(QueryDiscoverySupportCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  completer.Reply(fit::ok(phy_config_.discovery_support()));
}

void WlantapMac::QueryMacSublayerSupport(QueryMacSublayerSupportCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  completer.Reply(fit::ok(phy_config_.mac_sublayer_support()));
}

void WlantapMac::QuerySecuritySupport(QuerySecuritySupportCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  completer.Reply(fit::ok(phy_config_.security_support()));
}

void WlantapMac::QuerySpectrumManagementSupport(
    QuerySpectrumManagementSupportCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  completer.Reply(fit::ok(phy_config_.spectrum_management_support()));
}

void WlantapMac::Start(StartRequest& request, StartCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  fdf::info("Calling Start()");
  if (!sme_channel_.is_valid()) {
    completer.Reply(fit::error(ZX_ERR_ALREADY_BOUND));
    return;
  }

  listener_->WlantapMacStart(std::move(request.ifc()));
  completer.Reply(fit::ok(std::move(sme_channel_)));
}

void WlantapMac::Stop(StopCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  listener_->WlantapMacStop();
  completer.Reply();
}

void WlantapMac::QueueTx(QueueTxRequest& request, QueueTxCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  listener_->WlantapMacQueueTx(request.packet());
  completer.Reply(fit::ok());
}

void WlantapMac::SetChannel(SetChannelRequest& request, SetChannelCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  if (!request.primary().has_value()) {
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  fuchsia_wlan_ieee80211::ChannelNumber channel_num = request.primary().value();

  fuchsia_wlan_ieee80211::ChannelBandwidth cbw =
      request.bandwidth().value_or(fuchsia_wlan_ieee80211::ChannelBandwidth::kCbw20);

  fuchsia_wlan_ieee80211::ChannelNumber secondary80 =
      request.vht_secondary_80_channel().value_or(fuchsia_wlan_ieee80211::ChannelNumber{{
          .band = channel_num.band(),
          .number = 0,
      }});

  fuchsia_wlan_ieee80211::wire::ChannelNumber wire_channel_num{
      .band = static_cast<fuchsia_wlan_ieee80211::wire::WlanBand>(channel_num.band()),
      .number = channel_num.number(),
  };
  fuchsia_wlan_ieee80211::wire::ChannelBandwidth wire_cbw =
      static_cast<fuchsia_wlan_ieee80211::wire::ChannelBandwidth>(cbw);
  fuchsia_wlan_ieee80211::wire::ChannelNumber wire_secondary80{
      .band = static_cast<fuchsia_wlan_ieee80211::wire::WlanBand>(secondary80.band()),
      .number = secondary80.number(),
  };

  wlan::common::Channel channel = {
      .primary = wire_channel_num,
      .bandwidth = wire_cbw,
      .vht_secondary_80_channel = wire_secondary80,
  };
  if (!wlan::common::IsValidChan(channel)) {
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  listener_->WlantapMacSetChannel(channel_num, cbw, secondary80);
  completer.Reply(fit::ok());
}

void WlantapMac::JoinBss(JoinBssRequest& request, JoinBssCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  bool expected_remote = role_ == fuchsia_wlan_common::WlanMacRole::kClient;
  if (request.join_request().remote() != expected_remote) {
    completer.Reply(fit::error(ZX_ERR_INVALID_ARGS));
    return;
  }
  listener_->WlantapMacJoinBss(request.join_request());
  completer.Reply(fit::ok());
}

void WlantapMac::EnableBeaconing(EnableBeaconingRequest& request,
                                 EnableBeaconingCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();

  // This is the test driver, so we can just pretend beaconing was enabled.
  completer.Reply(fit::ok());
}

void WlantapMac::DisableBeaconing(DisableBeaconingCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();

  // This is the test driver, so we can just pretend the beacon was configured.
  completer.Reply(fit::ok());
}

void WlantapMac::StartPassiveScan(StartPassiveScanRequest& request,
                                  StartPassiveScanCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  uint64_t scan_id = 111;
  listener_->WlantapMacStartScan(scan_id);
  fuchsia_wlan_softmac::WlanSoftmacBaseStartPassiveScanResponse response{{.scan_id = scan_id}};
  completer.Reply(fit::ok(response));
}

void WlantapMac::StartActiveScan(StartActiveScanRequest& request,
                                 StartActiveScanCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  uint64_t scan_id = 222;
  listener_->WlantapMacStartScan(scan_id);

  fuchsia_wlan_softmac::WlanSoftmacBaseStartActiveScanResponse response{{.scan_id = scan_id}};
  completer.Reply(fit::ok(response));
}

void WlantapMac::InstallKey(InstallKeyRequest& request, InstallKeyCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  listener_->WlantapMacSetKey(request);
  completer.Reply(fit::ok());
}

void WlantapMac::NotifyAssociationComplete(NotifyAssociationCompleteRequest& request,
                                           NotifyAssociationCompleteCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();

  // This is the test driver, so we can just pretend the association was configured.
  // TODO(https://fxbug.dev/42103599): Evaluate the use and implement
  completer.Reply(fit::ok());
}

void WlantapMac::ClearAssociation(ClearAssociationRequest& request,
                                  ClearAssociationCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();

  // TODO(https://fxbug.dev/42103599): Evaluate the use and implement.
  // Association is never configured, so there is nothing to clear.
  completer.Reply(fit::ok());
}

void WlantapMac::CancelScan(CancelScanRequest& request, CancelScanCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  ZX_PANIC("CancelScan is not supported.");
}

void WlantapMac::UpdateWmmParameters(UpdateWmmParametersRequest& request,
                                     UpdateWmmParametersCompleter::Sync& completer) {
  WLAN_TRACE_DURATION();
  ZX_PANIC("UpdateWmmParameters is not supported.");
}

}  // namespace wlan
