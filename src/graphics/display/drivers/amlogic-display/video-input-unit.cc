// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/video-input-unit.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <algorithm>
#include <cmath>
#include <cstdint>
#include <memory>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/amlogic-display/board-resources.h"
#include "src/graphics/display/drivers/amlogic-display/common.h"
#include "src/graphics/display/drivers/amlogic-display/image-info.h"
#include "src/graphics/display/drivers/amlogic-display/pixel-grid-size2d.h"
#include "src/graphics/display/drivers/amlogic-display/rdma.h"
#include "src/graphics/display/drivers/amlogic-display/vpp-regs.h"
#include "src/graphics/display/drivers/amlogic-display/vpu-regs.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"

namespace amlogic_display {

namespace {
constexpr uint32_t kMaximumAlpha = 0xff;

// We use bicubic interpolation for scaling.
// TODO(payamm): Add support for other types of interpolation
unsigned int osd_filter_coefs_bicubic[] = {
    0x00800000, 0x007f0100, 0xff7f0200, 0xfe7f0300, 0xfd7e0500, 0xfc7e0600, 0xfb7d0800,
    0xfb7c0900, 0xfa7b0b00, 0xfa7a0dff, 0xf9790fff, 0xf97711ff, 0xf87613ff, 0xf87416fe,
    0xf87218fe, 0xf8701afe, 0xf76f1dfd, 0xf76d1ffd, 0xf76b21fd, 0xf76824fd, 0xf76627fc,
    0xf76429fc, 0xf7612cfc, 0xf75f2ffb, 0xf75d31fb, 0xf75a34fb, 0xf75837fa, 0xf7553afa,
    0xf8523cfa, 0xf8503ff9, 0xf84d42f9, 0xf84a45f9, 0xf84848f8};

constexpr uint32_t kFloatToFixed3_10ScaleFactor = 1024;
constexpr int32_t kMaxFloatToFixed3_10 = (4 * kFloatToFixed3_10ScaleFactor) - 1;
constexpr int32_t kMinFloatToFixed3_10 = -4 * kFloatToFixed3_10ScaleFactor;
constexpr uint32_t kFloatToFixed3_10Mask = 0x1FFF;

constexpr uint32_t kFloatToFixed2_10ScaleFactor = 1024;
constexpr int32_t kMaxFloatToFixed2_10 = (2 * kFloatToFixed2_10ScaleFactor) - 1;
constexpr int32_t kMinFloatToFixed2_10 = -2 * kFloatToFixed2_10ScaleFactor;
constexpr uint32_t kFloatToFixed2_10Mask = 0xFFF;

// AFBC related constants
constexpr uint32_t kAfbcb16x16Pixel = 0;
[[maybe_unused]] constexpr uint32_t kAfbc32x8Pixel = 1;
constexpr uint32_t kAfbcRGBA8888 = 5;
constexpr uint32_t kAfbcColorReorderR = 1;
constexpr uint32_t kAfbcColorReorderG = 2;
constexpr uint32_t kAfbcColorReorderB = 3;
constexpr uint32_t kAfbcColorReorderA = 4;

class OsdRegisters {
 public:
  hwreg::RegisterAddr<OsdCtrlStatReg> ctrl_stat;
  hwreg::RegisterAddr<OsdCtrlStat2Reg> ctrl_stat2;        /* VIU_OSD1_CTRL_STAT2 */
  hwreg::RegisterAddr<OsdColorAddrReg> color_addr;        /* VIU_OSD1_COLOR_ADDR */
  hwreg::RegisterAddr<OsdColorReg> color;                 /* VIU_OSD1_COLOR */
  hwreg::RegisterAddr<OsdTcolorAgReg> tcolor_ag0;         /* VIU_OSD1_TCOLOR_AG0 */
  hwreg::RegisterAddr<OsdTcolorAgReg> tcolor_ag1;         /* VIU_OSD1_TCOLOR_AG1 */
  hwreg::RegisterAddr<OsdTcolorAgReg> tcolor_ag2;         /* VIU_OSD1_TCOLOR_AG2 */
  hwreg::RegisterAddr<OsdTcolorAgReg> tcolor_ag3;         /* VIU_OSD1_TCOLOR_AG3 */
  hwreg::RegisterAddr<OsdBlk0CfgW0Reg> blk0_cfg_w0;       /* VIU_OSD1_BLK0_CFG_W0 */
  hwreg::RegisterAddr<OsdBlk0CfgW1Reg> blk0_cfg_w1;       /* VIU_OSD1_BLK0_CFG_W1 */
  hwreg::RegisterAddr<OsdBlk0CfgW2Reg> blk0_cfg_w2;       /* VIU_OSD1_BLK0_CFG_W2 */
  hwreg::RegisterAddr<OsdBlk0CfgW3Reg> blk0_cfg_w3;       /* VIU_OSD1_BLK0_CFG_W3 */
  hwreg::RegisterAddr<OsdBlk0CfgW4Reg> blk0_cfg_w4;       /* VIU_OSD1_BLK0_CFG_W4 */
  hwreg::RegisterAddr<OsdBlk1CfgW4Reg> blk1_cfg_w4;       /* VIU_OSD1_BLK1_CFG_W4 */
  hwreg::RegisterAddr<OsdBlk2CfgW4Reg> blk2_cfg_w4;       /* VIU_OSD1_BLK2_CFG_W4 */
  hwreg::RegisterAddr<OsdFifoCtrlStatReg> fifo_ctrl_stat; /* VIU_OSD1_FIFO_CTRL_STAT */
  // hwreg::RegisterAddr<OsdTestRdDataReg> test_rddata;/* VIU_OSD1_TEST_RDDATA */
  hwreg::RegisterAddr<OsdProtCtrlReg> prot_ctrl;              /* VIU_OSD1_PROT_CTRL */
  hwreg::RegisterAddr<OsdMaliUnpackCtrlReg> mali_unpack_ctrl; /* VIU_OSD1_MALI_UNPACK_CTRL */
  hwreg::RegisterAddr<OsdDimmCtrlReg> dimm_ctrl;              /* VIU_OSD1_DIMM_CTRL */

