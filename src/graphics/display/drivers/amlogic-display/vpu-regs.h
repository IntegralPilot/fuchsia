// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// See S905D2G datasheet v0.8, section 8.2.2 beginning at page 345

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VPU_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VPU_REGS_H_

#include <cstdint>

#include <hwreg/bitfields.h>

#define VPU_VIU_MISC_CTRL0 (0x1a06 << 2)
#define VPU_VIU_OSD1_CTRL_STAT (0x1a10 << 2)
#define VPU_VIU_OSD1_COLOR_ADDR (0x1a11 << 2)
#define VPU_VIU_OSD1_COLOR (0x1a12 << 2)
#define VPU_VIU_OSD1_TCOLOR_AG0 (0x1a17 << 2)
#define VPU_VIU_OSD1_TCOLOR_AG1 (0x1a19 << 2)
#define VPU_VIU_OSD1_TCOLOR_AG2 (0x1a19 << 2)
#define VPU_VIU_OSD1_TCOLOR_AG3 (0x1a1a << 2)
#define VPU_VIU_OSD1_CTRL_STAT2 (0x1a2d << 2)
#define VPU_VIU_OSD1_BLK0_CFG_W0 (0x1a1b << 2)
#define VPU_VIU_OSD1_BLK0_CFG_W1 (0x1a1c << 2)
#define VPU_VIU_OSD1_BLK0_CFG_W2 (0x1a1d << 2)
#define VPU_VIU_OSD1_BLK0_CFG_W3 (0x1a1e << 2)
#define VPU_VIU_OSD1_BLK0_CFG_W4 (0x1a13 << 2)
#define VPU_VIU_OSD1_BLK1_CFG_W4 (0x1a14 << 2)
#define VPU_VIU_OSD1_BLK2_CFG_W4 (0x1a15 << 2)
#define VPU_VIU_OSD1_FIFO_CTRL_STAT (0x1a2b << 2)
#define VPU_VIU_OSD1_PROT_CTRL (0x1a2e << 2)
#define VPU_VIU_OSD1_MALI_UNPACK_CTRL (0x1a2f << 2)
#define VPU_VIU_OSD1_DIMM_CTRL (0x1adf << 2)
#define VPU_VIU_OSD2_CTRL_STAT (0x1a30 << 2)
#define VPU_VIU_OSD2_FIFO_CTRL_STAT (0x1a4b << 2)
#define VPU_VIU_OSD2_BLK0_CFG_W4 (0x1a64 << 2)
#define VPU_VIU_OSD_BLEND_CTRL (0x39b0 << 2)
#define VPU_VIU_OSD_BLEND_DIN0_SCOPE_H (0x39b1 << 2)
#define VPU_VIU_OSD_BLEND_DIN0_SCOPE_V (0x39b2 << 2)
#define VPU_VIU_OSD_BLEND_DIN1_SCOPE_H (0x39b3 << 2)
#define VPU_VIU_OSD_BLEND_DIN1_SCOPE_V (0x39b4 << 2)
#define VPU_VIU_OSD_BLEND_DIN2_SCOPE_H (0x39b5 << 2)
#define VPU_VIU_OSD_BLEND_DIN2_SCOPE_V (0x39b6 << 2)
#define VPU_VIU_OSD_BLEND_DIN3_SCOPE_H (0x39b7 << 2)
#define VPU_VIU_OSD_BLEND_DIN3_SCOPE_V (0x39b8 << 2)
#define VPU_VIU_OSD_BLEND_DUMMY_DATA0 (0x39b9 << 2)
#define VPU_VIU_OSD_BLEND_DUMMY_ALPHA (0x39ba << 2)
#define VPU_VIU_OSD_BLEND_BLEND0_SIZE (0x39bb << 2)
#define VPU_VIU_OSD_BLEND_BLEND1_SIZE (0x39bc << 2)
#define VPU_VPP_POSTBLEND_H_SIZE (0x1d21 << 2)
#define VPU_VPP_HOLD_LINES (0x1d22 << 2)
#define VPU_VPP_MISC (0x1d26 << 2)
#define VPU_VPP_OFIFO_SIZE (0x1d27 << 2)
#define VPU_VPP_OUT_H_V_SIZE (0x1da5 << 2)
#define VPU_VPP_OSD_VSC_PHASE_STEP (0x1dc0 << 2)
#define VPU_VPP_OSD_VSC_INI_PHASE (0x1dc1 << 2)
#define VPU_VPP_OSD_VSC_CTRL0 (0x1dc2 << 2)
#define VPU_VPP_OSD_HSC_CTRL0 (0x1dc5 << 2)
#define VPU_VPP_OSD_HSC_PHASE_STEP (0x1dc3 << 2)
#define VPU_VPP_OSD_HSC_INI_PHASE (0x1dc4 << 2)
#define VPU_VPP_OSD_SC_DUMMY_DATA (0x1dc7 << 2)
#define VPU_VPP_OSD_SC_CTRL0 (0x1dc8 << 2)
#define VPU_VPP_OSD_SCI_WH_M1 (0x1dc9 << 2)
#define VPU_VPP_OSD_SCO_H_START_END (0x1dca << 2)
#define VPU_VPP_OSD_SCO_V_START_END (0x1dcb << 2)
#define VPU_VPP_OSD_SCALE_COEF_IDX (0x1dcc << 2)
#define VPU_VPP_OSD_SCALE_COEF (0x1dcd << 2)
#define VPU_VPP_OSD1_IN_SIZE (0x1df1 << 2)
#define VPU_VPP_OSD1_BLD_H_SCOPE (0x1df5 << 2)
#define VPU_VPP_OSD1_BLD_V_SCOPE (0x1df6 << 2)
#define VPU_VPP_OSD2_BLD_H_SCOPE (0x1df7 << 2)
#define VPU_VPP_OSD2_BLD_V_SCOPE (0x1df8 << 2)
#define VPU_OSD_PATH_MISC_CTRL (0x1a0e << 2)
#define VPU_OSD1_BLEND_SRC_CTRL (0x1dfd << 2)
#define VPU_OSD2_BLEND_SRC_CTRL (0x1dfe << 2)
#define VPU_RDARB_MODE_L1C1 (0x2790 << 2)
#define VPU_RDARB_MODE_L1C2 (0x2799 << 2)
#define VPU_RDARB_MODE_L2C1 (0x279d << 2)
#define VPU_WRARB_MODE_L2C1 (0x27a2 << 2)

