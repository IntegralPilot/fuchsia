// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HHI_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HHI_REGS_H_

#include <cstdint>

#include <hwreg/bitfields.h>

// The register definitions here are from AMLogic A311D Datasheet version 08.
//
// This datasheet is distributed by Khadas for the VIM3, at
// https://dl.khadas.com/hardware/VIM3/Datasheet/A311D_Datasheet_08_Wesion.pdf

// clang-format off
#define HHI_MIPI_CNTL0 (0x000 << 2)
#define HHI_MIPI_CNTL1 (0x001 << 2)
#define HHI_MIPI_CNTL2 (0x002 << 2)
#define HHI_GCLK_MPEG2 (0x052 << 2)
#define HHI_VDAC_CNTL0_G12A (0x0bb << 2)
#define HHI_VDAC_CNTL1_G12A (0x0bc << 2)
#define HHI_HDMI_PHY_CNTL0 (0xe8 << 2)
#define HHI_HDMI_PHY_CNTL1 (0xe9 << 2)
#define HHI_HDMI_PHY_CNTL2 (0xea << 2)
#define HHI_HDMI_PHY_CNTL3 (0xeb << 2)
#define HHI_HDMI_PHY_CNTL4 (0xec << 2)
#define HHI_HDMI_PHY_CNTL5 (0xed << 2)
#define HHI_HDMI_PHY_STATUS (0xee << 2)

#define ENCL_VIDEO_FILT_CTRL (0x1cc2 << 2)
#define ENCL_VIDEO_MAX_PXCNT (0x1cb0 << 2)
#define ENCL_VIDEO_HAVON_END (0x1cb1 << 2)
#define ENCL_VIDEO_HAVON_BEGIN (0x1cb2 << 2)
#define ENCL_VIDEO_VAVON_ELINE (0x1cb3 << 2)
#define ENCL_VIDEO_VAVON_BLINE (0x1cb4 << 2)
#define ENCL_VIDEO_HSO_BEGIN (0x1cb5 << 2)
#define ENCL_VIDEO_HSO_END (0x1cb6 << 2)
#define ENCL_VIDEO_VSO_BEGIN (0x1cb7 << 2)
#define ENCL_VIDEO_VSO_END (0x1cb8 << 2)
#define ENCL_VIDEO_VSO_BLINE (0x1cb9 << 2)
#define ENCL_VIDEO_VSO_ELINE (0x1cba << 2)
#define ENCL_VIDEO_MAX_LNCNT (0x1cbb << 2)
#define ENCL_VIDEO_MODE (0x1ca7 << 2)
#define ENCL_VIDEO_MODE_ADV (0x1ca8 << 2)
#define ENCL_VIDEO_RGBIN_CTRL (0x1cc7 << 2)
#define ENCL_VIDEO_EN (0x1ca0 << 2)

#define L_DITH_CNTL_ADDR (0x1408 << 2)
#define L_RGB_BASE_ADDR (0x1405 << 2)
#define L_RGB_COEFF_ADDR (0x1406 << 2)

#define VPP_MISC (0x1d26 << 2)
#define VPP_OUT_SATURATE (1 << 0)

// HHI_MIPI_CNTL0 Register Bit Def
#define MIPI_CNTL0_CMN_REF_GEN_CTRL(x) (x << 26)
#define MIPI_CNTL0_VREF_SEL(x) (x << 25)
#define VREF_SEL_VR 0
#define VREF_SEL_VBG 1
#define MIPI_CNTL0_LREF_SEL(x) (x << 24)
#define LREF_SEL_L_ROUT 0
#define LREF_SEL_LBG 1
#define MIPI_CNTL0_LBG_EN (1 << 23)
#define MIPI_CNTL0_VR_TRIM_CNTL(x) (x << 16)
#define MIPI_CNTL0_VR_GEN_FROM_LGB_EN (1 << 3)
#define MIPI_CNTL0_VR_GEN_BY_AVDD18_EN (1 << 2)

// HHI_MIPI_CNTL1 Register Bit Def
#define MIPI_CNTL1_DSI_VBG_EN (1 << 16)
#define MIPI_CNTL1_CTL (0x2e << 0)

// HHI_MIPI_CNTL2 Register Bit Def
#define MIPI_CNTL2_DEFAULT_VAL (0x2680fc5a)  // 4-lane DSI LINK

// clang-format on

namespace amlogic_display {

class HhiGclkMpeg2Reg : public hwreg::RegisterBase<HhiGclkMpeg2Reg, uint32_t> {
 public:
  // Undocumented
  DEF_BIT(4, clk81_en);

  static auto Get() { return hwreg::RegisterAddr<HhiGclkMpeg2Reg>(HHI_GCLK_MPEG2); }
};

// HHI_HDMI_PHY_CNTL0
class HhiHdmiPhyCntl0Reg : public hwreg::RegisterBase<HhiHdmiPhyCntl0Reg, uint32_t> {
 public:
  DEF_FIELD(31, 16, hdmi_ctl1);
  DEF_FIELD(15, 0, hdmi_ctl2);