  hwreg::RegisterAddr<OsdScaleCoefIdxReg> scale_coef_idx;  /* VPP_OSD_SCALE_COEF_IDX */
  hwreg::RegisterAddr<OsdScaleCoefReg> scale_coef;         /* VPP_OSD_SCALE_COEF */
  hwreg::RegisterAddr<OsdVscPhaseStepReg> vsc_phase_step;  /* VPP_OSD_VSC_PHASE_STEP */
  hwreg::RegisterAddr<OsdVscInitPhaseReg> vsc_init_phase;  /* VPP_OSD_VSC_INI_PHASE */
  hwreg::RegisterAddr<OsdVscCtrl0Reg> vsc_ctrl0;           /* VPP_OSD_VSC_CTRL0 */
  hwreg::RegisterAddr<OsdHscPhaseStepReg> hsc_phase_step;  /* VPP_OSD_HSC_PHASE_STEP */
  hwreg::RegisterAddr<OsdHscInitPhaseReg> hsc_init_phase;  /* VPP_OSD_HSC_INI_PHASE */
  hwreg::RegisterAddr<OsdHscCtrl0Reg> hsc_ctrl0;           /* VPP_OSD_HSC_CTRL0 */
  hwreg::RegisterAddr<OsdScDummyDataReg> sc_dummy_data;    /* VPP_OSD_SC_DUMMY_DATA */
  hwreg::RegisterAddr<OsdScCtrl0Reg> sc_ctrl0;             /* VPP_OSD_SC_CTRL0 */
  hwreg::RegisterAddr<OsdSciWhM1Reg> sci_wh_m1;            /* VPP_OSD_SCI_WH_M1 */
  hwreg::RegisterAddr<OsdScoHStartEndReg> sco_h_start_end; /* VPP_OSD_SCO_H_START_END */
  hwreg::RegisterAddr<OsdScoVStartEndReg> sco_v_start_end; /* VPP_OSD_SCO_V_START_END */
  hwreg::RegisterAddr<AfbcHeaderBufAddrLowS0Reg>
      afbc_header_buf_addr_low_s; /* VPU_MAFBC_HEADER_BUF_ADDR_LOW_S0 */
  hwreg::RegisterAddr<AfbcHeaderBufAddrHighS0Reg>
      afbc_header_buf_addr_high_s; /* VPU_MAFBC_HEADER_BUF_ADDR_HIGH_S0 */
  hwreg::RegisterAddr<AfbcFormatSpecifierS0Reg>
      afbc_format_specifier_s;                                   /* VPU_MAFBC_FORMAT_SPECIFIER_S0 */
  hwreg::RegisterAddr<AfbcBufferWidthS0Reg> afbc_buffer_width_s; /* VPU_MAFBC_BUFFER_WIDTH_S0 */
  hwreg::RegisterAddr<AfbcBufferHeightS0Reg> afbc_buffer_height_s; /* VPU_MAFBC_BUFFER_HEIGHT_S0 */
  hwreg::RegisterAddr<AfbcBoundingBoxXStartS0Reg>
      afbc_bounding_box_x_start_s; /* VPU_MAFBC_BOUNDING_BOX_X_START_S0 */
  hwreg::RegisterAddr<AfbcBoundingBoxXEndS0Reg>
      afbc_bounding_box_x_end_s; /* VPU_MAFBC_BOUNDING_BOX_X_END_S0 */
  hwreg::RegisterAddr<AfbcBoundingBoxYStartS0Reg>
      afbc_bounding_box_y_start_s; /* VPU_MAFBC_BOUNDING_BOX_Y_START_S0 */
  hwreg::RegisterAddr<AfbcBoundingBoxYEndS0Reg>
      afbc_bounding_box_y_end_s; /* VPU_MAFBC_BOUNDING_BOX_Y_END_S0 */
  hwreg::RegisterAddr<AfbcOutputBufAddrLowS0Reg>
      afbc_output_buf_addr_low_s; /* VPU_MAFBC_OUTPUT_BUF_ADDR_LOW_S0 */
  hwreg::RegisterAddr<AfbcOutputBufAddrHighS0Reg>
      afbc_output_buf_addr_high_s; /* VPU_MAFBC_OUTPUT_BUF_ADDR_HIGH_S0 */
  hwreg::RegisterAddr<AfbcOutputBufStrideS0Reg>
      afbc_output_buf_stride_s; /* VPU_MAFBC_OUTPUT_BUF_STRIDE_S0 */
  hwreg::RegisterAddr<AfbcPrefetchCfgS0Reg> afbc_prefetch_cfg_s; /* VPU_MAFBC_PREFETCH_CFG_S0 */
};

OsdRegisters osd1_registers = {
    OsdCtrlStatReg::Get(VPU_VIU_OSD1_CTRL_STAT),
    OsdCtrlStat2Reg::Get(VPU_VIU_OSD1_CTRL_STAT2),
    OsdColorAddrReg::Get(VPU_VIU_OSD1_COLOR_ADDR),
    OsdColorReg::Get(VPU_VIU_OSD1_COLOR),
    OsdTcolorAgReg::Get(VPU_VIU_OSD1_TCOLOR_AG0),
    OsdTcolorAgReg::Get(VPU_VIU_OSD1_TCOLOR_AG1),
    OsdTcolorAgReg::Get(VPU_VIU_OSD1_TCOLOR_AG2),
    OsdTcolorAgReg::Get(VPU_VIU_OSD1_TCOLOR_AG3),
    OsdBlk0CfgW0Reg::Get(VPU_VIU_OSD1_BLK0_CFG_W0),
    OsdBlk0CfgW1Reg::Get(VPU_VIU_OSD1_BLK0_CFG_W1),
    OsdBlk0CfgW2Reg::Get(VPU_VIU_OSD1_BLK0_CFG_W2),
    OsdBlk0CfgW3Reg::Get(VPU_VIU_OSD1_BLK0_CFG_W3),
    OsdBlk0CfgW4Reg::Get(VPU_VIU_OSD1_BLK0_CFG_W4),
    OsdBlk1CfgW4Reg::Get(VPU_VIU_OSD1_BLK1_CFG_W4),
    OsdBlk2CfgW4Reg::Get(VPU_VIU_OSD1_BLK2_CFG_W4),
    OsdFifoCtrlStatReg::Get(VPU_VIU_OSD1_FIFO_CTRL_STAT),
    // OsdTestRdDataReg::Get(VPU_VIU_OSD1_TEST_RDDATA),
    OsdProtCtrlReg::Get(VPU_VIU_OSD1_PROT_CTRL),
    OsdMaliUnpackCtrlReg::Get(VPU_VIU_OSD1_MALI_UNPACK_CTRL),
    OsdDimmCtrlReg::Get(VPU_VIU_OSD1_DIMM_CTRL),

    OsdScaleCoefIdxReg::Get(VPU_VPP_OSD_SCALE_COEF_IDX),
    OsdScaleCoefReg::Get(VPU_VPP_OSD_SCALE_COEF),
    OsdVscPhaseStepReg::Get(VPU_VPP_OSD_VSC_PHASE_STEP),
    OsdVscInitPhaseReg::Get(VPU_VPP_OSD_VSC_INI_PHASE),
    OsdVscCtrl0Reg::Get(VPU_VPP_OSD_VSC_CTRL0),
    OsdHscPhaseStepReg::Get(VPU_VPP_OSD_HSC_PHASE_STEP),
    OsdHscInitPhaseReg::Get(VPU_VPP_OSD_HSC_INI_PHASE),
    OsdHscCtrl0Reg::Get(VPU_VPP_OSD_HSC_CTRL0),
    OsdScDummyDataReg::Get(VPU_VPP_OSD_SC_DUMMY_DATA),
    OsdScCtrl0Reg::Get(VPU_VPP_OSD_SC_CTRL0),
    OsdSciWhM1Reg::Get(VPU_VPP_OSD_SCI_WH_M1),
    OsdScoHStartEndReg::Get(VPU_VPP_OSD_SCO_H_START_END),
    OsdScoVStartEndReg::Get(VPU_VPP_OSD_SCO_V_START_END),
    AfbcHeaderBufAddrLowS0Reg::Get(VPU_MAFBC_HEADER_BUF_ADDR_LOW_S0),
    AfbcHeaderBufAddrHighS0Reg::Get(VPU_MAFBC_HEADER_BUF_ADDR_HIGH_S0),
    AfbcFormatSpecifierS0Reg::Get(VPU_MAFBC_FORMAT_SPECIFIER_S0),
    AfbcBufferWidthS0Reg::Get(VPU_MAFBC_BUFFER_WIDTH_S0),
    AfbcBufferHeightS0Reg::Get(VPU_MAFBC_BUFFER_HEIGHT_S0),
    AfbcBoundingBoxXStartS0Reg::Get(VPU_MAFBC_BOUNDING_BOX_X_START_S0),
    AfbcBoundingBoxXEndS0Reg::Get(VPU_MAFBC_BOUNDING_BOX_X_END_S0),
    AfbcBoundingBoxYStartS0Reg::Get(VPU_MAFBC_BOUNDING_BOX_Y_START_S0),
    AfbcBoundingBoxYEndS0Reg::Get(VPU_MAFBC_BOUNDING_BOX_Y_END_S0),
    AfbcOutputBufAddrLowS0Reg::Get(VPU_MAFBC_OUTPUT_BUF_ADDR_LOW_S0),
    AfbcOutputBufAddrHighS0Reg::Get(VPU_MAFBC_OUTPUT_BUF_ADDR_HIGH_S0),
    AfbcOutputBufStrideS0Reg::Get(VPU_MAFBC_OUTPUT_BUF_STRIDE_S0),
    AfbcPrefetchCfgS0Reg::Get(VPU_MAFBC_PREFETCH_CFG_S0),
};

}  // namespace

display::DriverConfigStamp VideoInputUnit::GetLastConfigStampApplied() {
  return rdma_->GetLastConfigStampApplied();
}

VideoInputUnit::VideoInputUnit(fdf::MmioBuffer vpu_mmio, std::unique_ptr<RdmaEngine> rdma)
    : vpu_mmio_(std::move(vpu_mmio)), rdma_(std::move(rdma)) {}

void VideoInputUnit::DisableLayer(display::DriverConfigStamp config_stamp) {
  rdma_->StopRdma();
  osd1_registers.ctrl_stat.ReadFrom(&vpu_mmio_).set_blk_en(0).WriteTo(&vpu_mmio_);
  rdma_->ResetConfigStamp(config_stamp);
}

void VideoInputUnit::EnableLayer() {
  osd1_registers.ctrl_stat.ReadFrom(&vpu_mmio_).set_blk_en(1).WriteTo(&vpu_mmio_);
}

uint32_t VideoInputUnit::FloatToFixed2_10(float f) {
  auto fixed_num = static_cast<int32_t>(round(f * kFloatToFixed2_10ScaleFactor));

  // Amlogic hardware accepts values [-2 2). Let's make sure the result is within this range.
  // If not, clamp it
  fixed_num = std::clamp(fixed_num, kMinFloatToFixed2_10, kMaxFloatToFixed2_10);
  return fixed_num & kFloatToFixed2_10Mask;
}

uint32_t VideoInputUnit::FloatToFixed3_10(float f) {
  auto fixed_num = static_cast<int32_t>(round(f * kFloatToFixed3_10ScaleFactor));

  // Amlogic hardware accepts values [-4 4). Let's make sure the result is within this range.
  // If not, clamp it
  fixed_num = std::clamp(fixed_num, kMinFloatToFixed3_10, kMaxFloatToFixed3_10);
  return fixed_num & kFloatToFixed3_10Mask;
}

void VideoInputUnit::SetColorCorrection(uint32_t rdma_table_idx,
                                        const display::ColorConversion& color_conversion) {
  if (color_conversion == display::ColorConversion::kIdentity) {
    // Disable color conversion engine
    rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_EN_CTRL,
                             vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_EN_CTRL) & ~(1 << 0));
    return;
  }

  // Set enable bit
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_EN_CTRL,
                           vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_EN_CTRL) | (1 << 0));

  // Load PreOffset values (or 0 if none entered)
  auto offset0_1 = (FloatToFixed2_10(color_conversion.preoffsets()[0]) << 16) |
                   (FloatToFixed2_10(color_conversion.preoffsets()[1]) << 0);
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_PRE_OFFSET0_1, offset0_1);
  auto offset2 = FloatToFixed2_10(color_conversion.preoffsets()[2]) << 0;
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_PRE_OFFSET2, offset2);
  // TODO(b/182481217): remove when this bug is closed.
  fdf::trace("pre offset0_1={} offset2={}", offset0_1, offset2);

  // Load PostOffset values (or 0 if none entered)
  offset0_1 = (FloatToFixed2_10(color_conversion.postoffsets()[0]) << 16) |
              (FloatToFixed2_10(color_conversion.postoffsets()[1]) << 0);
  offset2 = FloatToFixed2_10(color_conversion.postoffsets()[2]) << 0;
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_OFFSET0_1, offset0_1);
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_OFFSET2, offset2);
  // TODO(b/182481217): remove when this bug is closed.
  fdf::trace("post offset0_1={} offset2={}", offset0_1, offset2);

  const std::array<std::array<float, 3>, 3>& coefficients = color_conversion.coefficients();

  // Load up the coefficient matrix registers
  auto coef00_01 =
      (FloatToFixed3_10(coefficients[0][0]) << 16) | (FloatToFixed3_10(coefficients[0][1]) << 0);
  auto coef02_10 =
      (FloatToFixed3_10(coefficients[0][2]) << 16) | (FloatToFixed3_10(coefficients[1][0]) << 0);
  auto coef11_12 =
      (FloatToFixed3_10(coefficients[1][1]) << 16) | (FloatToFixed3_10(coefficients[1][2]) << 0);
  auto coef20_21 =
      (FloatToFixed3_10(coefficients[2][0]) << 16) | (FloatToFixed3_10(coefficients[2][1]) << 0);
  auto coef22 = (FloatToFixed3_10(coefficients[2][2]) << 0);
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_COEF00_01, coef00_01);
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_COEF02_10, coef02_10);
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_COEF11_12, coef11_12);
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_COEF20_21, coef20_21);
  rdma_->SetRdmaTableValue(rdma_table_idx, IDX_MATRIX_COEF22, coef22);
  // TODO(b/182481217): remove when this bug is closed.
  fdf::trace("color correction regs 00_01={:x}u 02_12={:x}u 11_12={:x}u 20_21={} 22={:x}u",
             coef00_01, coef02_10, coef11_12, coef20_21, coef22);
}

