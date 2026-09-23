// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY3_REGS_H_
#define SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY3_REGS_H_

#include <lib/driver/mmio/cpp/mmio.h>
#include <zircon/types.h>

#include <hwreg/bitfields.h>

namespace aml_usb_phy {

constexpr uint32_t PHY3_R1_OFFSET = 0x4;
constexpr uint32_t PHY3_R2_OFFSET = 0x8;
constexpr uint32_t PHY3_R4_OFFSET = 0x10;
constexpr uint32_t PHY3_R5_OFFSET = 0x14;

class PHY3_R1 : public hwreg::RegisterBase<PHY3_R1, uint32_t> {
 public:
  DEF_FIELD(4, 0, phy_tx1_term_offset);
  DEF_FIELD(9, 5, phy_tx0_term_offset);
  DEF_FIELD(12, 10, phy_rx1_eq);
  DEF_FIELD(15, 13, phy_rx0_eq);
  DEF_FIELD(20, 16, phy_los_level);
  DEF_FIELD(23, 21, phy_los_bias);
  DEF_BIT(24, phy_ref_clkdiv2);
  DEF_FIELD(31, 25, phy_mpll_multiplier);
  static auto Get() { return hwreg::RegisterAddr<PHY3_R1>(PHY3_R1_OFFSET); }
};

class PHY3_R2 : public hwreg::RegisterBase<PHY3_R2, uint32_t> {
 public:
  DEF_FIELD(5, 0, pcs_tx_deemph_gen2_6db);
  DEF_FIELD(11, 6, pcs_tx_deemph_gen2_3p5db);
  DEF_FIELD(17, 12, pcs_tx_deemph_gen1);
  DEF_FIELD(20, 18, phy_tx_vboost_lvl);
  DEF_FIELD(31, 21, reserved);
  static auto Get() { return hwreg::RegisterAddr<PHY3_R2>(PHY3_R2_OFFSET); }
};

class PHY3_R4 : public hwreg::RegisterBase<PHY3_R4, uint32_t> {
 public:
  DEF_BIT(0, phy_cr_write);
  DEF_BIT(1, phy_cr_read);
  DEF_FIELD(17, 2, phy_cr_data_in);
  DEF_BIT(18, phy_cr_cap_data);
  DEF_BIT(19, phy_cr_cap_addr);
  DEF_FIELD(31, 20, reserved);
  static auto Get() { return hwreg::RegisterAddr<PHY3_R4>(PHY3_R4_OFFSET); }
};

class PHY3_R5 : public hwreg::RegisterBase<PHY3_R5, uint32_t> {
 public:
  DEF_FIELD(15, 0, phy_cr_data_out);
  DEF_BIT(16, phy_cr_ack);
  DEF_BIT(17, phy_bs_out);
  DEF_FIELD(31, 18, reserved);
  static auto Get() { return hwreg::RegisterAddr<PHY3_R5>(PHY3_R5_OFFSET); }
};

}  // namespace aml_usb_phy

#endif  // SRC_DEVICES_USB_DRIVERS_AML_USB_PHY_USB_PHY3_REGS_H_