#define VPU_ENCP_VIDEO_EN (0x1b80 << 2)
#define VPU_ENCI_VIDEO_EN (0x1b57 << 2)
#define VPU_ENCP_VIDEO_MODE (0x1b8d << 2)
#define VPU_ENCP_VIDEO_MODE_ADV (0x1b8e << 2)
#define VPU_ENCP_VIDEO_MAX_PXCNT (0x1b97 << 2)
#define VPU_ENCP_VIDEO_HAVON_END (0x1ba3 << 2)
#define VPU_ENCP_VIDEO_HAVON_BEGIN (0x1ba4 << 2)
#define VPU_ENCP_VIDEO_VAVON_ELINE (0x1baf << 2)
#define VPU_ENCP_VIDEO_VAVON_BLINE (0x1ba6 << 2)
#define VPU_ENCP_VIDEO_HSO_BEGIN (0x1ba7 << 2)
#define VPU_ENCP_VIDEO_HSO_END (0x1ba8 << 2)
#define VPU_ENCP_VIDEO_VSO_BEGIN (0x1ba9 << 2)
#define VPU_ENCP_VIDEO_VSO_END (0x1baa << 2)
#define VPU_ENCP_VIDEO_VSO_BLINE (0x1bab << 2)
#define VPU_ENCP_VIDEO_VSO_ELINE (0x1bac << 2)
#define VPU_ENCP_VIDEO_MAX_LNCNT (0x1bae << 2)
#define VPU_ENCP_DVI_HSO_BEGIN (0x1c30 << 2)
#define VPU_ENCP_DVI_HSO_END (0x1c31 << 2)
#define VPU_ENCP_DVI_VSO_BLINE_EVN (0x1c32 << 2)
#define VPU_ENCP_DVI_VSO_ELINE_EVN (0x1c34 << 2)
#define VPU_ENCP_DVI_VSO_BEGIN_EVN (0x1c36 << 2)
#define VPU_ENCP_DVI_VSO_END_EVN (0x1c38 << 2)
#define VPU_ENCP_DE_H_BEGIN (0x1c3a << 2)
#define VPU_ENCP_DE_H_END (0x1c3b << 2)
#define VPU_ENCP_DE_V_BEGIN_EVEN (0x1c3c << 2)
#define VPU_ENCP_DE_V_END_EVEN (0x1c3d << 2)
#define VPU_HDMI_SETTING (0x271b << 2)
#define VPU_HDMI_FMT_CTRL (0x2743 << 2)
#define VPU_HDMI_DITH_CNTL (0x27fc << 2)

#define VPU_VPP_POST_MATRIX_COEF00_01 (0x32b0 << 2)
#define VPU_VPP_POST_MATRIX_COEF02_10 (0x32b1 << 2)
#define VPU_VPP_POST_MATRIX_COEF11_12 (0x32b2 << 2)
#define VPU_VPP_POST_MATRIX_COEF20_21 (0x32b3 << 2)
#define VPU_VPP_POST_MATRIX_COEF22 (0x32b4 << 2)
#define VPU_VPP_POST_MATRIX_OFFSET0_1 (0x32b9 << 2)
#define VPU_VPP_POST_MATRIX_OFFSET2 (0x32ba << 2)
#define VPU_VPP_POST_MATRIX_PRE_OFFSET0_1 (0x32bb << 2)
#define VPU_VPP_POST_MATRIX_PRE_OFFSET2 (0x32bc << 2)
#define VPU_VPP_POST_MATRIX_EN_CTRL (0x32bd << 2)

// Registers needed for Video Loopback mode
#define VPU_WR_BACK_MISC_CTRL (0x1a0d << 2)
#define VPU_WRBACK_CTRL (0x1df9 << 2)