void VideoInputUnit::FlipOnVsync(const display::DriverLayer& layer,
                                 const display::Mode& display_mode,
                                 const display::ColorConversion& color_conversion,
                                 display::DriverConfigStamp config_stamp) {
  // TODO(https://fxbug.dev/401286733): color layers not yet supported.
  ZX_DEBUG_ASSERT_MSG(layer.image_source().width() != 0, "Solid color fill layers not supported");
  ZX_DEBUG_ASSERT_MSG(layer.image_source().height() != 0, "Solid color fill layers not supported");

  ImageInfo* info = reinterpret_cast<ImageInfo*>(layer.image_id().value());
  const int next_table_idx = rdma_->GetNextAvailableRdmaTableIndex();
  if (next_table_idx < 0) {
    fdf::error("No table available!");
    return;
  }

  fdf::trace("Table index {} used", next_table_idx);
  fdf::trace("AFBC {}", info->is_afbc ? "enabled" : "disabled");

  PixelGridSize2D display_contents_size = {.width = display_mode.active_area().width(),
                                           .height = display_mode.active_area().height()};

  // TODO(https://fxbug.dev/317922128): Use the (unscaled) layer source frame size.
  PixelGridSize2D layer_image_size = {.width = display_mode.active_area().width(),
                                      .height = display_mode.active_area().height()};

  if (ConfigNeededForSingleNonscaledLayer(layer_image_size, display_contents_size)) {
    fdf::info("Mode change ({} x {}) to ({} x {})", display_contents_size_.width,
              display_contents_size_.height, display_contents_size.width,
              display_contents_size.height);
    ConfigForSingleNonscaledLayer(layer_image_size, display_contents_size);
  }

  auto cfg_w0 = osd1_registers.blk0_cfg_w0.FromValue(0);
  if (info->is_afbc) {
    // AFBC: Enable sourcing from mali + configure as big endian
    cfg_w0.set_mali_src_en(1).set_little_endian(0);
  } else {
    uint8_t canvas_index = info->canvas_idx;
    // Update CFG_W0 with correct Canvas Index
    cfg_w0.set_mali_src_en(0).set_little_endian(1).set_tbl_addr(canvas_index);
  }
  cfg_w0.set_blk_mode(OsdBlk0CfgW0Reg::kBlockMode32Bit);

  switch (info->pixel_format.pixel_format) {
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
      cfg_w0.set_color_matrix(OsdBlk0CfgW0Reg::kColorMatrixAbgr8888);
      break;
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      cfg_w0.set_color_matrix(OsdBlk0CfgW0Reg::kColorMatrixArgb8888);
      break;
    default:
      // This should never happen. The image validity is guaranteed in
      // ImportImage() / CheckConfiguration().
      ZX_ASSERT_MSG(false, "Unsupported image format %u",
                    static_cast<uint32_t>(info->pixel_format.pixel_format));
      return;
  }

  rdma_->SetRdmaTableValue(next_table_idx, IDX_BLK0_CFG_W0, cfg_w0.reg_value());

  // Configure ctrl_stat and ctrl_stat2 registers
  auto osd_ctrl_stat_val = osd1_registers.ctrl_stat.ReadFrom(&vpu_mmio_);
  auto osd_ctrl_stat2_val = osd1_registers.ctrl_stat2.ReadFrom(&vpu_mmio_);

  // enable OSD Block
  osd_ctrl_stat_val.set_blk_en(1);

  // Amlogic supports two types of alpha blending:
  // Global: This alpha value is applied to the entire plane (i.e. all pixels)
  // Per-Pixel: Each pixel will be multiplied by its corresponding alpha channel
  //
  // If alpha blending is disabled by the client or we are supporting a format that does
  // not have an alpha channel, we need to:
  // a) Set global alpha multiplier to 1 (i.e. 0xFF)
  // b) Enable "replaced_alpha" and set its value to 0xFF. This will effectively
  //    tell the hardware to replace the value found in alpha channel with the "replaced"
  //    value
  //
  // If alpha blending is enabled but alpha_layer_val is NaN:
  // - Set global alpha multiplier to 1 (i.e. 0xFF)
  // - Disable "replaced_alpha" which allows hardware to use per-pixel alpha channel.
  //
  // If alpha blending is enabled and alpha_layer_val has a value:
  // - Set global alpha multiplier to alpha_layer_val
  // - Disable "replaced_alpha" which allows hardware to use per-pixel alpha channel.

  // Load default values: Set global alpha to 1 and enable replaced_alpha.
  osd_ctrl_stat2_val.set_replaced_alpha_en(1).set_replaced_alpha(kMaximumAlpha);
  osd_ctrl_stat_val.set_global_alpha(kMaximumAlpha);

  // This is guaranteed by DisplayEngine::CheckConfiguration().
  ZX_DEBUG_ASSERT(layer.image_metadata().dimensions().width() != 0);
  ZX_DEBUG_ASSERT(layer.image_metadata().dimensions().height() != 0);
  if (layer.alpha_mode() != display::AlphaMode::kDisable) {
    // If a global alpha value is provided, apply it.
    if (!isnan(layer.alpha_coefficient())) {
      auto num = static_cast<uint8_t>(round(layer.alpha_coefficient() * kMaximumAlpha));
      osd_ctrl_stat_val.set_global_alpha(num);
    }
  }

  // Use linear address for AFBC, Canvas otherwise
  osd_ctrl_stat_val.set_osd_mem_mode(info->is_afbc ? 1 : 0);
  osd_ctrl_stat2_val.set_pending_status_cleanup(1);

  rdma_->SetRdmaTableValue(next_table_idx, IDX_CTRL_STAT, osd_ctrl_stat_val.reg_value());
  rdma_->SetRdmaTableValue(next_table_idx, IDX_CTRL_STAT2, osd_ctrl_stat2_val.reg_value());

  if (info->is_afbc) {
    // Line Stride calculation based on vendor code
    auto a = fbl::round_up(fbl::round_up(info->image_width * 4, 16u) / 16, 2u);
    auto r = osd1_registers.blk2_cfg_w4.FromValue(0).set_linear_stride(a).reg_value();
    rdma_->SetRdmaTableValue(next_table_idx, IDX_BLK2_CFG_W4, r);

    // Set AFBC's Physical address since it does not use Canvas
    rdma_->SetRdmaTableValue(next_table_idx, IDX_AFBC_HEAD_BUF_ADDR_LOW,
                             (info->paddr & 0xFFFFFFFF));
    rdma_->SetRdmaTableValue(next_table_idx, IDX_AFBC_HEAD_BUF_ADDR_HIGH, (info->paddr >> 32));

    // Set OSD to unpack Mali source
    auto upackreg = osd1_registers.mali_unpack_ctrl.ReadFrom(&vpu_mmio_).set_mali_unpack_en(1);
    rdma_->SetRdmaTableValue(next_table_idx, IDX_MALI_UNPACK_CTRL, upackreg.reg_value());

    // Switch OSD to Mali Source
    auto miscctrl =
        OsdPathMiscCtrlReg::Get(VPU_OSD_PATH_MISC_CTRL).ReadFrom(&vpu_mmio_).set_osd1_mali_sel(1);
    rdma_->SetRdmaTableValue(next_table_idx, IDX_PATH_MISC_CTRL, miscctrl.reg_value());

    // S0 is our index of 0, which is programmed for OSD1
    rdma_->SetRdmaTableValue(next_table_idx, IDX_AFBC_SURFACE_CFG,
                             AfbcSurfaceCfgReg::Get(VPU_MAFBC_SURFACE_CFG)
                                 .ReadFrom(&vpu_mmio_)
                                 .set_cont(0)
                                 .set_s0_en(1)
                                 .reg_value());
    // set command - This uses a separate RDMA Table
    rdma_->SetAfbcRdmaTableValue(
        AfbcCommandReg::Get(VPU_MAFBC_COMMAND).FromValue(0).set_direct_swap(1).reg_value());
  } else {
    // Set OSD to unpack Normal source
    auto upackreg = osd1_registers.mali_unpack_ctrl.ReadFrom(&vpu_mmio_).set_mali_unpack_en(0);
    rdma_->SetRdmaTableValue(next_table_idx, IDX_MALI_UNPACK_CTRL, upackreg.reg_value());

    // Switch OSD to DDR Source
    auto miscctrl =
        OsdPathMiscCtrlReg::Get(VPU_OSD_PATH_MISC_CTRL).ReadFrom(&vpu_mmio_).set_osd1_mali_sel(0);
    rdma_->SetRdmaTableValue(next_table_idx, IDX_PATH_MISC_CTRL, miscctrl.reg_value());

    // Disable afbc sourcing
    rdma_->SetRdmaTableValue(next_table_idx, IDX_AFBC_SURFACE_CFG,
                             AfbcSurfaceCfgReg::Get(VPU_MAFBC_SURFACE_CFG)
                                 .ReadFrom(&vpu_mmio_)
                                 .set_s0_en(0)
                                 .reg_value());
    // clear command - This uses a separate RDMA Table
    rdma_->SetAfbcRdmaTableValue(
        AfbcCommandReg::Get(VPU_MAFBC_COMMAND).FromValue(0).set_direct_swap(0).reg_value());
  }

  SetColorCorrection(next_table_idx, color_conversion);

  // update last element of table which will be used to indicate whether RDMA operation was
  // completed or not
  rdma_->SetRdmaTableValue(next_table_idx, IDX_RDMA_CFG_STAMP_HIGH, (config_stamp.value() >> 32));
  rdma_->SetRdmaTableValue(next_table_idx, IDX_RDMA_CFG_STAMP_LOW,
                           (config_stamp.value() & 0xFFFFFFFF));

  rdma_->FlushRdmaTable(next_table_idx);
  if (info->is_afbc) {
    rdma_->FlushAfbcRdmaTable();
  }

  rdma_->ExecRdmaTable(next_table_idx, config_stamp, info->is_afbc);
}

