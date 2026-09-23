// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_SOFTMAC_IFC_BRIDGE_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_SOFTMAC_IFC_BRIDGE_H_

#include <fidl/fuchsia.driver.framework/cpp/driver/fidl.h>
#include <fidl/fuchsia.wlan.softmac/cpp/driver/fidl.h>
#include <fidl/fuchsia.wlan.softmac/cpp/fidl.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fidl_driver/cpp/transport.h>
#include <lib/operation/ethernet.h>
#include <lib/trace-engine/types.h>
#include <lib/zx/result.h>

#include "src/connectivity/wlan/drivers/wlansoftmac/rust_driver/c-binding/bindings.h"

namespace wlan::drivers::wlansoftmac {

class SoftmacIfcBridge : public fdf::WireServer<fuchsia_wlan_softmac::WlanSoftmacIfc> {
 public:
  static zx::result<std::unique_ptr<SoftmacIfcBridge>> New(
      fidl::SharedClient<fuchsia_driver_framework::Node> node_client,
      const ethernet_tx_t* ethernet_tx, const wlan_rx_t* wlan_rx,
      fdf::ServerEnd<fuchsia_wlan_softmac::WlanSoftmacIfc>&& server_endpoint,
      fidl::ClientEnd<fuchsia_wlan_softmac::WlanSoftmacIfcBridge>&& bridge_client_endpoint);

  ~SoftmacIfcBridge() override = default;

  void Recv(RecvRequestView fdf_request, fdf::Arena& arena,
            RecvCompleter::Sync& completer) override;
  zx::result<> EthernetTx(std::unique_ptr<eth::BorrowedOperation<>> op,
                          trace_async_id_t async_id) const;
  void ReportTxResult(ReportTxResultRequestView request, fdf::Arena& arena,
                      ReportTxResultCompleter::Sync& completer) override;
  void NotifyScanComplete(NotifyScanCompleteRequestView request, fdf::Arena& arena,
                          NotifyScanCompleteCompleter::Sync& completer) override;
  void StopBridgedDriver(fit::callback<void()> stop_completer);
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_wlan_softmac::WlanSoftmacIfc> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  explicit SoftmacIfcBridge(const ethernet_tx_t* ethernet_tx, const wlan_rx_t* wlan_rx)
      : ethernet_tx_(*ethernet_tx), wlan_rx_(*wlan_rx) {}
  const ethernet_tx_t ethernet_tx_;
  const wlan_rx_t wlan_rx_;

  std::unique_ptr<fdf::ServerBinding<fuchsia_wlan_softmac::WlanSoftmacIfc>>
      softmac_ifc_server_binding_;

  std::unique_ptr<fidl::Client<fuchsia_wlan_softmac::WlanSoftmacIfcBridge>>
      softmac_ifc_bridge_client_;
};

}  // namespace wlan::drivers::wlansoftmac

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_WLANSOFTMAC_SOFTMAC_IFC_BRIDGE_H_