#define VPU_VDIN1_MATRIX_CTRL (0x1310 << 2)
#define VPU_VDIN1_COEF00_01 (0x1311 << 2)
#define VPU_VDIN1_COEF02_10 (0x1312 << 2)
#define VPU_VDIN1_COEF11_12 (0x1313 << 2)
#define VPU_VDIN1_COEF20_21 (0x1314 << 2)
#define VPU_VDIN1_COEF22 (0x1315 << 2)
#define VPU_VDIN1_OFFSET0_1 (0x1316 << 2)
#define VPU_VDIN1_OFFSET2 (0x1317 << 2)
#define VPU_VDIN1_PRE_OFFSET0_1 (0x1318 << 2)
#define VPU_VDIN1_PRE_OFFSET2 (0x1319 << 2)

#define VPU_MAFBC_BLOCK_ID (0x3a00 << 2)
#define VPU_MAFBC_IRQ_RAW_STATUS (0x3a01 << 2)
#define VPU_MAFBC_IRQ_CLEAR (0x3a02 << 2)
#define VPU_MAFBC_IRQ_MASK (0x3a03 << 2)
#define VPU_MAFBC_IRQ_STATUS (0x3a04 << 2)
#define VPU_MAFBC_COMMAND (0x3a05 << 2)
#define VPU_MAFBC_STATUS (0x3a06 << 2)
#define VPU_MAFBC_SURFACE_CFG (0x3a07 << 2)
#define VPU_MAFBC_HEADER_BUF_ADDR_LOW_S0 (0x3a10 << 2)
#define VPU_MAFBC_HEADER_BUF_ADDR_HIGH_S0 (0x3a11 << 2)
#define VPU_MAFBC_FORMAT_SPECIFIER_S0 (0x3a12 << 2)
#define VPU_MAFBC_BUFFER_WIDTH_S0 (0x3a13 << 2)
#define VPU_MAFBC_BUFFER_HEIGHT_S0 (0x3a14 << 2)
#define VPU_MAFBC_BOUNDING_BOX_X_START_S0 (0x3a15 << 2)
#define VPU_MAFBC_BOUNDING_BOX_X_END_S0 (0x3a16 << 2)
#define VPU_MAFBC_BOUNDING_BOX_Y_START_S0 (0x3a17 << 2)
#define VPU_MAFBC_BOUNDING_BOX_Y_END_S0 (0x3a18 << 2)
#define VPU_MAFBC_OUTPUT_BUF_ADDR_LOW_S0 (0x3a19 << 2)
#define VPU_MAFBC_OUTPUT_BUF_ADDR_HIGH_S0 (0x3a1a << 2)
#define VPU_MAFBC_OUTPUT_BUF_STRIDE_S0 (0x3a1b << 2)
#define VPU_MAFBC_PREFETCH_CFG_S0 (0x3a1c << 2)