void VideoInputUnit::ConfigOsdLayers(PixelGridSize2D layer_image_size,
                                     PixelGridSize2D display_contents_size) {
  // init osd fifo control and set DDR request priority to be urgent
  uint32_t reg_val = 1;
  reg_val |= 4 << 5;   // hold_fifo_lines
  reg_val |= 1 << 10;  // burst_len_sel 3 = 64. This bit is split between 10 and 31
  reg_val |= 2 << 22;
  reg_val |= 2 << 24;
  reg_val |= 1 << 31;
  reg_val |= 32 << 12;  // fifo_depth_val: 32*8 = 256
  vpu_mmio_.Write32(reg_val, VPU_VIU_OSD1_FIFO_CTRL_STAT);
  vpu_mmio_.Write32(reg_val, VPU_VIU_OSD2_FIFO_CTRL_STAT);

  osd1_registers.ctrl_stat.FromValue(0)
      .set_blk_en(1)
      .set_global_alpha(kMaximumAlpha)
      .set_osd_mem_mode(0)
      .set_premult_en(0)
      .set_osd_en(1)
      .WriteTo(&vpu_mmio_);

  // TODO: split this method into HwInit for each OSD.
  Osd2CtrlStatReg::Get()
      .FromValue(0)
      .set_blk_en(1)
      .set_global_alpha(kMaximumAlpha)
      .set_osd_mem_mode(0)
      .set_premult_en(0)
      .set_osd_en(1)
      .WriteTo(&vpu_mmio_);

  // Set range of the virtual canvas coordinates.
  vpu_mmio_.Write32(((layer_image_size.width - 1) & 0x1fff) << 16, VPU_VIU_OSD1_BLK0_CFG_W1);
  vpu_mmio_.Write32(((layer_image_size.height - 1) & 0x1fff) << 16, VPU_VIU_OSD1_BLK0_CFG_W2);

  // Set geometry to normal mode
  uint32_t data32 = ((display_contents_size.width - 1) & 0xfff) << 16;
  vpu_mmio_.Write32(data32, VPU_VIU_OSD1_BLK0_CFG_W3);
  data32 = ((display_contents_size.height - 1) & 0xfff) << 16;
  vpu_mmio_.Write32(data32, VPU_VIU_OSD1_BLK0_CFG_W4);
}

