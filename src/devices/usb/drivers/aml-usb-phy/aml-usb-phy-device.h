// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_AML_USB_PHY_DEVICE_H_
#define SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_AML_USB_PHY_DEVICE_H_

#include <fidl/fuchsia.driver.framework/cpp/wire.h>
#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/platform-device/cpp/pdev.h>

#include <mutex>

#include "src/devices/usb/drivers/aml-usb-phy/aml-usb-phy.h"

namespace aml_usb_phy {

class AmlUsbPhyDevice : public fdf::DriverBase2 {
 public:
  class ChildNode : public fidl::WireAsyncEventHandler<fuchsia_driver_framework::NodeController> {
   public:
    explicit ChildNode(AmlUsbPhyDevice* parent, std::string_view name, uint32_t property_did)
        : parent_(parent), name_(name), property_did_(property_did) {}

    zx::result<> Publish();
    using UnPublishCompleter = fit::callback<void(zx::result<>)>;
    void UnPublish(UnPublishCompleter completer);
    std::string_view name() const { return name_; }

    // fidl::WireAsyncEventHandler<fuchsia_driver_framework::NodeController>
    void on_fidl_error(fidl::UnbindInfo info) override;
    void handle_unknown_event(
        fidl::UnknownEventMetadata<fuchsia_driver_framework::NodeController> metadata) override {}

    UnPublishCompleter completer_ __TA_GUARDED(lock_);

   private:
    AmlUsbPhyDevice* parent_;
    const std::string_view name_;
    const uint32_t property_did_;

    std::mutex lock_;
    fidl::WireClient<fuchsia_driver_framework::NodeController> child_controller_
        __TA_GUARDED(lock_);
    std::atomic_uint32_t count_ __TA_GUARDED(lock_) = 0;
  };

  static constexpr std::string_view kDeviceName = "aml_usb_phy";

  AmlUsbPhyDevice() : fdf::DriverBase2(kDeviceName) {}

  zx::result<> Start(fdf::DriverContext context) override;
  void Stop(fdf::StopCompleter completer) override;

  // Unbind from parent node when a fatal failure occurs.
  void UnbindOnFailure();

  ChildNode xhci_{this, "xhci", PDEV_DID_USB_XHCI_COMPOSITE};
  ChildNode dwc2_{this, "dwc2-phy", PDEV_DID_USB_DWC2};

  // For testing.
  std::unique_ptr<AmlUsbPhy>& device() { return device_; }

 private:
  zx::result<> CreateNode();

  // Virtual for testing.
  virtual zx::result<fdf::MmioBuffer> MapMmio(fdf::PDev& pdev, uint32_t idx);

  std::unique_ptr<AmlUsbPhy> device_;

  fidl::ServerBindingGroup<fuchsia_hardware_usb_phy::UsbPhy> bindings_;
  fidl::ClientEnd<fuchsia_driver_framework::Node> node_client_;
  fidl::WireClient<fuchsia_driver_framework::NodeController> controller_;
};

}  // namespace aml_usb_phy

#endif  // SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_AML_USB_PHY_DEVICE_H_
