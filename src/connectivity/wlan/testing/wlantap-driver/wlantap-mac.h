// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_MAC_H_
#define SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_MAC_H_

#include <fidl/fuchsia.wlan.softmac/cpp/driver/fidl.h>
#include <fidl/fuchsia.wlan.tap/cpp/driver/fidl.h>
#include <fidl/fuchsia.wlan.tap/cpp/fidl.h>
#include <lib/driver/logging/cpp/logger.h>

#include "fidl/fuchsia.wlan.softmac/cpp/markers.h"

namespace wlan {

// Serves the WlanSoftmac protocol.
// This class either responds to calls based on the given phy_config, or forwards calls to the
// Listener.
class WlantapMac : public fdf::Server<::fuchsia_wlan_softmac::WlanSoftmac> {
 public:
  // An interface to allow another class to intercept WlanSoftmac calls.
  // Many of the functions in WlantapMac simply forward calls along to the Listener.
  class Listener {
   public:
    virtual void WlantapMacStart(
        fdf::ClientEnd<::fuchsia_wlan_softmac::WlanSoftmacIfc> ifc_client) = 0;
    virtual void WlantapMacStop() = 0;
    virtual void WlantapMacQueueTx(const fuchsia_wlan_softmac::WlanTxPacket& pkt) = 0;
    virtual void WlantapMacSetChannel(const fuchsia_wlan_ieee80211::ChannelNumber& channel,
                                      fuchsia_wlan_ieee80211::ChannelBandwidth cbw,
                                      const fuchsia_wlan_ieee80211::ChannelNumber& secondary80) = 0;
    virtual void WlantapMacJoinBss(const fuchsia_wlan_driver::JoinBssRequest& join_request) = 0;
    virtual void WlantapMacStartScan(uint64_t scan_id) = 0;
    virtual void WlantapMacSetKey(const fuchsia_wlan_softmac::WlanKeyConfiguration& key_config) = 0;
  };

  WlantapMac(Listener* listener, fuchsia_wlan_common::WlanMacRole,
             const fuchsia_wlan_tap::WlantapPhyConfig& config, zx::channel sme_channel);

  fidl::ProtocolHandler<fuchsia_wlan_softmac::WlanSoftmac> ProtocolHandler();

  // WlanSoftmac protocol implementation.
  void Query(QueryCompleter::Sync& completer) override;
  void QueryDiscoverySupport(QueryDiscoverySupportCompleter::Sync& completer) override;
  void QueryMacSublayerSupport(QueryMacSublayerSupportCompleter::Sync& completer) override;
  void QuerySecuritySupport(QuerySecuritySupportCompleter::Sync& completer) override;
  void QuerySpectrumManagementSupport(
      QuerySpectrumManagementSupportCompleter::Sync& completer) override;
  void Start(StartRequest& request, StartCompleter::Sync& completer) override;
  void Stop(StopCompleter::Sync& completer) override;
  void QueueTx(QueueTxRequest& request, QueueTxCompleter::Sync& completer) override;
  void SetChannel(SetChannelRequest& request, SetChannelCompleter::Sync& completer) override;
  void JoinBss(JoinBssRequest& request, JoinBssCompleter::Sync& completer) override;
  void EnableBeaconing(EnableBeaconingRequest& request,
                       EnableBeaconingCompleter::Sync& completer) override;
  void DisableBeaconing(DisableBeaconingCompleter::Sync& completer) override;
  void InstallKey(InstallKeyRequest& request, InstallKeyCompleter::Sync& completer) override;
  void NotifyAssociationComplete(NotifyAssociationCompleteRequest& request,
                                 NotifyAssociationCompleteCompleter::Sync& completer) override;
  void ClearAssociation(ClearAssociationRequest& request,
                        ClearAssociationCompleter::Sync& completer) override;
  void StartPassiveScan(StartPassiveScanRequest& request,
                        StartPassiveScanCompleter::Sync& completer) override;
  void StartActiveScan(StartActiveScanRequest& request,
                       StartActiveScanCompleter::Sync& completer) override;
  void CancelScan(CancelScanRequest& request, CancelScanCompleter::Sync& completer) override;
  void UpdateWmmParameters(UpdateWmmParametersRequest& request,
                           UpdateWmmParametersCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_wlan_softmac::WlanSoftmac> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

 private:
  Listener* listener_;
  fuchsia_wlan_common::WlanMacRole role_;

  const fuchsia_wlan_tap::WlantapPhyConfig phy_config_;

  zx::channel sme_channel_;

  fdf::ServerBindingGroup<fuchsia_wlan_softmac::WlanSoftmac> bindings_;
};

}  // namespace wlan

#endif  // SRC_CONNECTIVITY_WLAN_TESTING_WLANTAP_DRIVER_WLANTAP_MAC_H_
