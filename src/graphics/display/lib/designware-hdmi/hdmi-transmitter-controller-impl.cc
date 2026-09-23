// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/designware-hdmi/hdmi-transmitter-controller-impl.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>

#include <fbl/vector.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/designware-hdmi/color-param.h"
#include "src/graphics/display/lib/designware-hdmi/ddc-controller-regs.h"
#include "src/graphics/display/lib/designware-hdmi/regs.h"

namespace designware_hdmi {

namespace {

// HDMI Specification 2.0b, Section 10.4.3 "Data Transfer Protocols", page 125.
constexpr uint8_t kScdcI2cTargetAddress = 0x54;

// The I2C address for writing the DDC segment.
//
// VESA Enhanced Display Data Channel (E-DDC) Standard version 1.3 revised
// Dec 31 2020, Section 2.2.3 "DDC Addresses", page 17.
constexpr uint8_t kDdcSegmentI2cTargetAddress = 0x30;

// The I2C address for writing the DDC data offset/reading DDC data.
//
// VESA Enhanced Display Data Channel (E-DDC) Standard version 1.3 revised
// Dec 31 2020, Section 2.2.3 "DDC Addresses", page 17.
constexpr uint8_t kDdcDataI2cTargetAddress = 0x50;

}  // namespace

void HdmiTransmitterControllerImpl::ScdcWrite(uint8_t addr, uint8_t val) {
  registers::DdcControllerDataTargetAddress::Get()
      .FromValue(0)
      .set_data_target_address(kScdcI2cTargetAddress)
      .WriteTo(&controller_mmio_);
  registers::DdcControllerWordOffset::Get().FromValue(0).set_word_offset(addr).WriteTo(
      &controller_mmio_);
  registers::DdcControllerWriteByte::Get().FromValue(0).set_byte(val).WriteTo(&controller_mmio_);
  registers::DdcControllerCommand::Get().FromValue(0).set_write(true).WriteTo(&controller_mmio_);
  zx::nanosleep(zx::deadline_after(zx::usec(2000)));
}

uint8_t HdmiTransmitterControllerImpl::ScdcRead(uint8_t addr) {
  registers::DdcControllerDataTargetAddress::Get()
      .FromValue(0)
      .set_data_target_address(kScdcI2cTargetAddress)
      .WriteTo(&controller_mmio_);
  registers::DdcControllerWordOffset::Get().FromValue(0).set_word_offset(addr).WriteTo(
      &controller_mmio_);
  registers::DdcControllerCommand::Get().FromValue(0).set_ddc_read_byte(true).WriteTo(
      &controller_mmio_);
  zx::nanosleep(zx::deadline_after(zx::usec(2000)));

  return registers::DdcControllerReadByte::Get().ReadFrom(&controller_mmio_).byte();
}

zx_status_t HdmiTransmitterControllerImpl::InitHw() {
  WriteReg(HDMITX_DWC_MC_LOCKONCLOCK, 0xff);
  WriteReg(HDMITX_DWC_MC_CLKDIS, 0x00);

  /* Step 2: Initialize DDC Interface (For EDID) */

  // FIXME: Pinmux i2c pins (skip for now since uboot it doing it)

  // Configure i2c interface
  // a. Do not mask any interrupt (read_req, done, nack, arbitration)
  registers::DdcControllerDoneInterruptMask::Get()
      .FromValue(0)
      .set_read_request_masked(false)
      .set_command_done_masked(false)
      .WriteTo(&controller_mmio_);
  registers::DdcControllerErrorInterruptMask::Get()
      .FromValue(0)
      .set_nack_masked(false)
      .set_arbitration_masked(false)
      .WriteTo(&controller_mmio_);

  // b. set interface to standard mode
  registers::DdcControllerClockControl::Get()
      .FromValue(0)
      .set_i2c_controller_transfer_mode(
          registers::DdcControllerClockControl::I2cControllerTransferMode::kStandardMode)
      .WriteTo(&controller_mmio_);

  // c. Setup i2c timings (based on u-boot source)
  registers::DdcControllerSlowSpeedSclHighLevelControl1::Get().FromValue(0x00).WriteTo(
      &controller_mmio_);
  registers::DdcControllerSlowSpeedSclHighLevelControl0::Get().FromValue(0xcf).WriteTo(
      &controller_mmio_);
  registers::DdcControllerSlowSpeedSclLowLevelControl1::Get().FromValue(0x00).WriteTo(
      &controller_mmio_);
  registers::DdcControllerSlowSpeedSclLowLevelControl0::Get().FromValue(0xff).WriteTo(
      &controller_mmio_);
  registers::DdcControllerFastSpeedSclHighLevelControl1::Get().FromValue(0x00).WriteTo(
      &controller_mmio_);
  registers::DdcControllerFastSpeedSclHighLevelControl0::Get().FromValue(0x0f).WriteTo(
      &controller_mmio_);
  registers::DdcControllerFastSpeedSclLowLevelControl1::Get().FromValue(0x00).WriteTo(
      &controller_mmio_);
  registers::DdcControllerFastSpeedSclLowLevelControl0::Get().FromValue(0x20).WriteTo(
      &controller_mmio_);
  registers::DdcControllerDataPinHoldTime::Get().FromValue(0).set_data_pin_hold_time(8).WriteTo(
      &controller_mmio_);

  // d. disable any SCDC operations for now
  registers::DdcControllerScdcControl::Get()
      .FromValue(registers::DdcControllerScdcControl::kDisableAllScdcOperations)
      .WriteTo(&controller_mmio_);

  return ZX_OK;
}

void HdmiTransmitterControllerImpl::ConfigHdmitx(const ColorParam& color_param,
                                                 const display::DisplayTiming& mode,
                                                 const hdmi_param_tx& p) {
  // setup video input mapping
  uint8_t video_input_mapping_config = 0;
  if (color_param.input_color_format == ColorFormat::kCfRgb) {
    switch (color_param.color_depth) {
      case ColorDepth::kCd24B:
        video_input_mapping_config |= TX_INVID0_VM_RGB444_8B;
        break;
      case ColorDepth::kCd30B:
        video_input_mapping_config |= TX_INVID0_VM_RGB444_10B;
        break;
      case ColorDepth::kCd36B:
        video_input_mapping_config |= TX_INVID0_VM_RGB444_12B;
        break;
      case ColorDepth::kCd48B:
      default:
        video_input_mapping_config |= TX_INVID0_VM_RGB444_16B;
        break;
    }
  } else if (color_param.input_color_format == ColorFormat::kCf444) {
    switch (color_param.color_depth) {
      case ColorDepth::kCd24B:
        video_input_mapping_config |= TX_INVID0_VM_YCBCR444_8B;
        break;
      case ColorDepth::kCd30B:
        video_input_mapping_config |= TX_INVID0_VM_YCBCR444_10B;
        break;
      case ColorDepth::kCd36B:
        video_input_mapping_config |= TX_INVID0_VM_YCBCR444_12B;
        break;
      case ColorDepth::kCd48B:
      default:
        video_input_mapping_config |= TX_INVID0_VM_YCBCR444_16B;
        break;
    }
  } else {
    ZX_DEBUG_ASSERT_MSG(false, "Invalid display input color format: %d",
                        static_cast<uint8_t>(color_param.input_color_format));
    return;
  }
  WriteReg(HDMITX_DWC_TX_INVID0, video_input_mapping_config);

  // Disable video input stuffing and zero-out related registers
  WriteReg(HDMITX_DWC_TX_INSTUFFING, 0x00);
  WriteReg(HDMITX_DWC_TX_GYDATA0, 0x00);
  WriteReg(HDMITX_DWC_TX_GYDATA1, 0x00);
  WriteReg(HDMITX_DWC_TX_RCRDATA0, 0x00);
  WriteReg(HDMITX_DWC_TX_RCRDATA1, 0x00);
  WriteReg(HDMITX_DWC_TX_BCBDATA0, 0x00);
  WriteReg(HDMITX_DWC_TX_BCBDATA1, 0x00);

  // configure CSC (Color Space Converter)
  ConfigCsc(color_param);

  // Video packet color depth and pixel repetition (none). writing 0 is also valid
  // hdmi_data = (4 << 4); // 4 == 24bit
  // hdmi_data = (display->color_depth << 4); // 4 == 24bit
  WriteReg(HDMITX_DWC_VP_PR_CD, (0 << 4));  // 4 == 24bit

  // setup video packet stuffing (nothing fancy to be done here)
  WriteReg(HDMITX_DWC_VP_STUFF, 0);

  // setup video packet remap (nothing here as well since we don't support 422)
  WriteReg(HDMITX_DWC_VP_REMAP, 0);

  // vp packet output configuration
  const uint8_t vp_packet_configuration =
      VP_CONF_BYPASS_EN | VP_CONF_BYPASS_SEL_VP | VP_CONF_OUTSELECTOR;
  WriteReg(HDMITX_DWC_VP_CONF, vp_packet_configuration);

  // Video packet Interrupt Mask
  WriteReg(HDMITX_DWC_VP_MASK, 0xFF);  // set all bits

  // TODO: For now skip audio configuration

  // Setup frame composer

  // fc_invidconf setup
  uint8_t input_video_configuration =
      FC_INVIDCONF_HDCP_KEEPOUT | FC_INVIDCONF_VSYNC_POL(mode->flags & ModeFlag::kVsyncPositive) |
      FC_INVIDCONF_HSYNC_POL(mode->flags & ModeFlag::kHsyncPositive) | FC_INVIDCONF_DE_POL_H |
      FC_INVIDCONF_DVI_HDMI_MODE;
  if (mode.fields_per_frame == display::FieldsPerFrame::kInterlaced) {
    input_video_configuration |= FC_INVIDCONF_VBLANK_OSC | FC_INVIDCONF_IN_VID_INTERLACED;
  }
  WriteReg(HDMITX_DWC_FC_INVIDCONF, input_video_configuration);

  // TODO(https://fxbug.dev/325994853): Add a configuration on the display
  // timings and make the ZX_ASSERT() checks below preconditions of
  // ConfigHdmiTx.

  // HActive
  const int horizontal_active_px = mode.horizontal_active_px;
  ZX_ASSERT(horizontal_active_px <= 0x3fff);
  WriteReg(HDMITX_DWC_FC_INHACTV0, (horizontal_active_px & 0xff));
  WriteReg(HDMITX_DWC_FC_INHACTV1, ((horizontal_active_px >> 8) & 0x3f));

  // HBlank
  const int horizontal_blank_px = mode.horizontal_blank_px();
  ZX_ASSERT(horizontal_blank_px <= 0x1fff);
  WriteReg(HDMITX_DWC_FC_INHBLANK0, (horizontal_blank_px & 0xff));
  WriteReg(HDMITX_DWC_FC_INHBLANK1, ((horizontal_blank_px >> 8) & 0x1f));

  // VActive
  const int vertical_active_lines = mode.vertical_active_lines;
  ZX_ASSERT(vertical_active_lines <= 0x1fff);
  WriteReg(HDMITX_DWC_FC_INVACTV0, (vertical_active_lines & 0xff));
  WriteReg(HDMITX_DWC_FC_INVACTV1, ((vertical_active_lines >> 8) & 0x1f));

  // VBlank
  const int vertical_blank_lines = mode.vertical_blank_lines();
  ZX_ASSERT(vertical_blank_lines <= 0xff);
  WriteReg(HDMITX_DWC_FC_INVBLANK, (vertical_blank_lines & 0xff));

  // HFP
  const int horizontal_front_porch_px = mode.horizontal_front_porch_px;
  ZX_ASSERT(horizontal_front_porch_px <= 0x1fff);
  WriteReg(HDMITX_DWC_FC_HSYNCINDELAY0, (horizontal_front_porch_px & 0xff));
  WriteReg(HDMITX_DWC_FC_HSYNCINDELAY1, ((horizontal_front_porch_px >> 8) & 0x1f));

  // HSync
  const int horizontal_sync_width_px = mode.horizontal_sync_width_px;
  ZX_ASSERT(horizontal_sync_width_px <= 0x3ff);
  WriteReg(HDMITX_DWC_FC_HSYNCINWIDTH0, (horizontal_sync_width_px & 0xff));
  WriteReg(HDMITX_DWC_FC_HSYNCINWIDTH1, ((horizontal_sync_width_px >> 8) & 0x3));

  // VFront
  const int vertical_front_porch_lines = mode.vertical_front_porch_lines;
  ZX_ASSERT(vertical_front_porch_lines <= 0xff);
  WriteReg(HDMITX_DWC_FC_VSYNCINDELAY, (vertical_front_porch_lines & 0xff));

  // VSync
  const int vertical_sync_width_lines = mode.vertical_sync_width_lines;
  ZX_ASSERT(vertical_sync_width_lines <= 0x3f);
  WriteReg(HDMITX_DWC_FC_VSYNCINWIDTH, (vertical_sync_width_lines & 0x3f));

  // Frame Composer control period duration (set to 12 per spec)
  WriteReg(HDMITX_DWC_FC_CTRLDUR, 12);

  // Frame Composer extended control period duration (set to 32 per spec)
  WriteReg(HDMITX_DWC_FC_EXCTRLDUR, 32);

  // Frame Composer extended control period max spacing (FIXME: spec says 50, uboot sets to 1)
  WriteReg(HDMITX_DWC_FC_EXCTRLSPAC, 1);

  // Frame Composer preamble filler (from uBoot)

  // Frame Composer GCP packet config
  WriteReg(HDMITX_DWC_FC_GCP, (1 << 0));  // set avmute. defauly_phase is 0

  // Frame Composer AVI Packet config (set active_format_present bit)
  // aviconf0 populates Table 10 of CEA spec (AVI InfoFrame Data Byte 1)
  // Y1Y0 = 00 for RGB, 10 for 444
  if (color_param.output_color_format == ColorFormat::kCfRgb) {
    video_input_mapping_config = FC_AVICONF0_RGB;
  } else {
    video_input_mapping_config = FC_AVICONF0_444;
  }
  // A0 = 1 Active Formate present on R3R0
  video_input_mapping_config |= FC_AVICONF0_A0;
  WriteReg(HDMITX_DWC_FC_AVICONF0, video_input_mapping_config);

  // aviconf1 populates Table 11 of AVI InfoFrame Data Byte 2
  // C1C0 = 0, M1M0=0x2 (16:9), R3R2R1R0=0x8 (same of M1M0)
  video_input_mapping_config = FC_AVICONF1_R3R0;  // set to 0x8 (same as coded frame aspect ratio)
  video_input_mapping_config |= FC_AVICONF1_M1M0(static_cast<uint8_t>(p.aspect_ratio));
  video_input_mapping_config |= FC_AVICONF1_C1C0(static_cast<uint8_t>(p.colorimetry));
  WriteReg(HDMITX_DWC_FC_AVICONF1, video_input_mapping_config);

  // Since we are support RGB/444, no need to write to ECx
  WriteReg(HDMITX_DWC_FC_AVICONF2, 0x0);

  // YCC and IT Quantizations according to CEA spec (limited range for now)
  WriteReg(HDMITX_DWC_FC_AVICONF3, 0x0);

  // Set AVI InfoFrame VIC
  // WriteReg(HDMITX_DWC_FC_AVIVID, (p->vic >= VESA_OFFSET)? 0 : p->vic);

  WriteReg(HDMITX_DWC_FC_ACTSPC_HDLR_CFG, 0);

  // Frame composer 2d vact config
  ZX_ASSERT(vertical_active_lines <= 0xfff);
  WriteReg(HDMITX_DWC_FC_INVACT_2D_0, (vertical_active_lines & 0xff));
  WriteReg(HDMITX_DWC_FC_INVACT_2D_1, ((vertical_active_lines >> 8) & 0xf));

  // disable all Frame Composer interrupts
  WriteReg(HDMITX_DWC_FC_MASK0, 0xe7);
  WriteReg(HDMITX_DWC_FC_MASK1, 0xfb);
  WriteReg(HDMITX_DWC_FC_MASK2, 0x3);

  // No pixel repetition for the currently supported resolution
  // TODO: pixel repetition is 0 for most progressive. We don't support interlaced
  static constexpr uint8_t kPixelRepeat = 0;
  WriteReg(HDMITX_DWC_FC_PRCONF, ((kPixelRepeat + 1) << 4) | (kPixelRepeat) << 0);

  // Skip HDCP for now

  // Clear Interrupts
  WriteReg(HDMITX_DWC_IH_FC_STAT0, 0xff);
  WriteReg(HDMITX_DWC_IH_FC_STAT1, 0xff);
  WriteReg(HDMITX_DWC_IH_FC_STAT2, 0xff);
  WriteReg(HDMITX_DWC_IH_AS_STAT0, 0xff);
  WriteReg(HDMITX_DWC_IH_PHY_STAT0, 0xff);
  // TODO(https://fxbug.dev/390552175): The Amlogic-provided reference code
  // sets the register to 0xff. We should figure out whether it's necessary to
  // set the undefined bits.
  registers::DdcControllerInterruptStatus::Get()
      .FromValue(0xff)
      .set_read_request_pending(true)
      .set_command_done_pending(true)
      .set_error_pending(true)
      .WriteTo(&controller_mmio_);
  WriteReg(HDMITX_DWC_IH_CEC_STAT0, 0xff);
  WriteReg(HDMITX_DWC_IH_VP_STAT0, 0xff);
  WriteReg(HDMITX_DWC_IH_I2CMPHY_STAT0, 0xff);
  WriteReg(HDMITX_DWC_A_APIINTCLR, 0xff);
  WriteReg(HDMITX_DWC_HDCP22REG_STAT, 0xff);
}

void HdmiTransmitterControllerImpl::SetupInterrupts() {
  // setup interrupts we care about
  WriteReg(HDMITX_DWC_IH_MUTE_FC_STAT0, 0xff);
  WriteReg(HDMITX_DWC_IH_MUTE_FC_STAT1, 0xff);
  WriteReg(HDMITX_DWC_IH_MUTE_FC_STAT2, 0x3);

  WriteReg(HDMITX_DWC_IH_MUTE_AS_STAT0, 0x7);  // mute all

  WriteReg(HDMITX_DWC_IH_MUTE_PHY_STAT0, 0x3f);

  // The DDC I2C "command done" interrupt is muted. The driver won't receive
  // interrupts on E-DDC read / write completion, instead it periodically polls
  // the interrupt status register.
  registers::DdcControllerInterruptMute::Get().FromValue(0).set_command_done_muted(true).WriteTo(
      &controller_mmio_);

  // turn all cec-related interrupts on
  WriteReg(HDMITX_DWC_IH_MUTE_CEC_STAT0, 0x0);

  WriteReg(HDMITX_DWC_IH_MUTE_VP_STAT0, 0xff);

  WriteReg(HDMITX_DWC_IH_MUTE_I2CMPHY_STAT0, 0x03);

  // enable global interrupt
  WriteReg(HDMITX_DWC_IH_MUTE, 0x0);
}

void HdmiTransmitterControllerImpl::Reset() {
  // reset
  WriteReg(HDMITX_DWC_MC_SWRSTZREQ, 0x00);
  zx::nanosleep(zx::deadline_after(zx::usec(10)));
  WriteReg(HDMITX_DWC_MC_SWRSTZREQ, 0x7d);
  // why???
  WriteReg(HDMITX_DWC_FC_VSYNCINWIDTH, ReadReg(HDMITX_DWC_FC_VSYNCINWIDTH));

  WriteReg(HDMITX_DWC_MC_CLKDIS, 0);
}

void HdmiTransmitterControllerImpl::SetupScdc(bool is4k) {
  uint8_t scdc_data = ScdcRead(0x1);
  fdf::info("version is {}", (scdc_data == 1) ? "2.0" : "<= 1.4");
  // scdc write is done twice in uboot
  // TODO: find scdc register def
  ScdcWrite(0x2, 0x1);
  ScdcWrite(0x2, 0x1);

  if (is4k) {
    ScdcWrite(0x20, 3);
    ScdcWrite(0x20, 3);
  } else {
    ScdcWrite(0x20, 0);
    ScdcWrite(0x20, 0);
  }
}

void HdmiTransmitterControllerImpl::ResetFc() {
  auto regval = ReadReg(HDMITX_DWC_FC_INVIDCONF);
  regval &= ~(1 << 3);  // clear hdmi mode select
  WriteReg(HDMITX_DWC_FC_INVIDCONF, regval);
  zx::nanosleep(zx::deadline_after(zx::usec(1)));
  regval = ReadReg(HDMITX_DWC_FC_INVIDCONF);
  regval |= (1 << 3);  // clear hdmi mode select
  WriteReg(HDMITX_DWC_FC_INVIDCONF, regval);
  zx::nanosleep(zx::deadline_after(zx::usec(1)));
}

void HdmiTransmitterControllerImpl::SetFcScramblerCtrl(bool is4k) {
  if (is4k) {
    // Set
    WriteReg(HDMITX_DWC_FC_SCRAMBLER_CTRL, ReadReg(HDMITX_DWC_FC_SCRAMBLER_CTRL) | (1 << 0));
  } else {
    // Clear
    WriteReg(HDMITX_DWC_FC_SCRAMBLER_CTRL, 0);
  }
}

void HdmiTransmitterControllerImpl::ConfigCsc(const ColorParam& color_param) {
  uint8_t csc_coef_a1_msb;
  uint8_t csc_coef_a1_lsb;
  uint8_t csc_coef_a2_msb;
  uint8_t csc_coef_a2_lsb;
  uint8_t csc_coef_a3_msb;
  uint8_t csc_coef_a3_lsb;
  uint8_t csc_coef_a4_msb;
  uint8_t csc_coef_a4_lsb;
  uint8_t csc_coef_b1_msb;
  uint8_t csc_coef_b1_lsb;
  uint8_t csc_coef_b2_msb;
  uint8_t csc_coef_b2_lsb;
  uint8_t csc_coef_b3_msb;
  uint8_t csc_coef_b3_lsb;
  uint8_t csc_coef_b4_msb;
  uint8_t csc_coef_b4_lsb;
  uint8_t csc_coef_c1_msb;
  uint8_t csc_coef_c1_lsb;
  uint8_t csc_coef_c2_msb;
  uint8_t csc_coef_c2_lsb;
  uint8_t csc_coef_c3_msb;
  uint8_t csc_coef_c3_lsb;
  uint8_t csc_coef_c4_msb;
  uint8_t csc_coef_c4_lsb;
  uint8_t csc_scale;

  // Color space conversion is needed by default.
  uint8_t main_controller_feed_through_control = MC_FLOWCTRL_ENB_CSC;
  if (color_param.input_color_format == color_param.output_color_format) {
    // no need to convert
    main_controller_feed_through_control = MC_FLOWCTRL_BYPASS_CSC;
  }
  WriteReg(HDMITX_DWC_MC_FLOWCTRL, main_controller_feed_through_control);

  // Since we don't support 422 at this point, set csc_cfg to 0
  WriteReg(HDMITX_DWC_CSC_CFG, 0);

  // Co-efficient values are from DesignWare Core HDMI TX Video Datapath Application Note V2.1

  // First determine whether we need to convert or not
  if (color_param.input_color_format != color_param.output_color_format) {
    if (color_param.input_color_format == ColorFormat::kCfRgb) {
      // from RGB
      csc_coef_a1_msb = 0x25;
      csc_coef_a1_lsb = 0x91;
      csc_coef_a2_msb = 0x13;
      csc_coef_a2_lsb = 0x23;
      csc_coef_a3_msb = 0x07;
      csc_coef_a3_lsb = 0x4C;
      csc_coef_a4_msb = 0x00;
      csc_coef_a4_lsb = 0x00;
      csc_coef_b1_msb = 0xE5;
      csc_coef_b1_lsb = 0x34;
      csc_coef_b2_msb = 0x20;
      csc_coef_b2_lsb = 0x00;
      csc_coef_b3_msb = 0xFA;
      csc_coef_b3_lsb = 0xCC;
      switch (color_param.color_depth) {
        case ColorDepth::kCd24B:
          csc_coef_b4_msb = 0x02;
          csc_coef_b4_lsb = 0x00;
          csc_coef_c4_msb = 0x02;
          csc_coef_c4_lsb = 0x00;
          break;
        case ColorDepth::kCd30B:
          csc_coef_b4_msb = 0x08;
          csc_coef_b4_lsb = 0x00;
          csc_coef_c4_msb = 0x08;
          csc_coef_c4_lsb = 0x00;
          break;
        case ColorDepth::kCd36B:
          csc_coef_b4_msb = 0x20;
          csc_coef_b4_lsb = 0x00;
          csc_coef_c4_msb = 0x20;
          csc_coef_c4_lsb = 0x00;
          break;
        default:
          csc_coef_b4_msb = 0x20;
          csc_coef_b4_lsb = 0x00;
          csc_coef_c4_msb = 0x20;
          csc_coef_c4_lsb = 0x00;
      }
      csc_coef_c1_msb = 0xEA;
      csc_coef_c1_lsb = 0xCD;
      csc_coef_c2_msb = 0xF5;
      csc_coef_c2_lsb = 0x33;
      csc_coef_c3_msb = 0x20;
      csc_coef_c3_lsb = 0x00;
      csc_scale = 0;
    } else {
      // to RGB
      csc_coef_a1_msb = 0x10;
      csc_coef_a1_lsb = 0x00;
      csc_coef_a2_msb = 0xf4;
      csc_coef_a2_lsb = 0x93;
      csc_coef_a3_msb = 0xfa;
      csc_coef_a3_lsb = 0x7f;
      csc_coef_b1_msb = 0x10;
      csc_coef_b1_lsb = 0x00;
      csc_coef_b2_msb = 0x16;
      csc_coef_b2_lsb = 0x6e;
      csc_coef_b3_msb = 0x00;
      csc_coef_b3_lsb = 0x00;
      switch (color_param.color_depth) {
        case ColorDepth::kCd24B:
          csc_coef_a4_msb = 0x00;
          csc_coef_a4_lsb = 0x87;
          csc_coef_b4_msb = 0xff;
          csc_coef_b4_lsb = 0x4d;
          csc_coef_c4_msb = 0xff;
          csc_coef_c4_lsb = 0x1e;
          break;
        case ColorDepth::kCd30B:
          csc_coef_a4_msb = 0x02;
          csc_coef_a4_lsb = 0x1d;
          csc_coef_b4_msb = 0xfd;
          csc_coef_b4_lsb = 0x33;
          csc_coef_c4_msb = 0xfc;
          csc_coef_c4_lsb = 0x75;
          break;
        case ColorDepth::kCd36B:
          csc_coef_a4_msb = 0x08;
          csc_coef_a4_lsb = 0x77;
          csc_coef_b4_msb = 0xf4;
          csc_coef_b4_lsb = 0xc9;
          csc_coef_c4_msb = 0xf1;
          csc_coef_c4_lsb = 0xd3;
          break;
        default:
          csc_coef_a4_msb = 0x08;
          csc_coef_a4_lsb = 0x77;
          csc_coef_b4_msb = 0xf4;
          csc_coef_b4_lsb = 0xc9;
          csc_coef_c4_msb = 0xf1;
          csc_coef_c4_lsb = 0xd3;
      }
      csc_coef_b4_msb = 0xff;
      csc_coef_b4_lsb = 0x4d;
      csc_coef_c1_msb = 0x10;
      csc_coef_c1_lsb = 0x00;
      csc_coef_c2_msb = 0x00;
      csc_coef_c2_lsb = 0x00;
      csc_coef_c3_msb = 0x1c;
      csc_coef_c3_lsb = 0x5a;
      csc_coef_c4_msb = 0xff;
      csc_coef_c4_lsb = 0x1e;
      csc_scale = 2;
    }
  } else {
    // No conversion. re-write default values just in case
    csc_coef_a1_msb = 0x20;
    csc_coef_a1_lsb = 0x00;
    csc_coef_a2_msb = 0x00;
    csc_coef_a2_lsb = 0x00;
    csc_coef_a3_msb = 0x00;
    csc_coef_a3_lsb = 0x00;
    csc_coef_a4_msb = 0x00;
    csc_coef_a4_lsb = 0x00;
    csc_coef_b1_msb = 0x00;
    csc_coef_b1_lsb = 0x00;
    csc_coef_b2_msb = 0x20;
    csc_coef_b2_lsb = 0x00;
    csc_coef_b3_msb = 0x00;
    csc_coef_b3_lsb = 0x00;
    csc_coef_b4_msb = 0x00;
    csc_coef_b4_lsb = 0x00;
    csc_coef_c1_msb = 0x00;
    csc_coef_c1_lsb = 0x00;
    csc_coef_c2_msb = 0x00;
    csc_coef_c2_lsb = 0x00;
    csc_coef_c3_msb = 0x20;
    csc_coef_c3_lsb = 0x00;
    csc_coef_c4_msb = 0x00;
    csc_coef_c4_lsb = 0x00;
    csc_scale = 1;
  }

  WriteReg(HDMITX_DWC_CSC_COEF_A1_MSB, csc_coef_a1_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_A1_LSB, csc_coef_a1_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_A2_MSB, csc_coef_a2_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_A2_LSB, csc_coef_a2_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_A3_MSB, csc_coef_a3_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_A3_LSB, csc_coef_a3_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_A4_MSB, csc_coef_a4_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_A4_LSB, csc_coef_a4_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_B1_MSB, csc_coef_b1_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_B1_LSB, csc_coef_b1_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_B2_MSB, csc_coef_b2_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_B2_LSB, csc_coef_b2_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_B3_MSB, csc_coef_b3_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_B3_LSB, csc_coef_b3_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_B4_MSB, csc_coef_b4_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_B4_LSB, csc_coef_b4_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_C1_MSB, csc_coef_c1_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_C1_LSB, csc_coef_c1_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_C2_MSB, csc_coef_c2_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_C2_LSB, csc_coef_c2_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_C3_MSB, csc_coef_c3_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_C3_LSB, csc_coef_c3_lsb);
  WriteReg(HDMITX_DWC_CSC_COEF_C4_MSB, csc_coef_c4_msb);
  WriteReg(HDMITX_DWC_CSC_COEF_C4_LSB, csc_coef_c4_lsb);

  // The value of `color_param.color_depth` is >= 0 and <= 7. So
  // `CSC_SCALE_COLOR_DEPTH()` won't cause an integer overflow.
  //
  // The value of `csc_scale` is 0, 1, or 2. So `CSC_SCALE_CSCSCALE()` won't
  // cause an integer overflow.
  //
  // `CSC_SCALE_COLOR_DEPTH(color_param.color_depth)` only occupies the bits 4-6
  // and `CSC_SCALE_CSCSCALE(csc_scale)` only occupies the bits 0-1. Thus they
  // won't overlap in any bit in the bitwise or operation.
  const uint8_t color_space_conversion_config =
      static_cast<const uint8_t>(
          CSC_SCALE_COLOR_DEPTH(static_cast<uint8_t>(color_param.color_depth))) |
      static_cast<const uint8_t>(CSC_SCALE_CSCSCALE(csc_scale));
  WriteReg(HDMITX_DWC_CSC_SCALE, color_space_conversion_config);
}

bool HdmiTransmitterControllerImpl::PollForDdcCommandDone() {
  auto interrupt_status = registers::DdcControllerInterruptStatus::Get().FromValue(0);
  bool interrupt_triggered = false;
  for (int attempt = 0; attempt < kMaxAttemptCountForPollForDdcCommandDone; ++attempt) {
    interrupt_status.ReadFrom(&controller_mmio_);
    if (interrupt_status.command_done_pending()) {
      interrupt_triggered = true;
      break;
    }

    // The duration between polls is from the U-boot reference code provided by
    // Amlogic.
    constexpr zx::duration kPollDuration = zx::usec(1000);
    zx::nanosleep(zx::deadline_after(kPollDuration));
  }

  if (!interrupt_triggered) {
    return false;
  }

  // The sleep duration is from the U-boot reference code provided by Amlogic.
  constexpr zx::duration kWaitDurationBeforeAckInterrupt = zx::usec(1000);
  zx::nanosleep(zx::deadline_after(kWaitDurationBeforeAckInterrupt));
  interrupt_status.set_command_done_pending(true).WriteTo(&controller_mmio_);
  return true;
}

zx::result<> HdmiTransmitterControllerImpl::ReadEdidBlock(
    int index, std::span<uint8_t, edid::kBlockSize> edid_block) {
  ZX_DEBUG_ASSERT(index >= 0);
  ZX_DEBUG_ASSERT(index < edid::kMaxEdidBlockCount);

  registers::DdcControllerDataTargetAddress::Get()
      .FromValue(0)
      .set_data_target_address(kDdcDataI2cTargetAddress)
      .WriteTo(&controller_mmio_);
  registers::DdcControllerSegmentTargetAddress::Get()
      .FromValue(0)
      .set_segment_target_address(kDdcSegmentI2cTargetAddress)
      .WriteTo(&controller_mmio_);

  // Size of an E-DDC segment.
  //
  // VESA Enhanced Display Data Channel (E-DDC) Standard version 1.3 revised
  // Dec 31 2020, Section 2.2.5 "Segment Pointer", page 18.
  static constexpr int kEddcSegmentSize = 256;
  static_assert(kEddcSegmentSize == edid::kBlockSize * 2);

  const int segment_pointer = index / 2;

  // `segment_pointer` is in [0, 127], so casting `segment_pointer` to uint8_t
  // doesn't overflow.
  registers::DdcControllerSegmentPointer::Get()
      .FromValue(0)
      .set_segment_pointer(static_cast<uint8_t>(segment_pointer))
      .WriteTo(&controller_mmio_);

  // Segment offset of the first byte in the current block.
  const int initial_segment_offset = (index % 2) * static_cast<int>(edid::kBlockSize);

  for (uint8_t bytes_read = 0; bytes_read < edid::kBlockSize; bytes_read += 8) {
    const int segment_offset = initial_segment_offset + bytes_read;

    // `segment_offset` is in [0, 255], so casting `segment_offset` to uint8_t
    // doesn't overflow.
    registers::DdcControllerWordOffset::Get()
        .FromValue(0)
        .set_word_offset(static_cast<uint8_t>(segment_offset))
        .WriteTo(&controller_mmio_);

    registers::DdcControllerCommand::Get().FromValue(0).set_eddc_read_8bytes(true).WriteTo(
        &controller_mmio_);

    bool success = PollForDdcCommandDone();
    if (!success) {
      fdf::error("DDC controller did not finish reading after {} attempts",
                 kMaxAttemptCountForPollForDdcCommandDone);
      return zx::error(ZX_ERR_TIMED_OUT);
    }

    for (int i = 0; i < 8; i++) {
      edid_block[bytes_read + i] =
          registers::DdcControllerReadBuffer::Get(i).ReadFrom(&controller_mmio_).byte();
    }
  }

  return zx::ok();
}

zx::result<fbl::Vector<uint8_t>> HdmiTransmitterControllerImpl::ReadExtendedEdid() {
  fbl::Vector<uint8_t> base_edid;
  fbl::AllocChecker alloc_checker;
  base_edid.resize(edid::kBlockSize, &alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for base EDID");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx::result<> base_edid_result =
      ReadEdidBlock(0, std::span<uint8_t, edid::kBlockSize>(base_edid.data(), edid::kBlockSize));
  if (base_edid_result.is_error()) {
    fdf::error("Failed to read EDID base block: {}", base_edid_result.status_string());
    return base_edid_result.take_error();
  }

  // VESA Enhanced Extended Display Identification Data (E-EDID) Standard,
  // Release A, Revision 2, dated September 25, 2006, revised December 31, 2020.
  // Section 3.1 "EDID Format Overview", page 19.
  static constexpr int kBaseEdidExtensionBlockCountOffset = 126;
  const int extension_block_count = base_edid[kBaseEdidExtensionBlockCountOffset];

  fbl::Vector<uint8_t> extended_edid = std::move(base_edid);
  const size_t extended_edid_size =
      static_cast<size_t>(extension_block_count + 1) * edid::kBlockSize;
  extended_edid.resize(extended_edid_size, 0, &alloc_checker);

  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate {} bytes for E-EDID", extended_edid_size);
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  std::ranges::copy(base_edid, extended_edid.begin());

  for (int extension_block_index = 1; extension_block_index <= extension_block_count;
       ++extension_block_index) {
    int extended_block_offset = extension_block_index * static_cast<int>(edid::kBlockSize);
    std::span<uint8_t, edid::kBlockSize> extended_block(
        extended_edid.begin() + extended_block_offset, edid::kBlockSize);
    zx::result<> extension_block_result = ReadEdidBlock(extension_block_index, extended_block);
    if (extension_block_result.is_error()) {
      fdf::error("Failed to read EDID extension block #{}: {}", extension_block_index,
                 extension_block_result.status_string());
      return extension_block_result.take_error();
    }
  }

  return zx::ok(std::move(extended_edid));
}

#define PRINT_REG(name) PrintReg(#name, (name))
void HdmiTransmitterControllerImpl::PrintReg(const char* name, uint32_t address) {
  fdf::info("{} (0x{:4x}): {}", name, address, ReadReg(address));
}

void HdmiTransmitterControllerImpl::PrintRegisters() {
  fdf::info("------------HdmiDw Registers------------");

  PRINT_REG(HDMITX_DWC_A_APIINTCLR);
  PRINT_REG(HDMITX_DWC_CSC_CFG);
  PRINT_REG(HDMITX_DWC_CSC_COEF_A1_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_A1_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_A2_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_A2_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_A3_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_A3_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_A4_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_A4_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_B1_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_B1_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_B2_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_B2_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_B3_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_B3_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_B4_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_B4_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_C1_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_C1_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_C2_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_C2_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_C3_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_C3_LSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_C4_MSB);
  PRINT_REG(HDMITX_DWC_CSC_COEF_C4_LSB);
  PRINT_REG(HDMITX_DWC_CSC_SCALE);
  PRINT_REG(HDMITX_DWC_FC_ACTSPC_HDLR_CFG);
  PRINT_REG(HDMITX_DWC_FC_AVICONF0);
  PRINT_REG(HDMITX_DWC_FC_AVICONF1);
  PRINT_REG(HDMITX_DWC_FC_AVICONF2);
  PRINT_REG(HDMITX_DWC_FC_AVICONF3);
  PRINT_REG(HDMITX_DWC_FC_CTRLDUR);
  PRINT_REG(HDMITX_DWC_FC_EXCTRLDUR);
  PRINT_REG(HDMITX_DWC_FC_EXCTRLSPAC);
  PRINT_REG(HDMITX_DWC_FC_GCP);
  PRINT_REG(HDMITX_DWC_FC_HSYNCINDELAY0);
  PRINT_REG(HDMITX_DWC_FC_HSYNCINDELAY1);
  PRINT_REG(HDMITX_DWC_FC_HSYNCINWIDTH0);
  PRINT_REG(HDMITX_DWC_FC_HSYNCINWIDTH1);
  PRINT_REG(HDMITX_DWC_FC_INHACTV0);
  PRINT_REG(HDMITX_DWC_FC_INHACTV1);
  PRINT_REG(HDMITX_DWC_FC_INHBLANK0);
  PRINT_REG(HDMITX_DWC_FC_INHBLANK1);
  PRINT_REG(HDMITX_DWC_FC_INVACTV0);
  PRINT_REG(HDMITX_DWC_FC_INVACTV1);
  PRINT_REG(HDMITX_DWC_FC_INVACT_2D_0);
  PRINT_REG(HDMITX_DWC_FC_INVACT_2D_1);
  PRINT_REG(HDMITX_DWC_FC_INVBLANK);
  PRINT_REG(HDMITX_DWC_FC_INVIDCONF);
  PRINT_REG(HDMITX_DWC_FC_MASK0);
  PRINT_REG(HDMITX_DWC_FC_MASK1);
  PRINT_REG(HDMITX_DWC_FC_MASK2);
  PRINT_REG(HDMITX_DWC_FC_PRCONF);
  PRINT_REG(HDMITX_DWC_FC_SCRAMBLER_CTRL);
  PRINT_REG(HDMITX_DWC_FC_VSYNCINDELAY);
  PRINT_REG(HDMITX_DWC_FC_VSYNCINWIDTH);
  PRINT_REG(HDMITX_DWC_HDCP22REG_STAT);
  PRINT_REG(HDMITX_DWC_I2CM_CTLINT);
  PRINT_REG(HDMITX_DWC_I2CM_DIV);
  PRINT_REG(HDMITX_DWC_I2CM_FS_SCL_HCNT_1);
  PRINT_REG(HDMITX_DWC_I2CM_FS_SCL_HCNT_0);
  PRINT_REG(HDMITX_DWC_I2CM_FS_SCL_LCNT_1);
  PRINT_REG(HDMITX_DWC_I2CM_FS_SCL_LCNT_0);
  PRINT_REG(HDMITX_DWC_I2CM_INT);
  PRINT_REG(HDMITX_DWC_I2CM_SDA_HOLD);
  PRINT_REG(HDMITX_DWC_I2CM_SCDC_UPDATE);
  PRINT_REG(HDMITX_DWC_I2CM_SS_SCL_HCNT_1);
  PRINT_REG(HDMITX_DWC_I2CM_SS_SCL_HCNT_0);
  PRINT_REG(HDMITX_DWC_I2CM_SS_SCL_LCNT_1);
  PRINT_REG(HDMITX_DWC_I2CM_SS_SCL_LCNT_0);
  PRINT_REG(HDMITX_DWC_IH_AS_STAT0);
  PRINT_REG(HDMITX_DWC_IH_CEC_STAT0);
  PRINT_REG(HDMITX_DWC_IH_FC_STAT0);
  PRINT_REG(HDMITX_DWC_IH_FC_STAT1);
  PRINT_REG(HDMITX_DWC_IH_FC_STAT2);
  PRINT_REG(HDMITX_DWC_IH_I2CM_STAT0);
  PRINT_REG(HDMITX_DWC_IH_I2CMPHY_STAT0);
  PRINT_REG(HDMITX_DWC_IH_MUTE);
  PRINT_REG(HDMITX_DWC_IH_MUTE_AS_STAT0);
  PRINT_REG(HDMITX_DWC_IH_MUTE_CEC_STAT0);
  PRINT_REG(HDMITX_DWC_IH_MUTE_FC_STAT0);
  PRINT_REG(HDMITX_DWC_IH_MUTE_FC_STAT1);
  PRINT_REG(HDMITX_DWC_IH_MUTE_FC_STAT2);
  PRINT_REG(HDMITX_DWC_IH_MUTE_I2CM_STAT0);
  PRINT_REG(HDMITX_DWC_IH_MUTE_I2CMPHY_STAT0);
  PRINT_REG(HDMITX_DWC_IH_MUTE_PHY_STAT0);
  PRINT_REG(HDMITX_DWC_IH_MUTE_VP_STAT0);
  PRINT_REG(HDMITX_DWC_IH_PHY_STAT0);
  PRINT_REG(HDMITX_DWC_IH_VP_STAT0);
  PRINT_REG(HDMITX_DWC_MC_FLOWCTRL);
  PRINT_REG(HDMITX_DWC_MC_SWRSTZREQ);
  PRINT_REG(HDMITX_DWC_MC_CLKDIS);
  PRINT_REG(HDMITX_DWC_TX_INVID0);
  PRINT_REG(HDMITX_DWC_TX_INSTUFFING);
  PRINT_REG(HDMITX_DWC_TX_GYDATA0);
  PRINT_REG(HDMITX_DWC_TX_GYDATA1);
  PRINT_REG(HDMITX_DWC_TX_RCRDATA0);
  PRINT_REG(HDMITX_DWC_TX_RCRDATA1);
  PRINT_REG(HDMITX_DWC_TX_BCBDATA0);
  PRINT_REG(HDMITX_DWC_TX_BCBDATA1);
  PRINT_REG(HDMITX_DWC_VP_CONF);
  PRINT_REG(HDMITX_DWC_VP_MASK);
  PRINT_REG(HDMITX_DWC_VP_PR_CD);
  PRINT_REG(HDMITX_DWC_VP_REMAP);
  PRINT_REG(HDMITX_DWC_VP_STUFF);
}
#undef PRINT_REG

}  // namespace designware_hdmi