  static auto Get() { return hwreg::RegisterAddr<HhiHdmiPhyCntl0Reg>(HHI_HDMI_PHY_CNTL0); }
};

// HHI_HDMI_PHY_CNTL1
class HhiHdmiPhyCntl1Reg : public hwreg::RegisterBase<HhiHdmiPhyCntl1Reg, uint32_t> {
 public:
  DEF_FIELD(31, 30, new_prbs_mode);
  DEF_FIELD(29, 28, new_prbs_prbsmode);
  DEF_BIT(27, new_prbs_sel);
  DEF_BIT(26, new_prbs_en);
  DEF_FIELD(25, 24, ch3_swap);  // 0: ch0, 1: ch1, 2: ch2, 3: ch3
  DEF_FIELD(23, 22, ch2_swap);  // 0: ch0, 1: ch1, 2: ch2, 3: ch3
  DEF_FIELD(21, 20, ch1_swap);  // 0: ch0, 1: ch1, 2: ch2, 3: ch3
  DEF_FIELD(19, 18, ch0_swap);  // 0: ch0, 1: ch1, 2: ch2, 3: ch3
  DEF_BIT(17, bit_invert);
  DEF_BIT(16, msb_lsb_swap);
  DEF_BIT(15, capture_add1);
  DEF_BIT(14, capture_clk_gate_en);
  DEF_BIT(13, hdmi_tx_prbs_en);
  DEF_BIT(12, hdmi_tx_prbs_err_en);
  DEF_FIELD(11, 8, hdmi_tx_sel_high);
  DEF_FIELD(7, 4, hdmi_tx_sel_low);
  DEF_BIT(3, hdmi_fifo_wr_enable);
  DEF_BIT(2, hdmi_fifo_enable);
  DEF_BIT(1, hdmi_tx_phy_clk_en);
  DEF_BIT(0, hdmi_tx_phy_soft_reset);

  static auto Get() { return hwreg::RegisterAddr<HhiHdmiPhyCntl1Reg>(HHI_HDMI_PHY_CNTL1); }
};

// HHI_HDMI_PHY_CNTL2
class HhiHdmiPhyCntl2Reg : public hwreg::RegisterBase<HhiHdmiPhyCntl2Reg, uint32_t> {
 public:
  DEF_BIT(8, test_error);
  DEF_FIELD(7, 0, hdmi_regrd);

  static auto Get() { return hwreg::RegisterAddr<HhiHdmiPhyCntl2Reg>(HHI_HDMI_PHY_CNTL2); }
};

// HHI_HDMI_PHY_CNTL3
class HhiHdmiPhyCntl3Reg : public hwreg::RegisterBase<HhiHdmiPhyCntl3Reg, uint32_t> {
 public:
  // All bits reserved

  static auto Get() { return hwreg::RegisterAddr<HhiHdmiPhyCntl3Reg>(HHI_HDMI_PHY_CNTL3); }
};

// HHI_HDMI_PHY_CNTL4
class HhiHdmiPhyCntl4Reg : public hwreg::RegisterBase<HhiHdmiPhyCntl4Reg, uint32_t> {
 public:
  DEF_FIELD(31, 24, new_prbs_err_thr);
  DEF_FIELD(21, 20, dtest_sel);
  DEF_BIT(19, new_prbs_clr_ber_meter);
  DEF_BIT(17, new_prbs_freez_ber);
  DEF_BIT(16, new_prbs_inverse_in);
  DEF_FIELD(15, 14, new_prbs_mode);

  static auto Get() { return hwreg::RegisterAddr<HhiHdmiPhyCntl4Reg>(HHI_HDMI_PHY_CNTL4); }
};

// HHI_HDMI_PHY_CNTL5
class HhiHdmiPhyCntl5Reg : public hwreg::RegisterBase<HhiHdmiPhyCntl5Reg, uint32_t> {
 public:
  DEF_FIELD(31, 24, new_pbrs_err_thr);
  DEF_FIELD(21, 20, dtest_sel);
  DEF_BIT(19, new_prbs_clr_ber_meter);
  DEF_BIT(17, new_prbs_freez_ber);
  DEF_BIT(16, new_prbs_inverse_in);
  DEF_FIELD(15, 14, new_prbs_mode);

  static auto Get() { return hwreg::RegisterAddr<HhiHdmiPhyCntl5Reg>(HHI_HDMI_PHY_CNTL5); }
};

// HHI_HDMI_PHY_STATUS
class HhiHdmiPhyStatusReg : public hwreg::RegisterBase<HhiHdmiPhyStatusReg, uint32_t> {
 public:
  DEF_BIT(29, prbs_enabled);
  DEF_BIT(28, test_error);
  DEF_BIT(24, new_prbs_pattern_nok);
  DEF_BIT(20, new_prbs_lock);
  DEF_FIELD(19, 0, new_prbs_ber_meter);

  static auto Get() { return hwreg::RegisterAddr<HhiHdmiPhyStatusReg>(HHI_HDMI_PHY_STATUS); }
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_HHI_REGS_H_
