// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_USB_DCI_INTERFACE_SERVER_H_
#define SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_USB_DCI_INTERFACE_SERVER_H_

#include <fidl/fuchsia.hardware.usb.dci/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/sync/cpp/completion.h>
#include <lib/trace/event.h>

namespace usb_peripheral {

class UsbPeripheral;
class UsbDciInterfaceServer : public fidl::WireServer<fuchsia_hardware_usb_dci::UsbDciInterface> {
 public:
  explicit UsbDciInterfaceServer(UsbPeripheral* drv) : drv_{drv} {
    auto result = fdf::SynchronizedDispatcher::Create(
        {}, "usb-dci-interface-server",
        [this](fdf_dispatcher_t*) { dispatcher_shutdown_.Signal(); });
    ZX_ASSERT(result.is_ok());
    dispatcher_ = std::move(*result);
  }

  void Stop();

  // fuchsia_hardware_usb_dci::UsbDciInterface protocol implementation.
  void Control(ControlRequestView req, ControlCompleter::Sync& completer) override;
  void SetConnected(SetConnectedRequestView req, SetConnectedCompleter::Sync& completer) override;
  void SetSpeed(SetSpeedRequestView req, SetSpeedCompleter::Sync& completer) override;

  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_usb_dci::UsbDciInterface> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override {
    TRACE_DURATION("usb-peripheral", __func__);
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  fidl::ClientEnd<fuchsia_hardware_usb_dci::UsbDciInterface> AddBinding() {
    TRACE_DURATION("usb-peripheral", __func__);
    libsync::Completion wait;
    auto eps = fidl::Endpoints<fuchsia_hardware_usb_dci::UsbDciInterface>::Create();
    async::PostTask(dispatcher_.async_dispatcher(), [this, &eps, &wait]() {
      bindings_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(eps.server),
                           this, fidl::kIgnoreBindingClosure);
      wait.Signal();
    });
    wait.Wait();
    return std::move(eps.client);
  }

 private:
  // Pointer to UsbPeripheral driver. This class must not outlive the driver instance.
  UsbPeripheral* drv_;

  // UsbDciInterfaceServer must live on a different dispatcher so that methods such as
  // `fuchsia_hardware_usb_dci::UsbDci::SetInterface` can call back into
  // `fuchsia_hardware_usb_dci::UsbDciInterface::SetConnected`.
  fdf::SynchronizedDispatcher dispatcher_;
  libsync::Completion dispatcher_shutdown_;
  fidl::ServerBindingGroup<fuchsia_hardware_usb_dci::UsbDciInterface> bindings_;
};

}  // namespace usb_peripheral

#endif  // SRC_DEVICES_USB_DRIVERS_USB_PERIPHERAL_USB_DCI_INTERFACE_SERVER_H_
