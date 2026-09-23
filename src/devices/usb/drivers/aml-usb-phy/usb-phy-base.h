// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY_BASE_H_
#define SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY_BASE_H_

#include <fidl/fuchsia.hardware.usb.phy/cpp/fidl.h>
#include <lib/driver/mmio/cpp/mmio.h>

#include <cstdint>
#include <sstream>
#include <string>

namespace aml_usb_phy {

class UsbPhyBase {
 public:
  const fdf::MmioBuffer& mmio() const { return mmio_; }
  bool is_otg_capable() const { return is_otg_capable_; }
  fuchsia_hardware_usb_phy::Mode dr_mode() const { return dr_mode_; }
  uint32_t idx() const { return idx_; }
  fuchsia_hardware_usb_phy::ProtocolVersion protocol() const { return protocol_; }

  std::string name() const {
    std::stringstream stream;
    stream << (protocol_ == fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20 ? "UsbPhy2"
                                                                              : "UsbPhy3")
           << "[" << idx_ << "]";
    return stream.str();
  }

  fuchsia_hardware_usb_phy::Mode phy_mode() { return phy_mode_; }
  void SetMode(fuchsia_hardware_usb_phy::Mode mode, fdf::MmioBuffer& usbctrl_mmio) {
    SetModeInternal(mode, usbctrl_mmio);
    phy_mode_ = mode;
  }

  // Used for debugging.
  virtual void dump_regs() const = 0;

 protected:
  UsbPhyBase(uint32_t idx, fuchsia_hardware_usb_phy::ProtocolVersion protocol, fdf::MmioBuffer mmio,
             bool is_otg_capable, fuchsia_hardware_usb_phy::Mode dr_mode)
      : idx_(idx),
        protocol_(protocol),
        mmio_(std::move(mmio)),
        is_otg_capable_(is_otg_capable),
        dr_mode_(dr_mode) {}

 private:
  virtual void SetModeInternal(fuchsia_hardware_usb_phy::Mode mode,
                               fdf::MmioBuffer& usbctrl_mmio) = 0;

  const uint32_t idx_;
  const fuchsia_hardware_usb_phy::ProtocolVersion protocol_;
  fdf::MmioBuffer mmio_;
  const bool is_otg_capable_;
  const fuchsia_hardware_usb_phy::Mode dr_mode_;  // USB Controller Mode. Internal to Driver.

  fuchsia_hardware_usb_phy::Mode phy_mode_ =
      fuchsia_hardware_usb_phy::Mode::kUnknown;  // Physical USB mode. Must hold parent's lock_
                                                 // while accessing.
};

}  // namespace aml_usb_phy

#endif  // SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY_BASE_H_
