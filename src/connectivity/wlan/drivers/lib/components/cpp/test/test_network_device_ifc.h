// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_TEST_TEST_NETWORK_DEVICE_IFC_H_
#define SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_TEST_TEST_NETWORK_DEVICE_IFC_H_

#include <fidl/fuchsia.hardware.network.driver/cpp/driver/wire.h>

#include <functional>
#include <optional>

namespace wlan::drivers::components::test {

// Test implementation of network_device_ifc_protocol_t that contains mock calls useful for
// mocking and veriyfing interactions with a network device.
class TestNetworkDeviceIfc
    : public fdf::WireServer<fuchsia_hardware_network_driver::NetworkDeviceIfc> {
 public:
  TestNetworkDeviceIfc() = default;

  fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkPort>& PortClient() {
    return *port_;
  }

  zx::result<fdf::ClientEnd<fuchsia_hardware_network_driver::NetworkDeviceIfc>> Bind(
      fdf_dispatcher_t* dispatcher);

  // NetworkDeviceIfc methods
  void PortStatusChanged(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcPortStatusChangedRequest* request,
      fdf::Arena& arena, PortStatusChangedCompleter::Sync& completer) override {
    if (port_status_changed_) {
      port_status_changed_(request, arena, completer);
    }
  }
  void AddPort(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcAddPortRequest* request,
               fdf::Arena& arena, AddPortCompleter::Sync& completer) override {
    if (add_port_) {
      add_port_(request, arena, completer);
      if (request->port.is_valid()) {
        // Create a port client here if the callback did not take the client end.
        port_ =
            std::make_unique<fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkPort>>(
                std::move(request->port), fdf::Dispatcher::GetCurrent()->get());
      }
    } else {
      // Always create a port client if there is no callback.
      port_ = std::make_unique<fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkPort>>(
          std::move(request->port), fdf::Dispatcher::GetCurrent()->get());
      completer.buffer(arena).Reply(ZX_OK);
    }
  }
  void RemovePort(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcRemovePortRequest* request,
                  fdf::Arena& arena, RemovePortCompleter::Sync& completer) override {
    if (port_ && port_->is_valid()) {
      auto status = port_->buffer(arena)->Removed();
      ZX_ASSERT(status.ok());
    }
    if (remove_port_) {
      remove_port_(request, arena, completer);
    }
  }
  void CompleteRx(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest* request,
                  fdf::Arena& arena, CompleteRxCompleter::Sync& completer) override {
    if (complete_rx_) {
      complete_rx_(request, arena, completer);
    }
  }
  void CompleteTx(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest* request,
                  fdf::Arena& arena, CompleteTxCompleter::Sync& completer) override {
    if (complete_tx_) {
      complete_tx_(request, arena, completer);
    }
  }

  void DelegateRxLease(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcDelegateRxLeaseRequest* request,
      fdf::Arena& arena, DelegateRxLeaseCompleter::Sync& completer) override {
    if (delegate_rx_lease_) {
      delegate_rx_lease_(request, arena, completer);
    }
  }

  void UpdateRxBufferParams(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcUpdateRxBufferParamsRequest* request,
      fdf::Arena& arena, UpdateRxBufferParamsCompleter::Sync& completer) override {
    if (update_rx_buffer_params_) {
      update_rx_buffer_params_(request, arena, completer);
    }
  }

  void RequestRxSpace(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcRequestRxSpaceRequest* request,
      fdf::Arena& arena, RequestRxSpaceCompleter::Sync& completer) override {
    if (request_rx_space_) {
      request_rx_space_(request, arena, completer);
    }
  }

  std::function<void(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcPortStatusChangedRequest*, fdf::Arena&,
      PortStatusChangedCompleter::Sync&)>
      port_status_changed_;
  std::function<void(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcAddPortRequest*,
                     fdf::Arena&, AddPortCompleter::Sync&)>
      add_port_;
  std::function<void(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcRemovePortRequest*,
                     fdf::Arena&, RemovePortCompleter::Sync&)>
      remove_port_;
  std::function<void(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteRxRequest*,
                     fdf::Arena&, CompleteRxCompleter::Sync&)>
      complete_rx_;
  std::function<void(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcCompleteTxRequest*,
                     fdf::Arena&, CompleteTxCompleter::Sync&)>
      complete_tx_;
  fit::function<void(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcDelegateRxLeaseRequest*,
                     fdf::Arena&, DelegateRxLeaseCompleter::Sync&)>
      delegate_rx_lease_;
  std::function<void(
      fuchsia_hardware_network_driver::wire::NetworkDeviceIfcUpdateRxBufferParamsRequest*,
      fdf::Arena&, UpdateRxBufferParamsCompleter::Sync&)>
      update_rx_buffer_params_;
  std::function<void(fuchsia_hardware_network_driver::wire::NetworkDeviceIfcRequestRxSpaceRequest*,
                     fdf::Arena&, RequestRxSpaceCompleter::Sync&)>
      request_rx_space_;

 private:
  std::optional<fdf::ServerBindingRef<fuchsia_hardware_network_driver::NetworkDeviceIfc>> binding_;
  std::unique_ptr<fdf::WireSharedClient<fuchsia_hardware_network_driver::NetworkPort>> port_;
};

}  // namespace wlan::drivers::components::test

#endif  // SRC_CONNECTIVITY_WLAN_DRIVERS_LIB_COMPONENTS_CPP_TEST_TEST_NETWORK_DEVICE_IFC_H_