void VideoInputUnit::ConfigSingleLayerBlending(PixelGridSize2D layer_size,
                                               PixelGridSize2D display_contents_size) {
  // TODO(https://fxbug.dev/317961333): The documentation below needs to be
  // re-organized. Move the descriptions of the blenders and the blender muxes
  // to the blender register definitions; at this function we should only keep
  // how the muxes are wired to each other.

  // There are two types of blenders in the Amlogic display engine:
  // - OSD blenders;
  // - Video post-processor (VPP) blenders.
  //
  // The OSD blenders consists of three blenders (BLEND0, BLEND1 and BLEND2);
  // each blender takes up to 2 inputs where each input can be a layer or the
  // output of a previous blender.
  //
  // The output of OSD blenders are then passed on to the VPP blenders.
  // The VPP blenders set consists of a "pre-processing blender" (blend, then
  // perform post-processing) and a "post-processing blender" (perform post-
  // processing, then blend).
  //
  // Currently, for the single-layer configuration, we use the following
  // blender configurations:
  //
  //              +------------------------+-------------------+
  //              |       OSD blenders     |   VPP blenders    |
  //              |                        |                   |
  // OSD1 -scaler--> BLEND0 -----> BLEND2 ---->  Postblend   -----> Encoder
  //              |             /          |                   |
  //              |  BLEND1 ---/           |     Pre-blend     |
  //              |                        |     (bypassed)    |
  //              +------------------------+-------------------+
  //
  // BLEND0 takes the scaled layer as input, so it's input size is `layer_size`.
  // Since there's no other scaling occurring, the output size of all the OSD
  // blenders, and the input / output sizes of the VPP blenders are the same
  // as the `display_contents_size`.

  // TODO(https://fxbug.dev/42062952): Support multi-layer blender configurations.

  // osd blend ctrl
  vpu_mmio_.Write32(4 << 29 | 0 << 27 |  // blend2_premult_en
                        1 << 26 |        // blend_din0 input to blend0
                        0 << 25 |        // blend1_dout to blend2
                        0 << 24 |        // blend1_din3 input to blend1
                        1 << 20 |        // blend_din_en
                        0 << 16 |        // din_premult_en
                        1 << 0,          // din_reoder_sel = OSD1
                    VIU_OSD_BLEND_CTRL);

  // vpp osd1 blend ctrl
  vpu_mmio_.Write32((0 & 0xf) << 0 | (0 & 0x1) << 4 | (3 & 0xf) << 8 |  // postbld_src3_sel
                        (0 & 0x1) << 16 |                               // postbld_osd1_premult
                        (1 & 0x1) << 20,
                    OSD1_BLEND_SRC_CTRL);
  // vpp osd2 blend ctrl
  vpu_mmio_.Write32((0 & 0xf) << 0 | (0 & 0x1) << 4 | (0 & 0xf) << 8 |  // postbld_src4_sel
                        (0 & 0x1) << 16 |                               // postbld_osd2_premult
                        (1 & 0x1) << 20,
                    OSD2_BLEND_SRC_CTRL);

  // used default dummy data
  vpu_mmio_.Write32(0x0 << 16 | 0x0 << 8 | 0x0, VIU_OSD_BLEND_DUMMY_DATA0);
  // used default dummy alpha data
  vpu_mmio_.Write32(0x0 << 20 | 0x0 << 11 | 0x0, VIU_OSD_BLEND_DUMMY_ALPHA);

  // Size of the VIU OSD blender BLEND0 input.
  vpu_mmio_.Write32((layer_size.width - 1) << 16, VPU_VIU_OSD_BLEND_DIN0_SCOPE_H);
  vpu_mmio_.Write32((layer_size.height - 1) << 16, VPU_VIU_OSD_BLEND_DIN0_SCOPE_V);

  // Size of the VIU OSD blender BLEND0 output.
  vpu_mmio_.Write32(display_contents_size.height << 16 | display_contents_size.width,
                    VIU_OSD_BLEND_BLEND0_SIZE);

  // Size of the VIU OSD blender BLEND1 output.
  // TODO(https://fxbug.dev/42062952): This is not used when there's only one layer.
  vpu_mmio_.Write32(display_contents_size.height << 16 | display_contents_size.width,
                    VIU_OSD_BLEND_BLEND1_SIZE);

  vpu_mmio_.Write32(SetFieldValue32(vpu_mmio_.Read32(DOLBY_PATH_CTRL), /*field_begin_bit=*/2,
                                    /*field_size_bits=*/2, /*field_value=*/0x3),
                    DOLBY_PATH_CTRL);

  // Bypass VPP pre-processing blending ane only enable post-processing
  // blending.
  vpu_mmio_.Write32(vpu_mmio_.Read32(VPP_MISC) | VPP_POSTBLEND_EN, VPP_MISC);
  vpu_mmio_.Write32(vpu_mmio_.Read32(VPP_MISC) & ~(VPP_PREBLEND_EN), VPP_MISC);

  // "VPP OSD1" is the input of the VPP post-processing blender, which is the
  // output of the BLEND2 OSD blender.
  //
  // Input size of the post-processing blender.
  vpu_mmio_.Write32(display_contents_size.height << 16 | display_contents_size.width,
                    VPP_OSD1_IN_SIZE);

  // Scope of the VPP post-processing blender.
  vpu_mmio_.Write32(0 << 16 | (display_contents_size.width - 1), VPP_OSD1_BLD_H_SCOPE);
  vpu_mmio_.Write32(0 << 16 | (display_contents_size.height - 1), VPP_OSD1_BLD_V_SCOPE);

  // Setup VPP output.
  vpu_mmio_.Write32(display_contents_size.width, VPP_POSTBLEND_H_SIZE);
  vpu_mmio_.Write32((display_contents_size.width << 16) | display_contents_size.height,
                    VPU_VPP_OUT_H_V_SIZE);
}