namespace amlogic_display {

template <class DerivedType, class IntType>
class RegisterBase : public hwreg::RegisterBase<DerivedType, IntType> {
 public:
  static auto Get(uint32_t addr) { return hwreg::RegisterAddr<DerivedType>(addr); }
};

class WrBackMiscCtrlReg : public hwreg::RegisterBase<WrBackMiscCtrlReg, uint32_t> {
 public:
  DEF_BIT(1, chan1_hsync_enable);
  DEF_BIT(0, chan0_hsync_enable);
  static auto Get() { return hwreg::RegisterAddr<WrBackMiscCtrlReg>(VPU_WR_BACK_MISC_CTRL); }
};

class WrBackCtrlReg : public hwreg::RegisterBase<WrBackCtrlReg, uint32_t> {
 public:
  DEF_FIELD(2, 0, chan0_sel);
  static auto Get() { return hwreg::RegisterAddr<WrBackCtrlReg>(VPU_WRBACK_CTRL); }
};

class VdInMatrixCtrlReg : public hwreg::RegisterBase<VdInMatrixCtrlReg, uint32_t> {
 public:
  DEF_FIELD(3, 2, select);
  DEF_BIT(1, enable);
  static auto Get() { return hwreg::RegisterAddr<VdInMatrixCtrlReg>(VPU_VDIN1_MATRIX_CTRL); }
};

class VdinCoef00_01Reg : public hwreg::RegisterBase<VdinCoef00_01Reg, uint32_t> {
 public:
  DEF_FIELD(28, 16, coef00);
  DEF_FIELD(12, 0, coef01);
  static auto Get() { return hwreg::RegisterAddr<VdinCoef00_01Reg>(VPU_VDIN1_COEF00_01); }
};

class VdinCoef02_10Reg : public hwreg::RegisterBase<VdinCoef02_10Reg, uint32_t> {
 public:
  DEF_FIELD(28, 16, coef02);
  DEF_FIELD(12, 0, coef10);
  static auto Get() { return hwreg::RegisterAddr<VdinCoef02_10Reg>(VPU_VDIN1_COEF02_10); }
};

class VdinCoef11_12Reg : public hwreg::RegisterBase<VdinCoef11_12Reg, uint32_t> {
 public:
  DEF_FIELD(28, 16, coef11);
  DEF_FIELD(12, 0, coef12);
  static auto Get() { return hwreg::RegisterAddr<VdinCoef11_12Reg>(VPU_VDIN1_COEF11_12); }
};

class VdinCoef20_21Reg : public hwreg::RegisterBase<VdinCoef20_21Reg, uint32_t> {
 public:
  DEF_FIELD(28, 16, coef20);
  DEF_FIELD(12, 0, coef21);
  static auto Get() { return hwreg::RegisterAddr<VdinCoef20_21Reg>(VPU_VDIN1_COEF20_21); }
};

class VdinCoef22Reg : public hwreg::RegisterBase<VdinCoef22Reg, uint32_t> {
 public:
  DEF_FIELD(12, 0, coef22);
  static auto Get() { return hwreg::RegisterAddr<VdinCoef22Reg>(VPU_VDIN1_COEF22); }
};

// VDIN1_MATRIX_OFFSET0_1
// A311D datasheet, section 10.2.3 "Video Output" > "Register Description",
// page 1113.
class VdinOffset0_1Reg : public hwreg::RegisterBase<VdinOffset0_1Reg, uint32_t> {
 public:
  DEF_FIELD(26, 16, offset0);
  DEF_FIELD(10, 0, offset1);
  static auto Get() { return hwreg::RegisterAddr<VdinOffset0_1Reg>(VPU_VDIN1_OFFSET0_1); }
};

// VDIN1_MATRIX_OFFSET2
// A311D datasheet, section 10.2.3 "Video Output" > "Register Description",
// page 1113.
class VdinOffset2Reg : public hwreg::RegisterBase<VdinOffset2Reg, uint32_t> {
 public:
  DEF_FIELD(10, 0, offset2);
  static auto Get() { return hwreg::RegisterAddr<VdinOffset2Reg>(VPU_VDIN1_OFFSET2); }
};

// VDIN1_MATRIX_PRE_OFFSET0_1
// A311D datasheet, section 10.2.3 "Video Output" > "Register Description",
// page 1114.
class VdinPreOffset0_1Reg : public hwreg::RegisterBase<VdinPreOffset0_1Reg, uint32_t> {
 public:
  DEF_FIELD(26, 16, preoffset0);
  DEF_FIELD(10, 0, preoffset1);
  static auto Get() { return hwreg::RegisterAddr<VdinPreOffset0_1Reg>(VPU_VDIN1_PRE_OFFSET0_1); }
};

// VDIN1_MATRIX_PRE_OFFSET2
// A311D datasheet, section 10.2.3 "Video Output" > "Register Description",
// page 1114.
class VdinPreOffset2Reg : public hwreg::RegisterBase<VdinPreOffset2Reg, uint32_t> {
 public:
  DEF_FIELD(10, 0, preoffset2);
  static auto Get() { return hwreg::RegisterAddr<VdinPreOffset2Reg>(VPU_VDIN1_PRE_OFFSET2); }
};

class AfbcCommandReg : public RegisterBase<AfbcCommandReg, uint32_t> {
 public:
  DEF_BIT(1, pending_swap);
  DEF_BIT(0, direct_swap);
};

class AfbcSurfaceCfgReg : public RegisterBase<AfbcSurfaceCfgReg, uint32_t> {
 public:
  DEF_BIT(16, cont);
  DEF_BIT(3, s3_en);
  DEF_BIT(2, s2_en);
  DEF_BIT(1, s1_en);
  DEF_BIT(0, s0_en);
};

class AfbcIrqMaskReg : public RegisterBase<AfbcIrqMaskReg, uint32_t> {
 public:
  DEF_BIT(3, detiling_error);
  DEF_BIT(2, decode_error);
  DEF_BIT(1, configuration_swapped);
  DEF_BIT(0, surfaces_completed);
};

class AfbcHeaderBufAddrLowS0Reg : public RegisterBase<AfbcHeaderBufAddrLowS0Reg, uint32_t> {
 public:
  DEF_FIELD(31, 0, header_buffer_addr);
};

class AfbcHeaderBufAddrHighS0Reg : public RegisterBase<AfbcHeaderBufAddrHighS0Reg, uint32_t> {
 public:
  DEF_FIELD(15, 0, header_buffer_addr);
};

class AfbcFormatSpecifierS0Reg : public RegisterBase<AfbcFormatSpecifierS0Reg, uint32_t> {
 public:
  DEF_BIT(19, payload_limit_en);
  DEF_BIT(18, tiled_header_enabled);
  DEF_FIELD(17, 16, super_block_aspect);
  DEF_BIT(9, block_split_mode_enabled);
  DEF_BIT(8, yuv_transform_enabled);
  DEF_FIELD(3, 0, pixel_format);
};

class AfbcBufferWidthS0Reg : public RegisterBase<AfbcBufferWidthS0Reg, uint32_t> {
 public:
  DEF_FIELD(13, 0, buffer_width);
};

class AfbcBufferHeightS0Reg : public RegisterBase<AfbcBufferHeightS0Reg, uint32_t> {
 public:
  DEF_FIELD(13, 0, buffer_height);
};

class AfbcBoundingBoxXStartS0Reg : public RegisterBase<AfbcBoundingBoxXStartS0Reg, uint32_t> {
 public:
  DEF_FIELD(12, 0, buffer_x_start);
};

class AfbcBoundingBoxXEndS0Reg : public RegisterBase<AfbcBoundingBoxXEndS0Reg, uint32_t> {
 public:
  DEF_FIELD(12, 0, buffer_x_end);
};

class AfbcBoundingBoxYStartS0Reg : public RegisterBase<AfbcBoundingBoxYStartS0Reg, uint32_t> {
 public:
  DEF_FIELD(12, 0, buffer_y_start);
};

class AfbcBoundingBoxYEndS0Reg : public RegisterBase<AfbcBoundingBoxYEndS0Reg, uint32_t> {
 public:
  DEF_FIELD(12, 0, buffer_y_end);
};

class AfbcOutputBufAddrLowS0Reg : public RegisterBase<AfbcOutputBufAddrLowS0Reg, uint32_t> {
 public:
  DEF_FIELD(31, 0, output_buffer_addr);
};

class AfbcOutputBufAddrHighS0Reg : public RegisterBase<AfbcOutputBufAddrHighS0Reg, uint32_t> {
 public:
  DEF_FIELD(15, 0, output_buffer_addr);
};

class AfbcOutputBufStrideS0Reg : public RegisterBase<AfbcOutputBufStrideS0Reg, uint32_t> {
 public:
  DEF_FIELD(15, 0, output_buffer_stride);
};

class AfbcPrefetchCfgS0Reg : public RegisterBase<AfbcPrefetchCfgS0Reg, uint32_t> {
 public:
  DEF_BIT(1, prefetch_read_y);
  DEF_BIT(0, prefetch_read_x);
};

class OsdMaliUnpackCtrlReg : public RegisterBase<OsdMaliUnpackCtrlReg, uint32_t> {
 public:
  DEF_BIT(31, mali_unpack_en);
  DEF_FIELD(15, 12, r);
  DEF_FIELD(11, 8, g);
  DEF_FIELD(7, 4, b);
  DEF_FIELD(3, 0, a);
};

class OsdBlk0CfgW0Reg : public RegisterBase<OsdBlk0CfgW0Reg, uint32_t> {
 public:
  DEF_BIT(30, mali_src_en);
  DEF_FIELD(23, 16, tbl_addr);
  DEF_BIT(15, little_endian);

