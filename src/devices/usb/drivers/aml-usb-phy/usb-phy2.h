// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY2_H_
#define SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY2_H_

#include <lib/driver/mmio/cpp/mmio.h>

#include "src/devices/usb/drivers/aml-usb-phy/usb-phy-base.h"

namespace aml_usb_phy {

class AmlUsbPhy;

class UsbPhy2 final : public UsbPhyBase {
 public:
  UsbPhy2(uint8_t idx, fdf::MmioBuffer mmio, bool is_otg_capable,
          fuchsia_hardware_usb_phy::Mode dr_mode)
      : UsbPhyBase(idx, fuchsia_hardware_usb_phy::ProtocolVersion::kUsb20, std::move(mmio),
                   is_otg_capable, dr_mode) {}

  void InitPll(fuchsia_hardware_usb_phy::AmlogicPhyType type, bool needs_hack);

  void dump_regs() const override;

 private:
  void SetModeInternal(fuchsia_hardware_usb_phy::Mode mode, fdf::MmioBuffer& usbctrl_mmio) override;
};

}  // namespace aml_usb_phy

#endif  // SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY2_H_