void VideoInputUnit::DisableScaling() {
  // Disable osd scaler path.
  vpu_mmio_.Write32(0, VPU_VPP_OSD_SC_CTRL0);
  vpu_mmio_.Write32(0, VPU_VPP_OSD_VSC_CTRL0);
  vpu_mmio_.Write32(0, VPU_VPP_OSD_HSC_CTRL0);

  // Apply scale coefficients
  osd1_registers.scale_coef_idx.ReadFrom(&vpu_mmio_)
      .set_hi_res_coef(0)
      .set_h_coef(0)
      .set_index(0)
      .WriteTo(&vpu_mmio_);
  for (unsigned int i : osd_filter_coefs_bicubic) {
    osd1_registers.scale_coef.FromValue(i).WriteTo(&vpu_mmio_);
  }

  osd1_registers.scale_coef_idx.ReadFrom(&vpu_mmio_)
      .set_hi_res_coef(0)
      .set_h_coef(1)
      .set_index(0)
      .WriteTo(&vpu_mmio_);
  for (unsigned int i : osd_filter_coefs_bicubic) {
    osd1_registers.scale_coef.FromValue(i).WriteTo(&vpu_mmio_);
  }
}

void VideoInputUnit::SetMinimumRgb(uint8_t minimum_rgb) {
  // According to spec, minimum rgb should be set as follows:
  // Shift value by 2bits (8bit -> 10bit) and write new value for
  // each channel separately.
  VppClipMisc1Reg::Get()
      .FromValue(0)
      .set_r_clamp(minimum_rgb << 2)
      .set_g_clamp(minimum_rgb << 2)
      .set_b_clamp(minimum_rgb << 2)
      .WriteTo(&vpu_mmio_);
}