  // TODO(https://fxbug.dev/42075769): Replace the uint32_t value with strong-typed enum
  // classes.
  static constexpr uint32_t kBlockMode32Bit = 5;
  DEF_FIELD(11, 8, blk_mode);
  DEF_BIT(6, alpha_en);

  // TODO(https://fxbug.dev/42075769): Replace the uint32_t values with strong-typed enum
  // classes.
  static constexpr uint32_t kColorMatrixArgb8888 = 1;
  static constexpr uint32_t kColorMatrixAbgr8888 = 2;
  DEF_FIELD(5, 2, color_matrix);
  DEF_BIT(1, interlace_en);
  DEF_BIT(0, interlace_sel_odd);
};

class OsdBlk0CfgW1Reg : public RegisterBase<OsdBlk0CfgW1Reg, uint32_t> {
 public:
  DEF_FIELD(28, 16, x_end);
  DEF_FIELD(12, 0, x_start);
};

class OsdBlk0CfgW2Reg : public RegisterBase<OsdBlk0CfgW2Reg, uint32_t> {
 public:
  DEF_FIELD(28, 16, y_end);
  DEF_FIELD(12, 0, y_start);
};

class OsdBlk0CfgW3Reg : public RegisterBase<OsdBlk0CfgW3Reg, uint32_t> {
 public:
  DEF_FIELD(27, 16, h_end);
  DEF_FIELD(11, 0, h_start);
};

class OsdBlk0CfgW4Reg : public RegisterBase<OsdBlk0CfgW4Reg, uint32_t> {
 public:
  DEF_FIELD(27, 16, v_end);
  DEF_FIELD(11, 0, v_start);
};

class OsdBlk1CfgW4Reg : public RegisterBase<OsdBlk1CfgW4Reg, uint32_t> {
 public:
  DEF_FIELD(31, 0, frame_addr);
};

class OsdBlk2CfgW4Reg : public RegisterBase<OsdBlk2CfgW4Reg, uint32_t> {
 public:
  // TRM shows 32 bits for linear_stride, but vendor code uses only the first 12
  DEF_FIELD(11, 0, linear_stride);
};

enum FifoWordsPerBurst {
  Burst1 = 0x0,
  Burst2 = 0x1,
  Burst4 = 0x2,
  BurstInvalid = 0x3,
};

class OsdFifoCtrlStatReg : public RegisterBase<OsdFifoCtrlStatReg, uint32_t> {
 public:
  DEF_BIT(31, burst_len_sel_hi_bit2);  // bit 2 of burst_len_sel
  DEF_BIT(30, byte_swap);              // 1`swap bytes, 0 do not swap
  DEF_BIT(29, div_swap);               // 1 swap 64-bit words in 128-bit data
  DEF_FIELD(28, 24, fifo_lim);         // when fifo.length() < fifo_lim*16, close OSD_RD_MIF
  DEF_ENUM_FIELD(FifoWordsPerBurst, 23, 22, fifo_ctrl);  // 00=1 word/burst, 01=2, 10=4, 11=invalid
  DEF_FIELD(21, 20, fifo_st);                            // read-only. 0=idle, 1=active, 2=aborting
  DEF_BIT(19, fifo_overflow);                            // read-only
  DEF_FIELD(18, 12, fifo_depth_val);                     // depth of fifo = val*8
  DEF_FIELD(11, 10, burst_len_sel_bits10);  // index into [24, 32, 48, 64, 96, 128] per burst
  DEF_FIELD(9, 5, hold_fifo_lines);         // lines after vsync to wait before requesting data
  DEF_BIT(4, clear_err);
  DEF_BIT(3, fifo_sync_rst);
  DEF_FIELD(2, 1, endian);  // endianness of 64-bit data [none, 32-bit LE, 16-bit LE, 16-bit ME
  DEF_BIT(1, urgent);       // priority of data requests
};

class OsdCtrlStatReg : public RegisterBase<OsdCtrlStatReg, uint32_t> {
 public:
  DEF_BIT(21, osd_en);
  DEF_FIELD(20, 12, global_alpha);
  DEF_BIT(3, rsv);
  DEF_BIT(2, osd_mem_mode);
  DEF_BIT(1, premult_en);
  DEF_BIT(0, blk_en);
};

class OsdCtrlStat2Reg : public RegisterBase<OsdCtrlStat2Reg, uint32_t> {
 public:
  DEF_BIT(14, replaced_alpha_en);
  DEF_FIELD(13, 6, replaced_alpha);
  DEF_BIT(1, pending_status_cleanup);
};

class OsdTcolorAgReg : public RegisterBase<OsdTcolorAgReg, uint32_t> {
 public:
  DEF_FIELD(31, 24, y_or_r);
  DEF_FIELD(23, 16, cb_or_g);
  DEF_FIELD(15, 8, cr_or_b);
  DEF_FIELD(7, 0, a);
};

class OsdColorAddrReg : public RegisterBase<OsdColorAddrReg, uint32_t> {};

class OsdColorReg : public RegisterBase<OsdColorReg, uint32_t> {
 public:
  DEF_FIELD(31, 24, r);
  DEF_FIELD(23, 16, g);
  DEF_FIELD(15, 8, b);
  DEF_FIELD(7, 0, a);
};

class OsdProtCtrlReg : public RegisterBase<OsdProtCtrlReg, uint32_t> {
 public:
  DEF_FIELD(31, 16, urgent_ctrl);
  DEF_BIT(15, prot_en);
  DEF_FIELD(12, 0, prot_fifo_size);
};

class OsdDimmCtrlReg : public RegisterBase<OsdDimmCtrlReg, uint32_t> {
 public:
  DEF_BIT(30, en);
  DEF_FIELD(29, 0, rgb);
};

class OsdScaleCoefIdxReg : public RegisterBase<OsdScaleCoefIdxReg, uint32_t> {
 public:
  DEF_BIT(15, auto_incr_step);  // bit9 ? (value ? 2 : 1) :  2
  DEF_BIT(14, unused_cbus_readback);
  DEF_BIT(9, hi_res_coef);
  DEF_BIT(8, h_coef);  // if 0, vertical coefficient
  DEF_FIELD(6, 0, index);
};

class OsdScaleCoefReg : public RegisterBase<OsdScaleCoefReg, uint32_t> {};

class OsdVscPhaseStepReg : public RegisterBase<OsdVscPhaseStepReg, uint32_t> {
  DEF_FIELD(27, 0, phase_4_24);
};

class OsdVscInitPhaseReg : public RegisterBase<OsdVscInitPhaseReg, uint32_t> {
  DEF_FIELD(31, 16, bottom_vscale_phase);
  DEF_FIELD(15, 0, top_vscale_phase);
};

class OsdVscCtrl0Reg : public RegisterBase<OsdVscCtrl0Reg, uint32_t> {
 public:
  DEF_BIT(24, en);
  DEF_BIT(23, interlace);
  DEF_FIELD(22, 21, double_line_mode);
  DEF_BIT(20, phase0_always_en);
  DEF_BIT(19, nearest_en);
  DEF_FIELD(17, 16, bot_rpt_l0_num);
  DEF_FIELD(14, 11, bot_ini_rcv_num);
  DEF_FIELD(9, 8, top_rpt_l0_num);
  DEF_FIELD(6, 3, top_ini_rcv_num);
  DEF_FIELD(2, 0, bank_length);
};

class OsdHscPhaseStepReg : public RegisterBase<OsdHscPhaseStepReg, uint32_t> {
 public:
  DEF_FIELD(27, 0, phase_4_24);
};

class OsdHscInitPhaseReg : public RegisterBase<OsdHscInitPhaseReg, uint32_t> {
 public:
  DEF_FIELD(31, 16, init_phase1);
  DEF_FIELD(15, 0, init_phase0);
};

class OsdHscCtrl0Reg : public RegisterBase<OsdHscCtrl0Reg, uint32_t> {
 public:
  DEF_BIT(22, hsc_en);
  DEF_BIT(21, double_pix_mode);
  DEF_BIT(20, phase0_always_en);
  DEF_BIT(19, vsc_nearest_en);
  DEF_FIELD(17, 16, hsc_rpt_p0_num1);
  DEF_FIELD(14, 11, hsc_ini_rcv_num1);
  DEF_FIELD(9, 8, hsc_rpt_p0_num0);
  DEF_FIELD(6, 3, hsc_ini_rcv_num0);
  DEF_FIELD(2, 0, hsc_bank_length);
};

class OsdScDummyDataReg : public RegisterBase<OsdScDummyDataReg, uint32_t> {
 public:
  DEF_FIELD(31, 24, c0);  // >>4
  DEF_FIELD(23, 16, c1);
  DEF_FIELD(15, 8, c2);
  DEF_FIELD(7, 0, c3);  // alpha
};

class OsdScCtrl0Reg : public RegisterBase<OsdScCtrl0Reg, uint32_t> {
 public:
  DEF_FIELD(27, 16, gclk_ctrl);
  DEF_BIT(13, din_osd_alpha_mode);
  DEF_BIT(12, dout_alpha_mode);
  DEF_FIELD(11, 4, alpha);
  DEF_BIT(3, path_en);
  DEF_BIT(2, en);
};

class OsdSciWhM1Reg : public RegisterBase<OsdSciWhM1Reg, uint32_t> {
 public:
  DEF_FIELD(28, 16, width_minus_1);
  DEF_FIELD(12, 0, height_minus_1);
};

class OsdScoHStartEndReg : public RegisterBase<OsdScoHStartEndReg, uint32_t> {
 public:
  DEF_FIELD(27, 16, h_start);
  DEF_FIELD(11, 0, h_end);
};

class OsdScoVStartEndReg : public RegisterBase<OsdScoVStartEndReg, uint32_t> {
 public:
  DEF_FIELD(27, 16, v_start);
  DEF_FIELD(11, 0, v_end);
};

class Osd2CtrlStatReg : public hwreg::RegisterBase<Osd2CtrlStatReg, uint32_t> {
 public:
  DEF_BIT(21, osd_en);
  DEF_FIELD(20, 12, global_alpha);
  DEF_BIT(2, osd_mem_mode);
  DEF_BIT(1, premult_en);
  DEF_BIT(0, blk_en);
  static auto Get() { return hwreg::RegisterAddr<Osd2CtrlStatReg>(VPU_VIU_OSD2_CTRL_STAT); }
};

class OsdPathMiscCtrlReg : public RegisterBase<OsdPathMiscCtrlReg, uint32_t> {
 public:
  DEF_BIT(4, osd1_mali_sel);
};

// VPU_VIU_VENC_MUX_CTRL
//
// Configures the routing between VIU (Video Input Unit) and encoders. Each VIU
// outputs data (image pixel component values) that feeds into encoders. Each
// encoder outputs a Vsync signal that drives VIU operation. The data and Vsync
// signals are configured together, with an exception that lets the hardware
// ensure that the VIU blocks operate in sync.
//
// S905D3 Datasheet, Section 8.2.3.1 "VPU Registers", Page 304.
// A311D Datasheet, Section 10.2.3.1 "VPU Registers", Page 668.
// S905D2 Datasheet, Section 7.2.3.1 "VPU Registers", Page 312-313.

class VideoInputUnitEncoderMuxControl
    : public hwreg::RegisterBase<VideoInputUnitEncoderMuxControl, uint32_t> {
 public:
  enum class Encoder : uint32_t {
    // ENCL (LVDS / LCD Encoder)
    kLcd = 0,
    // ENCI (Interlaced encoder)
    kInterlaced = 1,
    // ENCP (Progressive encoder)
    kProgressive = 2,
    // ENCT (TV Panel encoder)
    kTvPanel = 3,
  };

  static hwreg::RegisterAddr<VideoInputUnitEncoderMuxControl> Get() {
    return {0x271a * sizeof(uint32_t)};
  }

  // Bit 20 is defined differently in A311D / T931 / S905D2 and S905D3
  // documentations.
  //
  // On S905D3 datasheets, bit 20 is defined as `vsync_shared_by_viu_blocks`.
  // On all the other datasheets, bit 20 is reserved. Our experiments on
  // VIM3 (Amlogic A311D), Astro (Amlogic S905D2), Sherlock (Amlogic T931) and
  // Nelson (Amlogic S905D3) show that the S905D3 description applies to the
  // other chips.

  // If true, the hardware ensures that the VIU blocks operate in sync.
  //
  // The hardware keeps the VIU blocks in sync by routing the same Vsync signal
  // to them. Concretely, the Vsync signal from the encoder configured by
  // `viu2_encoder_selection` is also routed to VIU1, so Vsync routing ignores
  // `viu1_vsync_encoder_selection`.
  DEF_BIT(20, vsync_shared_by_viu_blocks);

  // Undocumented field. zero after reset.
  DEF_FIELD(17, 16, rasp_dpi_clock_selection);

  // Bits 4-11 are defined differently in A311D / T931 / S905D2 and
  // S905D3 documentations.
  //
  // On A311D / T931 / S905D2 datasheets, bits 11-8 selects the encoder
  // (ENCI/ENCP/ENCT/ENCL) of which data is sent to video input channel VDI6,
  // and bits 7-4 selects the encoder of which clock signal is sent to VDI6.
  //
  // On S905D3 datasheets, bits 11-6 are reserved. Bits 5-4 are defined as
  // `viu1_vsync_encoder_selection` selecting encoder for vsync signal
  // connection with VIU1.
  //
  // Experiments on VIM3 (Amlogic A311D), Astro (Amlogic S905D2), Sherlock
  // (Amlogic T931) and Nelson (Amlogic S905D3) show that the VDI6 mux fields
  // (bits 11-8 and bits 7-4) have been replaced by register `WritebackMuxControl`
  // and the the register layout is the same as that in S905D3, so we only keep
  // the S905D3 register layout in our code.

  // The encoder whose Vsync is connected to VIU1.
  //
  // Only used when `vsync_shared_by_viu_blocks` is true.
  DEF_ENUM_FIELD(Encoder, 5, 4, viu1_vsync_encoder_selection);

  // The encoder connected to VIU2.
  //
  // Both the encoder's Vsync control signal and data signals will be connected
  // to VIU2.
  DEF_ENUM_FIELD(Encoder, 3, 2, viu2_encoder_selection);

  // The encoder whose data signals are connected to VIU1.
  //
  // If `vsync_shared_by_viu_blocks` is false, the same encoder's Vsync signal
  // will also be encoded to VIU1.
  DEF_ENUM_FIELD(Encoder, 1, 0, viu1_encoder_selection);
};

class VpuHdmiFmtCtrlReg : public hwreg::RegisterBase<VpuHdmiFmtCtrlReg, uint32_t> {
 public:
  DEF_FIELD(21, 19, frame_count_offset_for_B);
  DEF_FIELD(18, 16, frame_count_offset_for_G);
  DEF_BIT(15, hcnt_hold_when_de_valid);
  DEF_BIT(14, RGB_frame_count_separate);
  DEF_BIT(13, dith4x4_frame_random_enable);
  DEF_BIT(12, dith4x4_enable);
  DEF_BIT(11, tunnel_enable_for_dolby);
  DEF_BIT(10, rounding_enable);
  DEF_FIELD(9, 6, cntl_hdmi_dith10);
  DEF_BIT(5, cntl_hdmi_dith_md);
  DEF_BIT(4, cntl_hdmi_dith_en);
  DEF_FIELD(3, 2, cntl_chroma_dnsmp);  // 0: use pixel 0
                                       // 1: use pixel 1
                                       // 2: use average
  DEF_FIELD(1, 0, cntl_hdmi_vid_fmt);  // 0: no conversion
                                       // 1: convert to 422
                                       // 2: convert to 420

