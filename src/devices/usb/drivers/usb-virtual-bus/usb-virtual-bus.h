// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_BUS_H_
#define SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_BUS_H_

#include <fidl/fuchsia.hardware.usb.virtual.bus/cpp/fidl.h>
#include <fuchsia/hardware/usb/bus/cpp/banjo.h>
#include <fuchsia/hardware/usb/hci/cpp/banjo.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/devfs/cpp/connector.h>
#include <lib/fit/function.h>

#include <format>
#include <memory>
#include <string_view>
#include <type_traits>
#include <vector>

#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-device.h"
#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-endpoint.h"
#include "src/devices/usb/drivers/usb-virtual-bus/usb-virtual-host.h"

namespace usb_virtual_bus {

/*
    THEORY OF OPERATION

    The usb-virtual-bus driver implements a virtual USB bus that can be used for testing USB
    drivers (both host-side and device-side) without requiring physical hardware. It operates by
    creating a virtual USB Host Controller Interface (HCI) and a virtual USB Device Controller
    Interface (DCI) and connecting them back-to-back. This setup simulates a physical USB host
    connected to a USB device.

    The core of the data forwarding logic is managed by an array of UsbVirtualEp objects, with
    each instance corresponding to a specific USB endpoint address. These objects act as the
    communication channel between the virtual host and device. When a host-side driver queues a
    USB request, the virtual HCI implementation receives it and places the request into the
    appropriate UsbVirtualEp. The virtual DCI, which is connected to the same UsbVirtualEp array,
    then makes this request available to the bound device-side driver. For data flowing from the
    device to the host (IN transfers), the process is reversed. The UsbVirtualEp structs serve as
    the shared transport medium, similar to a physical wire.

    The bus is controlled by a test program via the fuchsia.hardware.usb.virtual.bus.Bus
    FIDL protocol. This interface allows the test to orchestrate the test environment by
    enabling/disabling the bus and simulating device connection and disconnection events.

    The connection state is managed by a simple state machine within the driver. It can be in one
    of four states: kDisconnected, kConnecting, kConnected, or kDisconnecting. These states
    represent the progress of FIDL Connect/Disconnect requests. The actual hardware state is
    tracked by two boolean flags: dci_connected (Peripheral ready) and hci_connected (Host
    enumeration active).

    A Connect() call transitions the state to kConnecting and ensures both components are
    activated. A Disconnect() call transitions to kDisconnecting and ensures both are
    deactivated. The virtual bus reaches kConnected or kDisconnected only when both hardware
    components have acknowledged the transition.
*/

class UsbVirtualDevice;
class UsbVirtualHost;

// This is the main class for the USB virtual bus.
class UsbVirtualBus : public fdf::DriverBase2,
                      public fidl::Server<fuchsia_hardware_usb_virtual_bus::Bus> {
 public:
  enum class ConnectedState : uint8_t {
    kDisconnected = 0,
    kConnecting = 1,
    kConnected = 2,
    kDisconnecting = 3,
  };

 private:
  static constexpr std::string kName = "usb-virtual-bus";

 public:
  UsbVirtualBus()
      : fdf::DriverBase2(kName), devfs_connector_(fit::bind_member<&UsbVirtualBus::Serve>(this)) {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  zx::result<> SetBusInterface(
      fidl::ClientEnd<fuchsia_hardware_usb_hci::UsbHciInterface> client_end);
  zx::result<> SetDciInterface(
      fidl::ClientEnd<fuchsia_hardware_usb_dci::UsbDciInterface> client_end);

  // Events from the virtual device (DCI) side.
  void OnStartDci(fit::callback<void(zx_status_t)> callback);
  void OnStopDci(fit::callback<void(zx_status_t)> callback);
  std::unique_ptr<UsbVirtualDevice>& device() { return get<UsbVirtualDevice>(); }
  std::unique_ptr<UsbVirtualHost>& host() { return get<UsbVirtualHost>(); }

  // fuchsia_hardware_usb_virtual_bus::Bus Methods
  void Enable(EnableCompleter::Sync& completer) override;
  void Disable(DisableCompleter::Sync& completer) override;
  void Connect(ConnectCompleter::Sync& completer) override;
  void Disconnect(DisconnectCompleter::Sync& completer) override;

  ConnectedState GetConnectedState() const { return connected_; }
  void SetConnectedState(ConnectedState state);

  UsbVirtualEp& ep(uint8_t index) { return eps_[index]; }

  async_dispatcher_t* async_dispatcher() { return dispatcher(); }

  const std::shared_ptr<fdf::Namespace>& incoming() const { return incoming_; }
  const std::string& node_name() const { return node_name_; }

  template <typename T>
  void FinishRemove() {
    if (!removed_) {
      return;
    }

    // Only the host-side contains a compat server.
    if constexpr (std::is_same_v<T, UsbVirtualHost>) {
      get<T>()->compat_server().reset();
    }

    zx::result result = outgoing()->RemoveService<typename T::Service>();
    if (result.is_error()) {
      fdf::error("Failed to remove device service: {}", result);
      // Continue despite failure.
    }
    get<T>().reset();
  }

 private:
  DISALLOW_COPY_ASSIGN_AND_MOVE(UsbVirtualBus);

  friend class UsbVirtualEp;

  void ConnectInternal(fit::callback<void(zx_status_t)> callback);
  void DisconnectInternal(fit::callback<void(zx_status_t)> callback);

  void HciSetConnected();
  void OnHciConnectCompleted(zx_status_t status);
  void DciSetConnected();
  void OnDciConnectCompleted(zx_status_t status);

  void HciSetDisconnected();
  void OnHciDisconnectCompleted(zx_status_t status);
  void DciSetDisconnected(zx_status_t status);
  void OnDciDisconnectCompleted(zx_status_t status);

  std::vector<fit::callback<void(zx_status_t)>> connect_callbacks_;
  std::vector<fit::callback<void(zx_status_t)>> disconnect_callbacks_;
  std::vector<fit::callback<void(zx_status_t)>> disable_callbacks_;

  void Serve(fidl::ServerEnd<fuchsia_hardware_usb_virtual_bus::Bus> request);
  void Disable(fit::callback<void(zx_status_t)> callback);

  zx::result<std::unique_ptr<UsbVirtualHost>> CreateHost();
  zx::result<std::unique_ptr<UsbVirtualDevice>> CreateDevice();
  template <typename T>
  zx::result<> RemoveChild(std::unique_ptr<T>& child);

  template <typename T>
  std::unique_ptr<T>& get();
  template <>
  std::unique_ptr<UsbVirtualHost>& get() {
    return host_;
  }
  template <>
  std::unique_ptr<UsbVirtualDevice>& get() {
    return device_;
  }

  fdf::OwnedChildNode child_;
  driver_devfs::Connector<fuchsia_hardware_usb_virtual_bus::Bus> devfs_connector_;
  fidl::ServerBindingGroup<fuchsia_hardware_usb_virtual_bus::Bus> bindings_;

  // Reference to class that implements the virtual device controller protocol.
  std::unique_ptr<UsbVirtualDevice> device_;
  // Reference to class that implements the virtual host controller protocol.
  std::unique_ptr<UsbVirtualHost> host_;

  fidl::Client<fuchsia_hardware_usb_dci::UsbDciInterface> dci_intf_;
  fidl::Client<fuchsia_hardware_usb_hci::UsbHciInterface> hci_intf_;

  UsbVirtualEp eps_[USB_MAX_EPS];

  ConnectedState connected_ = ConnectedState::kDisconnected;

  bool dci_connected_ = false;
  bool hci_connected_ = false;
  std::shared_ptr<fdf::Namespace> incoming_;
  std::string node_name_;
  std::atomic_bool removed_{false};
};

}  // namespace usb_virtual_bus

template <>
struct std::formatter<usb_virtual_bus::UsbVirtualBus::ConnectedState>
    : std::formatter<std::string_view> {
  auto format(usb_virtual_bus::UsbVirtualBus::ConnectedState state,
              std::format_context& ctx) const {
    std::string_view name = "unknown";
    switch (state) {
      case usb_virtual_bus::UsbVirtualBus::ConnectedState::kDisconnected:
        name = "Disconnected";
        break;
      case usb_virtual_bus::UsbVirtualBus::ConnectedState::kConnecting:
        name = "Connecting";
        break;
      case usb_virtual_bus::UsbVirtualBus::ConnectedState::kConnected:
        name = "Connected";
        break;
      case usb_virtual_bus::UsbVirtualBus::ConnectedState::kDisconnecting:
        name = "Disconnecting";
        break;
    }
    return std::formatter<std::string_view>::format(name, ctx);
  }
};

#endif  // SRC_DEVICES_USB_DRIVERS_USB_VIRTUAL_BUS_USB_VIRTUAL_BUS_H_