void VideoInputUnit::ConfigAfbcDecoder(PixelGridSize2D layer_image_size) {
  // The format specifier must match the sysmem format modifier flags specified
  // in DisplayEngine::DisplayEngineSetBufferCollectionConstraints().
  //
  // Note RGBA8888 works for both RGBA and BGRA formats. The color channels can
  // be reordered by setting MALI_UNPACK_CTRL register.
  osd1_registers.afbc_format_specifier_s.FromValue(0)
      .set_block_split_mode_enabled(true)
      .set_tiled_header_enabled(false)
      .set_yuv_transform_enabled(true)
      .set_super_block_aspect(kAfbcb16x16Pixel)
      .set_pixel_format(kAfbcRGBA8888)
      .WriteTo(&vpu_mmio_);

  // Setup color RGBA channel order
  osd1_registers.mali_unpack_ctrl.ReadFrom(&vpu_mmio_)
      .set_r(kAfbcColorReorderR)
      .set_g(kAfbcColorReorderG)
      .set_b(kAfbcColorReorderB)
      .set_a(kAfbcColorReorderA)
      .WriteTo(&vpu_mmio_);

  // Set afbc input buffer width/height in pixel
  osd1_registers.afbc_buffer_width_s.FromValue(0)
      .set_buffer_width(layer_image_size.width)
      .WriteTo(&vpu_mmio_);
  osd1_registers.afbc_buffer_height_s.FromValue(0)
      .set_buffer_height(layer_image_size.height)
      .WriteTo(&vpu_mmio_);

  // Set afbc input buffer
  osd1_registers.afbc_bounding_box_x_start_s.FromValue(0).set_buffer_x_start(0).WriteTo(&vpu_mmio_);
  osd1_registers.afbc_bounding_box_x_end_s.FromValue(0)
      .set_buffer_x_end(layer_image_size.width -
                        1)  // vendor code has width - 1 - 1, which is technically
                            // incorrect and gives the same result as this.
      .WriteTo(&vpu_mmio_);
  osd1_registers.afbc_bounding_box_y_start_s.FromValue(0).set_buffer_y_start(0).WriteTo(&vpu_mmio_);
  osd1_registers.afbc_bounding_box_y_end_s.FromValue(0)
      .set_buffer_y_end(layer_image_size.height -
                        1)  // vendor code has height -1 -1, but that cuts off the bottom row.
      .WriteTo(&vpu_mmio_);

  // Set output buffer stride
  osd1_registers.afbc_output_buf_stride_s.FromValue(0)
      .set_output_buffer_stride(layer_image_size.width * 4)
      .WriteTo(&vpu_mmio_);

  // Set afbc output buffer index
  // The way this is calculated based on vendor code is as follows:
  // Take OSD being used (1-based index): Therefore OSD1 -> index 1
  // out_addr = index << 24
  osd1_registers.afbc_output_buf_addr_low_s.FromValue(0).set_output_buffer_addr(1 << 24).WriteTo(
      &vpu_mmio_);
  osd1_registers.afbc_output_buf_addr_high_s.FromValue(0).set_output_buffer_addr(0).WriteTo(
      &vpu_mmio_);

  // Set linear address to the out_addr mentioned above
  osd1_registers.blk1_cfg_w4.FromValue(0).set_frame_addr(1 << 24).WriteTo(&vpu_mmio_);
}

bool VideoInputUnit::ConfigNeededForSingleNonscaledLayer(
    PixelGridSize2D layer_image_size, PixelGridSize2D display_contents_size) const {
  return layer_image_size != layer_image_size_ || display_contents_size != display_contents_size_;
}

void VideoInputUnit::ConfigForSingleNonscaledLayer(PixelGridSize2D layer_image_size,
                                                   PixelGridSize2D display_contents_size) {
  ZX_DEBUG_ASSERT_MSG(display_contents_size.IsValid(), "Invalid display size (%d x %d)",
                      display_contents_size.width, display_contents_size.height);
  ZX_DEBUG_ASSERT_MSG(layer_image_size.IsValid(), "Invalid framebuffer size (%d x %d)",
                      layer_image_size.width, layer_image_size.height);

  // TODO(https://fxbug.dev/317922128): Remove this assertion once we support
  // framebuffer scaling.
  ZX_DEBUG_ASSERT(layer_image_size.width == display_contents_size.width);
  ZX_DEBUG_ASSERT(layer_image_size.height == display_contents_size.height);

  // init vpu fifo control register
  uint32_t reg_val = vpu_mmio_.Read32(VPP_OFIFO_SIZE);
  reg_val = 0xfff << 20;
  reg_val |= (0xfff + 1);
  vpu_mmio_.Write32(reg_val, VPP_OFIFO_SIZE);

  ConfigOsdLayers(layer_image_size, display_contents_size);
  DisableScaling();
  ConfigSingleLayerBlending(/*layer_size=*/layer_image_size, display_contents_size);
  ConfigAfbcDecoder(layer_image_size);

  display_contents_size_ = display_contents_size;
  layer_image_size_ = layer_image_size;
}

#define REG_OFFSET (0x20 << 2)
void VideoInputUnit::Dump() {
  DumpNonRdmaRegisters();
  rdma_->DumpRdmaRegisters();
}

#define LOG_REG(reg) fdf::info("reg[0x{:x}]: 0x{:08x} " #reg, (reg), vpu_mmio_.Read32((reg)))
#define LOG_REG_INSTANCE(reg, offset, index)                                                 \
  fdf::info("reg[0x{:x}]: 0x{:08x} " #reg " #{}", (reg) + (offset), vpu_mmio_.Read32((reg)), \
            index + 1)
