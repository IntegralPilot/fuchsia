// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_SOFTMAC_BRIDGE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_SOFTMAC_BRIDGE_H_

#include <fidl/fuchsia.driver.framework/cpp/driver/fidl.h>
#include <fidl/fuchsia.wlan.softmac/cpp/driver/fidl.h>
#include <fuchsia/hardware/ethernet/cpp/banjo.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/component/cpp/start_completer.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/operation/ethernet.h>
#include <lib/trace-engine/types.h>
#include <lib/zx/result.h>

#include <mutex>

#include <wlan/drivers/log.h>

#include "softmac_ifc_bridge.h"
#include "src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding/bindings.h"

namespace wlan::drivers::wlansoftmac {

using InitCompleter = fit::callback<void(zx_status_t status)>;

class SoftmacBridge : public fidl::Server<fuchsia_wlan_softmac::WlanSoftmacBridge> {
 public:
  static zx::result<std::unique_ptr<SoftmacBridge>> New(
      fidl::SharedClient<fuchsia_driver_framework::Node> node_client,
      fdf::StartCompleter start_completer, fit::callback<void(zx_status_t)> shutdown_completer,
      fdf::SharedClient<fuchsia_wlan_softmac::WlanSoftmac>&& softmac_client,
      std::shared_ptr<std::mutex> ethernet_proxy_lock,
      ddk::EthernetIfcProtocolClient* ethernet_proxy,
      std::optional<uint32_t>* cached_ethernet_status);
  void StopBridgedDriver(fit::callback<void()> completer);
  ~SoftmacBridge() override;

  void Query(QueryCompleter::Sync& completer) final;
  void QueryDiscoverySupport(QueryDiscoverySupportCompleter::Sync& completer) final;
  void QueryMacSublayerSupport(QueryMacSublayerSupportCompleter::Sync& completer) final;
  void QuerySecuritySupport(QuerySecuritySupportCompleter::Sync& completer) final;
  void QuerySpectrumManagementSupport(
      QuerySpectrumManagementSupportCompleter::Sync& completer) final;
  void Start(StartRequest& request, StartCompleter::Sync& completer) final;
  void SetEthernetStatus(SetEthernetStatusRequest& request,
                         SetEthernetStatusCompleter::Sync& completer) final;
  void SetChannel(SetChannelRequest& request, SetChannelCompleter::Sync& completer) final;
  void JoinBss(JoinBssRequest& request, JoinBssCompleter::Sync& completer) final;
  void EnableBeaconing(EnableBeaconingRequest& request,
                       EnableBeaconingCompleter::Sync& completer) final;
  void DisableBeaconing(DisableBeaconingCompleter::Sync& completer) final;
  void InstallKey(InstallKeyRequest& request, InstallKeyCompleter::Sync& completer) final;
  void NotifyAssociationComplete(NotifyAssociationCompleteRequest& request,
                                 NotifyAssociationCompleteCompleter::Sync& completer) final;
  void ClearAssociation(ClearAssociationRequest& request,
                        ClearAssociationCompleter::Sync& completer) final;
  void StartPassiveScan(StartPassiveScanRequest& request,
                        StartPassiveScanCompleter::Sync& completer) final;
  void StartActiveScan(StartActiveScanRequest& request,
                       StartActiveScanCompleter::Sync& completer) final;
  void CancelScan(CancelScanRequest& request, CancelScanCompleter::Sync& completer) final;
  void UpdateWmmParameters(UpdateWmmParametersRequest& request,
                           UpdateWmmParametersCompleter::Sync& completer) final;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_wlan_softmac::WlanSoftmacBridge> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {}

  zx::result<> EthernetTx(std::unique_ptr<eth::BorrowedOperation<>> op,
                          trace_async_id_t async_id) const;
  static zx_status_t WlanTx(void* ctx, const uint8_t* payload, size_t payload_size);
  static zx_status_t EthernetRx(void* ctx, const uint8_t* payload, size_t payload_size);

 private:
  explicit SoftmacBridge(fidl::SharedClient<fuchsia_driver_framework::Node> node_client,
                         fdf::SharedClient<fuchsia_wlan_softmac::WlanSoftmac>&& softmac_client,
                         std::shared_ptr<std::mutex> ethernet_proxy_lock,
                         ddk::EthernetIfcProtocolClient* ethernet_proxy,
                         std::optional<uint32_t>* cached_ethernet_status);

  template <typename, typename = void>
  static constexpr bool has_value_type = false;
  template <typename T>
  static constexpr bool has_value_type<T, std::void_t<typename T::value_type>> = true;

  std::unique_ptr<fidl::ServerBinding<fuchsia_wlan_softmac::WlanSoftmacBridge>>
      softmac_bridge_server_;
  fidl::SharedClient<fuchsia_driver_framework::Node> node_client_;
  fdf::SharedClient<fuchsia_wlan_softmac::WlanSoftmac> softmac_client_;

  // Mark `softmac_ifc_bridge_` as a mutable member of this class so `Start` can be a const function
  // that lazy-initializes `softmac_ifc_bridge_`. Note that `softmac_ifc_bridge_` is never mutated
  // again until its reset upon the framework calling the unbind hook.
  mutable std::unique_ptr<SoftmacIfcBridge> softmac_ifc_bridge_;

  std::shared_ptr<std::mutex> ethernet_proxy_lock_;
  ddk::EthernetIfcProtocolClient* ethernet_proxy_ __TA_GUARDED(ethernet_proxy_lock_);
  mutable std::optional<uint32_t>* cached_ethernet_status_ __TA_GUARDED(ethernet_proxy_lock_);
};

}  // namespace wlan::drivers::wlansoftmac

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_SOFTMAC_BRIDGE_H_