  static auto Get() { return hwreg::RegisterAddr<VpuHdmiFmtCtrlReg>(VPU_HDMI_FMT_CTRL); }
};

class VpuHdmiDithCntlReg : public hwreg::RegisterBase<VpuHdmiDithCntlReg, uint32_t> {
 public:
  DEF_FIELD(21, 19, frame_count_offset_for_B);
  DEF_FIELD(18, 16, frame_count_offset_for_G);
  DEF_BIT(15, hcnt_hold_when_de_valid);
  DEF_BIT(14, RGB_frame_count_separate);
  DEF_BIT(13, dith4x4_frame_random_enable);
  DEF_BIT(12, dith4x4_enable);
  DEF_BIT(11, tunnel_enable_for_dolby);
  DEF_BIT(10, rounding_enable);
  DEF_FIELD(9, 6, cntl_hdmi_dith10);
  DEF_BIT(5, cntl_hdmi_dith_md);
  DEF_BIT(4, cntl_hdmi_dith_en);
  DEF_BIT(3, hsync_invert);
  DEF_BIT(2, vsync_invert);
  DEF_BIT(0, dither_lut_sel);  // 1: sel 10b to 8b
                               // 0: sel 12b to 10b

  static auto Get() { return hwreg::RegisterAddr<VpuHdmiDithCntlReg>(VPU_HDMI_DITH_CNTL); }
};

class VpuHdmiSettingReg : public hwreg::RegisterBase<VpuHdmiSettingReg, uint32_t> {
 public:
  DEF_FIELD(15, 12, rd_rate);      // 0: One read every rd_clk
                                   // 1: One read every 2 rd_clk
                                   // 2: One read every 3 rd_clk
                                   // ...
                                   // 15: One read every 16 rd_clk
  DEF_FIELD(11, 8, wr_rate);       // 0: One write every wd_clk
                                   // 1: One write every 2 wd_clk
                                   // 2: One write every 3 wd_clk
                                   // ...
                                   // 15: One write every 16 wd_clk
  DEF_FIELD(7, 5, data_comp_map);  // 0: output CrYCb (BRG)
                                   // 1: output YCbCr (RGB)
                                   // 2: output YCrCb (RBG)
                                   // 3: output CbCrY (GBR)
                                   // 4: output CbYCr (GRB)
                                   // 5: output CrCbY (BGR)
  DEF_BIT(4, inv_dvi_clk);
  DEF_BIT(3, inv_vsync);
  DEF_BIT(2, inv_hsync);
  DEF_FIELD(1, 0, src_sel);  // 00: disable HDMI source
                             // 01: select ENCI data to HDMI
                             // 10: select ENCP data to HDMI

  static auto Get() { return hwreg::RegisterAddr<VpuHdmiSettingReg>(VPU_HDMI_SETTING); }
};

// Common register type for VD1/VD2, post, OSD, and OSD-wrap matrices.
class MatrixEnCtrlReg : public hwreg::RegisterBase<MatrixEnCtrlReg, uint32_t> {
 public:
  DEF_FIELD(5, 4, gate_clock_ctrl);
  DEF_BIT(1, enable_sync_sel);
  DEF_BIT(0, conv_en_pre);
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_VPU_REGS_H_