void VideoInputUnit::DumpNonRdmaRegisters() {
  uint32_t offset = 0;
  uint32_t index = 0;

  fdf::info("VPU_VIU_VENC_MUX_CTRL: 0x{:08x}",
            VideoInputUnitEncoderMuxControl::Get().ReadFrom(&vpu_mmio_).reg_value());
  LOG_REG(VPU_VPP_MISC);
  LOG_REG(VPU_VPP_OFIFO_SIZE);
  LOG_REG(VPU_VPP_HOLD_LINES);

  LOG_REG(VPU_OSD_PATH_MISC_CTRL);
  LOG_REG(VPU_VIU_OSD_BLEND_CTRL);
  LOG_REG(VPU_VIU_OSD_BLEND_DIN0_SCOPE_H);
  LOG_REG(VPU_VIU_OSD_BLEND_DIN0_SCOPE_V);
  LOG_REG(VPU_VIU_OSD_BLEND_DIN1_SCOPE_H);
  LOG_REG(VPU_VIU_OSD_BLEND_DIN1_SCOPE_V);
  LOG_REG(VPU_VIU_OSD_BLEND_DIN2_SCOPE_H);
  LOG_REG(VPU_VIU_OSD_BLEND_DIN2_SCOPE_V);
  LOG_REG(VPU_VIU_OSD_BLEND_DIN3_SCOPE_H);
  LOG_REG(VPU_VIU_OSD_BLEND_DIN3_SCOPE_V);
  LOG_REG(VPU_VIU_OSD_BLEND_DUMMY_DATA0);
  LOG_REG(VPU_VIU_OSD_BLEND_DUMMY_ALPHA);
  LOG_REG(VPU_VIU_OSD_BLEND_BLEND0_SIZE);
  LOG_REG(VPU_VIU_OSD_BLEND_BLEND1_SIZE);

  LOG_REG(VPU_VPP_OSD1_IN_SIZE);
  LOG_REG(VPU_VPP_OSD1_BLD_H_SCOPE);
  LOG_REG(VPU_VPP_OSD1_BLD_V_SCOPE);
  LOG_REG(VPU_VPP_OSD2_BLD_H_SCOPE);
  LOG_REG(VPU_VPP_OSD2_BLD_V_SCOPE);
  LOG_REG(OSD1_BLEND_SRC_CTRL);
  LOG_REG(OSD2_BLEND_SRC_CTRL);
  LOG_REG(VPU_VPP_POSTBLEND_H_SIZE);
  LOG_REG(VPU_VPP_OUT_H_V_SIZE);

  LOG_REG(VPU_VPP_OSD_SC_CTRL0);
  LOG_REG(VPU_VPP_OSD_SCI_WH_M1);
  LOG_REG(VPU_VPP_OSD_SCO_H_START_END);
  LOG_REG(VPU_VPP_OSD_SCO_V_START_END);
  LOG_REG(VPU_VPP_POSTBLEND_H_SIZE);
  for (index = 0; index < 2; index++) {
    if (index == 1)
      offset = REG_OFFSET;
    LOG_REG_INSTANCE(VPU_VIU_OSD1_FIFO_CTRL_STAT, offset, index);
    LOG_REG_INSTANCE(VPU_VIU_OSD1_CTRL_STAT, offset, index);
    LOG_REG_INSTANCE(VPU_VIU_OSD1_CTRL_STAT2, offset, index);
    LOG_REG_INSTANCE(VPU_VIU_OSD1_BLK0_CFG_W0, offset, index);
    LOG_REG_INSTANCE(VPU_VIU_OSD1_BLK0_CFG_W1, offset, index);
    LOG_REG_INSTANCE(VPU_VIU_OSD1_BLK0_CFG_W2, offset, index);
    LOG_REG_INSTANCE(VPU_VIU_OSD1_BLK0_CFG_W3, offset, index);
    if (index == 1) {
      LOG_REG(VPU_VIU_OSD2_BLK0_CFG_W4);
    } else {
      LOG_REG(VPU_VIU_OSD1_BLK0_CFG_W4);
    }
  }

  fdf::info("Dumping all Color Correction Matrix related Registers");
  fdf::info("VPU_VPP_POST_MATRIX_COEF00_01 = 0x{:x}",
            vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_COEF00_01));
  fdf::info("VPU_VPP_POST_MATRIX_COEF02_10 = 0x{:x}",
            vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_COEF02_10));
  fdf::info("VPU_VPP_POST_MATRIX_COEF11_12 = 0x{:x}",
            vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_COEF11_12));
  fdf::info("VPU_VPP_POST_MATRIX_COEF20_21 = 0x{:x}",
            vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_COEF20_21));
  fdf::info("VPU_VPP_POST_MATRIX_COEF22 = 0x{:x}", vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_COEF22));
  fdf::info("VPU_VPP_POST_MATRIX_OFFSET0_1 = 0x{:x}",
            vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_OFFSET0_1));
  fdf::info("VPU_VPP_POST_MATRIX_OFFSET2 = 0x{:x}", vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_OFFSET2));
  fdf::info("VPU_VPP_POST_MATRIX_PRE_OFFSET0_1 = 0x{:x}",
            vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_PRE_OFFSET0_1));
  fdf::info("VPU_VPP_POST_MATRIX_PRE_OFFSET2 = 0x{:x}",
            vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_PRE_OFFSET2));
  fdf::info("VPU_VPP_POST_MATRIX_EN_CTRL = 0x{:x}", vpu_mmio_.Read32(VPU_VPP_POST_MATRIX_EN_CTRL));
}

void VideoInputUnit::Release() {
  DisableLayer();
  rdma_->Release();
}

// static
zx::result<std::unique_ptr<VideoInputUnit>> VideoInputUnit::Create(
    fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device,
    inspect::Node* video_input_unit_node) {
  ZX_DEBUG_ASSERT(platform_device.is_valid());

  zx::result<fdf::MmioBuffer> vpu_mmio_result = MapMmio(kMmioNameVpu, platform_device);
  if (vpu_mmio_result.is_error()) {
    return vpu_mmio_result.take_error();
  }

  zx::result<std::unique_ptr<RdmaEngine>> rdma_result =
      RdmaEngine::Create(platform_device, video_input_unit_node);
  if (rdma_result.is_error()) {
    return rdma_result.take_error();
  }

  fbl::AllocChecker alloc_checker;
  auto self = fbl::make_unique_checked<VideoInputUnit>(
      &alloc_checker, std::move(vpu_mmio_result).value(), std::move(rdma_result).value());
  if (!alloc_checker.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx_status_t status = self->rdma_->SetupRdma();
  if (status != ZX_OK) {
    fdf::error("Could not setup RDMA");
    return zx::error(status);
  }

  return zx::ok(self.release());
}

// static
zx::result<std::unique_ptr<VideoInputUnit>> VideoInputUnit::CreateForTesting(
    fdf::MmioBuffer vpu_mmio, std::unique_ptr<RdmaEngine> rdma, PixelGridSize2D layer_image_size,
    PixelGridSize2D display_contents_size) {
  fbl::AllocChecker alloc_checker;
  auto self = fbl::make_unique_checked<VideoInputUnit>(&alloc_checker, std::move(vpu_mmio),
                                                       std::move(rdma));
  if (!alloc_checker.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  self->layer_image_size_ = layer_image_size;
  self->display_contents_size_ = display_contents_size;
  return zx::ok(std::move(self));
}

}  // namespace amlogic_display
