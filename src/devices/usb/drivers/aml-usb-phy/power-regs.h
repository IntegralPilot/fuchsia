// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_POWER_REGS_H_
#define SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_POWER_REGS_H_

#include <zircon/types.h>

#include <hwreg/bitfields.h>

namespace aml_usb_phy {

class A0_RTI_GEN_PWR_SLEEP0 : public hwreg::RegisterBase<A0_RTI_GEN_PWR_SLEEP0, uint32_t> {
 public:
  DEF_BIT(0, dos_hcodec_power_off);
  DEF_BIT(1, dos_vdec_power_off);
  DEF_BIT(2, dos_hevc_power_off);
  DEF_BIT(3, dos_hevc_encoder_power_off);
  DEF_BIT(8, dos_hdmi_vpu_power_off);
  DEF_BIT(17, usb_comb_power_off);
  DEF_BIT(18, pci_comb_power_off);
  DEF_BIT(19, ge2d_power_off);
  static auto Get() { return hwreg::RegisterAddr<A0_RTI_GEN_PWR_SLEEP0>(0x0); }
};

class A0_RTI_GEN_PWR_ISO0 : public hwreg::RegisterBase<A0_RTI_GEN_PWR_ISO0, uint32_t> {
 public:
  DEF_BIT(0, dos_hcodec_isolation_enable);
  DEF_BIT(1, dos_vdec_isolation_enable);
  DEF_BIT(2, dos_hevc_isolation_enable);
  DEF_BIT(3, dos_hevc_encoder_isolation_enable);
  DEF_BIT(8, dos_hdmi_vpu_isolation_enable);
  DEF_BIT(17, usb_comb_isolation_enable);
  DEF_BIT(18, pci_comb_isolation_enable);
  DEF_BIT(19, ge2d_isolation_enable);
  static auto Get() { return hwreg::RegisterAddr<A0_RTI_GEN_PWR_ISO0>(0x4); }
};

// TODO: See https://fxbug.dev/42120085
class HHI_MEM_PD_REG0 : public hwreg::RegisterBase<HHI_MEM_PD_REG0, uint32_t> {
 public:
  DEF_FIELD(3, 2, __reserved_ethernet_memory_pd);
  DEF_FIELD(5, 4, __reserved_audio_memory_pd);
  DEF_FIELD(15, 8, __reserved_hdmi_memory_pd);
  DEF_FIELD(17, 16, __reserved_ddr_memory_pd);

  DEF_FIELD(25, 18, ge2d_pd);
  DEF_FIELD(29, 26, pcie_comb_pd);
  DEF_FIELD(31, 30, usb_comb_pd);

  static auto Get() { return hwreg::RegisterAddr<HHI_MEM_PD_REG0>(0x0); }
};

}  // namespace aml_usb_phy

#endif  // SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_POWER_REGS_H_
