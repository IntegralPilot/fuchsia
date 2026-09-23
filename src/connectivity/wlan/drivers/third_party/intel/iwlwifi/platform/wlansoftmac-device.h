// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_WLANSOFTMAC_DEVICE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_WLANSOFTMAC_DEVICE_H_

#include <fidl/fuchsia.wlan.ieee80211/cpp/driver/wire.h>
#include <fidl/fuchsia.wlan.softmac/cpp/driver/wire.h>
#include <lib/fdf/cpp/dispatcher.h>

#include <memory>

#include "third_party/iwlwifi/platform/banjo/ieee80211.h"
#include "third_party/iwlwifi/platform/ieee80211.h"

struct iwl_mvm_vif;
struct iwl_trans;

namespace wlan::iwlwifi {

#include <fidl/fuchsia.wlan.softmac/cpp/driver/wire.h>
class MvmSta;
class WlanSoftmacDevice;

class WlanSoftmacDevice : public fdf::WireServer<fuchsia_wlan_softmac::WlanSoftmac> {
 public:
  WlanSoftmacDevice(iwl_trans* drvdata, uint16_t iface_id, struct iwl_mvm_vif* mvmvif);
  ~WlanSoftmacDevice();

  // WlanSoftmac protocol implementation.
  void Query(fdf::Arena& arena, QueryCompleter::Sync& completer) override;
  void QueryDiscoverySupport(fdf::Arena& arena,
                             QueryDiscoverySupportCompleter::Sync& completer) override;
  void QueryMacSublayerSupport(fdf::Arena& arena,
                               QueryMacSublayerSupportCompleter::Sync& completer) override;
  void QuerySecuritySupport(fdf::Arena& arena,
                            QuerySecuritySupportCompleter::Sync& completer) override;
  void QuerySpectrumManagementSupport(
      fdf::Arena& arena, QuerySpectrumManagementSupportCompleter::Sync& completer) override;
  void Start(StartRequestView request, fdf::Arena& arena, StartCompleter::Sync& completer) override;
  void Stop(fdf::Arena& arena, StopCompleter::Sync& completer) override;
  void QueueTx(QueueTxRequestView request, fdf::Arena& arena,
               QueueTxCompleter::Sync& completer) override;
  void SetChannel(SetChannelRequestView request, fdf::Arena& arena,
                  SetChannelCompleter::Sync& completer) override;
  void JoinBss(JoinBssRequestView request, fdf::Arena& arena,
               JoinBssCompleter::Sync& completer) override;
  void EnableBeaconing(EnableBeaconingRequestView request, fdf::Arena& arena,
                       EnableBeaconingCompleter::Sync& completer) override;
  void DisableBeaconing(fdf::Arena& arena, DisableBeaconingCompleter::Sync& completer) override;
  void InstallKey(InstallKeyRequestView request, fdf::Arena& arena,
                  InstallKeyCompleter::Sync& completer) override;
  void NotifyAssociationComplete(NotifyAssociationCompleteRequestView request, fdf::Arena& arena,
                                 NotifyAssociationCompleteCompleter::Sync& completer) override;
  void ClearAssociation(ClearAssociationRequestView request, fdf::Arena& arena,
                        ClearAssociationCompleter::Sync& completer) override;
  void StartPassiveScan(StartPassiveScanRequestView request, fdf::Arena& arena,
                        StartPassiveScanCompleter::Sync& completer) override;
  void StartActiveScan(StartActiveScanRequestView request, fdf::Arena& arena,
                       StartActiveScanCompleter::Sync& completer) override;
  void CancelScan(CancelScanRequestView request, fdf::Arena& arena,
                  CancelScanCompleter::Sync& completer) override;
  void UpdateWmmParameters(UpdateWmmParametersRequestView request, fdf::Arena& arena,
                           UpdateWmmParametersCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_wlan_softmac::WlanSoftmac> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

  // Entry functions to access WlanSoftmacIfc protocol implementation in client_.
  void Recv(fuchsia_wlan_softmac::wire::WlanRxPacket* rx_packet);
  void NotifyScanComplete(zx_status_t status, uint64_t scan_id);

  // Helper function
  bool IsValidChannel(fuchsia_wlan_ieee80211::wire::ChannelNumber channel,
                      fuchsia_wlan_ieee80211::wire::ChannelBandwidth cbw);

  void ServiceConnectHandler(fdf_dispatcher_t* dispatcher,
                             fdf::ServerEnd<fuchsia_wlan_softmac::WlanSoftmac> server_end);

 protected:
  struct ieee80211_vif vif_;
  struct iwl_mvm_vif* mvmvif_;

 private:
  iwl_trans* drvdata_;

  // Each peer on this interface will require a MvmSta instance.  For now, as we only support client
  // mode, we have only one peer (the AP), which simplifies things.
  std::unique_ptr<MvmSta> ap_mvm_sta_;

  // The FIDL client to communicate with Wlan device.
  fdf::WireSyncClient<fuchsia_wlan_softmac::WlanSoftmacIfc> client_;
};

}  // namespace wlan::iwlwifi

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_THIRD_PARTY_INTEL_IWLWIFI_PLATFORM_WLANSOFTMAC_DEVICE_H_
