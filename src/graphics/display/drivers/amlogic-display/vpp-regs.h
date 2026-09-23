// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VPP_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VPP_REGS_H_
#include <cstdint>

#include <hwreg/bitfields.h>

// VPP register is part of VPU register space
#define DOLBY_PATH_CTRL (0x1a0c << 2)
#define VPP_POSTBLEND_H_SIZE (0x1d21 << 2)
#define VPP_HOLD_LINES (0x1d22 << 2)
#define VPP_MISC (0x1d26 << 2)
#define VPP_OFIFO_SIZE (0x1d27 << 2)
#define VPP_MATRIX_CTRL (0x1d5f << 2)

#define VPP_OSD1_IN_SIZE (0x1df1 << 2)
#define VPP_OSD1_BLD_H_SCOPE (0x1df5 << 2)
#define VPP_OSD1_BLD_V_SCOPE (0x1df6 << 2)
#define VPP_OSD2_BLD_H_SCOPE (0x1df7 << 2)
#define VPP_OSD2_BLD_V_SCOPE (0x1df8 << 2)

#define OSD1_BLEND_SRC_CTRL (0x1dfd << 2)
#define OSD2_BLEND_SRC_CTRL (0x1dfe << 2)

#define VPP_POST2_MATRIX_EN_CTRL (0x39ad << 2)

#define VIU_OSD_BLEND_CTRL (0x39b0 << 2)
#define VIU_OSD_BLEND_DUMMY_DATA0 (0x39b9 << 2)
#define VIU_OSD_BLEND_DUMMY_ALPHA (0x39ba << 2)
#define VIU_OSD_BLEND_BLEND0_SIZE (0x39bb << 2)
#define VIU_OSD_BLEND_BLEND1_SIZE (0x39bc << 2)

#define VPP_WRAP_OSD1_MATRIX_COEF00_01 (0x3d60 << 2)
#define VPP_WRAP_OSD1_MATRIX_COEF02_10 (0x3d61 << 2)
#define VPP_WRAP_OSD1_MATRIX_COEF11_12 (0x3d62 << 2)
#define VPP_WRAP_OSD1_MATRIX_COEF20_21 (0x3d63 << 2)
#define VPP_WRAP_OSD1_MATRIX_COEF22 (0x3d64 << 2)
#define VPP_WRAP_OSD1_MATRIX_COEF13_14 (0x3d65 << 2)
#define VPP_WRAP_OSD1_MATRIX_COEF23_24 (0x3d66 << 2)
#define VPP_WRAP_OSD1_MATRIX_COEF15_25 (0x3d67 << 2)
#define VPP_WRAP_OSD1_MATRIX_CLIP (0x3d68 << 2)
#define VPP_WRAP_OSD1_MATRIX_OFFSET0_1 (0x3d69 << 2)
#define VPP_WRAP_OSD1_MATRIX_OFFSET2 (0x3d6a << 2)
#define VPP_WRAP_OSD1_MATRIX_PRE_OFFSET0_1 (0x3d6b << 2)
#define VPP_WRAP_OSD1_MATRIX_PRE_OFFSET2 (0x3d6c << 2)
#define VPP_WRAP_OSD1_MATRIX_EN_CTRL (0x3d6d << 2)

#define VPP_OSD2_PREBLEND (1 << 17)
#define VPP_OSD1_PREBLEND (1 << 16)
#define VPP_VD2_PREBLEND (1 << 15)
#define VPP_VD1_PREBLEND (1 << 14)
#define VPP_OSD2_POSTBLEND (1 << 13)
#define VPP_OSD1_POSTBLEND (1 << 12)
#define VPP_VD2_POSTBLEND (1 << 11)
#define VPP_VD1_POSTBLEND (1 << 10)
#define VPP_POSTBLEND_EN (1 << 7)
#define VPP_PRE_FG_OSD2 (1 << 5)
#define VPP_PREBLEND_EN (1 << 6)
#define VPP_POST_FG_OSD2 (1 << 4)

#define VPP_DUMMY_DATA (0x1d00 << 2)
#define VPP_DUMMY_DATA1 (0x1d69 << 2)
#define VPP_OSD_SC_DUMMY_DATA (0x1dc7 << 2)

// Gamma Registers
#define VPP_GAMMA_CNTL_PORT (0x1400 << 2)
#define VPP_GAMMA_DATA_PORT (0x1401 << 2)
#define VPP_GAMMA_ADDR_PORT (0x1402 << 2)

// RGB Clamp Register
#define VPP_CLIP_MISC1 (0x1dda << 2)

namespace amlogic_display {

class VppClipMisc1Reg : public hwreg::RegisterBase<VppClipMisc1Reg, uint32_t> {
 public:
  DEF_FIELD(29, 20, r_clamp);
  DEF_FIELD(19, 10, g_clamp);
  DEF_FIELD(9, 0, b_clamp);
  static auto Get() { return hwreg::RegisterAddr<VppClipMisc1Reg>(VPP_CLIP_MISC1); }
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VPP_REGS_H_
