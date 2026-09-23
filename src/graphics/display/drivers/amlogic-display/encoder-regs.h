// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_ENCODER_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_ENCODER_REGS_H_

#include <zircon/assert.h>

#include <cstdint>

#include <hwreg/bitfields.h>

#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace amlogic_display {

// VENC_VIDEO_TST_EN
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 377.
class EncoderBuiltInSelfTestEnabled
    : public hwreg::RegisterBase<EncoderBuiltInSelfTestEnabled, uint32_t> {
 public:
  static hwreg::RegisterAddr<EncoderBuiltInSelfTestEnabled> Get() {
    return {0x1b70 * sizeof(uint32_t)};
  }

  // These bits are undocumented on Amlogic datasheets.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 1);

  // If true, the encoder built-in self-test (BIST) mode is enabled, and the
  // encoder outputs predefined patterns as specified in the
  // EncoderBuiltInSelfTestModeSelection register.
  DEF_BIT(0, enabled);
};

enum class EncoderBuiltInSelfTestMode : uint8_t {
  // Outputs a fixed color specified by the following registers:
  // - EncoderBuiltInSelfTestFixedColorLuminance
  // - EncoderBuiltInSelfTestFixedColorChrominanceBlue
  // - EncoderBuiltInSelfTestFixedColorChrominanceRed
  kFixedColor = 0,

  // Outputs color bars with 100% and 75% luminance (intensity), also known as
  // 100/75 color bars.
  kColorBar = 1,

  // Outputs thin horizontal and vertical lines on the screen.
  kThinLines = 2,

  // Outputs a grid of white dots on the display.
  kDotGrid = 3,
};

// VENC_VIDEO_TST_MDSEL
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", pages 377-378.
class EncoderBuiltInSelfTestModeSelection
    : public hwreg::RegisterBase<EncoderBuiltInSelfTestModeSelection, uint32_t> {
 public:
  static hwreg::RegisterAddr<EncoderBuiltInSelfTestModeSelection> Get() {
    return {0x1b71 * sizeof(uint32_t)};
  }

  // These bits are undocumented on Amlogic datasheets.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 8);

  DEF_ENUM_FIELD(EncoderBuiltInSelfTestMode, 7, 0, mode);
};

class EncoderBuiltInSelfTestFixedColorLuminance
    : public hwreg::RegisterBase<EncoderBuiltInSelfTestFixedColorLuminance, uint32_t> {
 public:
  static hwreg::RegisterAddr<EncoderBuiltInSelfTestFixedColorLuminance> Get() {
    return {0x1b72 * sizeof(uint32_t)};
  }

  // These bits are undocumented on Amlogic datasheets.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 10);

  DEF_FIELD(9, 0, luminance);
};

// VENC_VIDEO_TST_CB
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", pages 378.
class EncoderBuiltInSelfTestFixedColorChrominanceBlue
    : public hwreg::RegisterBase<EncoderBuiltInSelfTestFixedColorChrominanceBlue, uint32_t> {
 public:
  static hwreg::RegisterAddr<EncoderBuiltInSelfTestFixedColorChrominanceBlue> Get() {
    return {0x1b73 * sizeof(uint32_t)};
  }

  // These bits are undocumented on Amlogic datasheets.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 10);

  DEF_FIELD(9, 0, chrominance_blue);
};

// VENC_VIDEO_TST_CR
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", pages 378.
class EncoderBuiltInSelfTestFixedColorChrominanceRed
    : public hwreg::RegisterBase<EncoderBuiltInSelfTestFixedColorChrominanceRed, uint32_t> {
 public:
  static hwreg::RegisterAddr<EncoderBuiltInSelfTestFixedColorChrominanceRed> Get() {
    return {0x1b74 * sizeof(uint32_t)};
  }

  // These bits are undocumented on Amlogic datasheets.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 10);

  DEF_FIELD(9, 0, chrominance_red);
};

// The code references the S912 datasheet for the encoder control registers.
//
// These registers are not documented in the datasheets of the newer SoCs
// (S905D2, A311D / T931 and S905D3). Experiments on Astro (using S905D2),
// Sherlock (using T931), Nelson (using S905D3) and Khadas VIM3 (using A311D)
// boards show that these registers have the same definition and register
// addresses as S912.

// # Amlogic Display Encoders
//
// An encoder converts the image data processed by the Video Input Unit to
// video and timing signals.
//
// The following encoders are available on an Amlogic SoC:
//
// * ENCP (HDMI / DVI encoder)
//
//   Encodes the image data for digital output over HDMI / DVI.
//
//   While the name ENCP stands for "Progressive encoder", it actually supports
//   both progressive and interlaced (1080i) display timings.
//
// * ENCI (Standard-definition interlaced HDMI / DVI encoder)
//
//   Encodes the image data to 480i / 576i (standard definition) for digital
//   output over HDMI / DVI.
//
// * ENCL (LCD panel encoder)
//
//   Encodes the image data for MIPI-DSI host controller and / or LCD timing
//   controllers (TCON).
//
// * ENCT (TV panel encoder)
//
//   Encodes the image data for composite video baseband signal (CVBS) output.

// ENCI_VIDEO_EN (Interlace Video Enable)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 372.
class InterlacedHdmiEncoderEnabled
    : public hwreg::RegisterBase<InterlacedHdmiEncoderEnabled, uint32_t> {
 public:
  static hwreg::RegisterAddr<InterlacedHdmiEncoderEnabled> Get() {
    return {0x1b57 * sizeof(uint32_t)};
  }

  // These bits are undocumented on Amlogic datasheets.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 1);

  DEF_BIT(0, enabled);
};

// ## ENCP (HDMI / DVI encoder)
//
// The HDMI / DVI encoder takes its input from the VIU FIFO (VFIFO) and the
// HDMI Pixel Clock (from the clock tree), and generates the following signals
// for the HDMI / DVI transmitter:
//
// - Video signal, a parallel 24-bit wide RGB / YUV color signal (for non-HDR
//   displays).
// - Display Enabled (DE) signal
// - Horizontal Sync (HSYNC) signal
// - Vertical Sync (VSYNC) signal
//
// All the signals are synchronized to the Pixel Clock.
//
// The HDMI / DVI encoder consists of two parts:
// - The video signal encoder, converting the VFIFO bytes to the video signal.
// - The timing signal producer, producing DE, HSYNC and VSYNC signals.

// ENCP_VIDEO_EN (Progressive Video Enable)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 379.
class HdmiEncoderEnabled : public hwreg::RegisterBase<HdmiEncoderEnabled, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderEnabled> Get() { return {0x1b80 * sizeof(uint32_t)}; }

  // These bits are undocumented on Amlogic datasheets.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 1);

  DEF_BIT(0, enabled);
};

// ENCP_VIDEO_MODE
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", pages 380-381.
class HdmiEncoderModeConfig : public hwreg::RegisterBase<HdmiEncoderModeConfig, uint32_t> {
 public:
  enum class DisplayEnabledSignalPolarity : uint8_t {
    kActiveLow = 0,
    kActiveHigh = 1,
  };

  static hwreg::RegisterAddr<HdmiEncoderModeConfig> Get() { return {0x1b8d * sizeof(uint32_t)}; }

  // These bits are undocumented on Amlogic datasheets.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 16);

  // If true, the ENCP encoder debug counter registers are enabled.
  //
  // Documented as "pixel count and line count shadow enable" in the S912
  // datasheet.
  DEF_BIT(15, debug_counter_enabled);

  DEF_ENUM_FIELD(DisplayEnabledSignalPolarity, 14, 14, display_enabled_signal_polarity);

  // If true, the encoder increases the value of the horizontal period (maximum
  // pixel counter) register by one internally.
  DEF_BIT(13, horizontal_period_increases_by_one);

  // Bits 12-0 configure the timing signals for the interlaced output.
  //
  // Rather than directly manipulating these bits, it's preferred to use the
  // helper methods: ConfigureFor1080i(), ConfigureForHighDefinitionProgressive()
  // and ConfigureForStandardDefinitionProgressive().

  DEF_BIT(12, is_output_interlaced);

  // True iff the Vertical Sync Offset for odd field registers are enabled to
  // produce vertical sync signals for odd fields.
  DEF_BIT(11, vertical_sync_offset_for_odd_field_enabled);

  // True iff the Vertical Active On for odd field registers are enabled to
  // produce video signals for odd fields.
  DEF_BIT(10, vertical_active_on_for_odd_field_enabled);

  // Equalizing pulses are narrow pulses added before (pre-equalizing) and
  // after (post-equalizing) the vertical sync pulses (also known as "broad
  // pulses" in Analog TV) to indicate the start of a field.
  //
  // This avoids premature vertical-deflections when the vertical and
  // horizontal syncs are combined into a single sync signal, which is common
  // in Analog TV [1]. The equalizing pulse is also used to indicate the
  // start of a field in 525/59.94 (NTSC) TV system [2].
  //
  // Digital signals don't require equalizing pulses.
  //
  // [1] Raster Graphics Handbook, Chapter 8 "The Monitor Interface",
  //     Conrac Corporation, 1985, pages 8-9 and 8-10.
  // [2] A Technical Introduction to Digital Video, Chapter 1 "Basic
  //     Principles", Charles Poynton, 1996, page 12.

  // True iff the equalizing pulse generator is enabled to produce analog
  // equalizing pulse signals for odd fields in interlaced outputs.
  DEF_BIT(9, analog_equalizing_pulse_for_odd_field_enabled);

  // True iff the equalizing pulse generator is enabled to produce analog
  // equalizing pulse signals for even fields in interlaced outputs, or for
  // progressive outputs.
  DEF_BIT(8, analog_equalizing_pulse_enabled);

  // Undocumented in the S912 datasheet.
  //
  // The S912 datasheet requires this bit set to 1 for 1080i output, and
  // requires this bit set to 0 for 720p, 480p and 540p output.
  DEF_BIT(7, bit7_undocumented);

  // Documented as "Enable Hsync and equalization pulse switch in center" in
  // the S912 datasheet. The function of this bit is unclear; it seems it
  // only works for the analog signal output.
  DEF_BIT(6, analog_hsync_and_equalizing_pulse_switch_in_center);

  // True iff the Analog vertical sync signal generator is enabled to produce
  // analog Vsync signals for odd fields.
  DEF_BIT(5, analog_vertical_sync_for_odd_field_enabled);

  // Documented as "Enable 2nd vertical pulse in a line (1080i)" in the
  // S912 datasheet. The function of this bit is unclear.
  DEF_BIT(4, second_vertical_sync_in_a_line_enabled);

  // Bits 3-0 configures the analog signal generator to handle the timings with
  // non-integral values.
  //
  // The SMPTE 274M-2008 Standard requires that, for the 1080i output with
  // a total of 1125 lines per picture, the digital interface may output 563
  // and 562 lines in even and odd fields respectively, while the analog
  // interface must remain equally spaced (562 1/2 lines) for each field and
  // must align to the half of a line.
  //
  // Section 7 "Raster Structure", SMPTE Standard 274M-2008: For Television --
  // 1920 x 1080 Image Sample Structure, Digital Representation and Digital
  // Timing Reference Sequences for Multiple Picture Rates, The Society of
  // Motion Picture and Television Engineers, Jan 2008, page 6.

  // True iff the end of the vertical sync is aligned to the half of a line
  // for odd fields.
  DEF_BIT(3, analog_vertical_sync_end_for_odd_field_aligned_to_half_line);

  // True iff the start of the vertical sync is aligned to the half of a line
  // for odd fields.
  DEF_BIT(2, analog_vertical_sync_start_for_odd_field_aligned_to_half_line);

  // True iff the end of the vertical sync is aligned to the half of a line
  // for even fields.
  DEF_BIT(1, analog_vertical_sync_end_for_even_field_aligned_to_half_line);

  // True iff the start of the vertical sync is aligned to the half of a line
  // for even fields.
  DEF_BIT(0, analog_vertical_sync_start_for_even_field_aligned_to_half_line);

  // The following register field configuration matches the preferred register
  // value provided in the S912 datasheet.
  HdmiEncoderModeConfig& ConfigureFor1080i() {
    return set_is_output_interlaced(true)
        .set_vertical_sync_offset_for_odd_field_enabled(true)
        .set_vertical_active_on_for_odd_field_enabled(true)
        .set_analog_equalizing_pulse_for_odd_field_enabled(true)
        .set_analog_equalizing_pulse_enabled(true)
        .set_bit7_undocumented(true)
        .set_analog_hsync_and_equalizing_pulse_switch_in_center(true)
        .set_analog_vertical_sync_for_odd_field_enabled(true)
        .set_second_vertical_sync_in_a_line_enabled(true)
        .set_analog_vertical_sync_end_for_odd_field_aligned_to_half_line(true)
        .set_analog_vertical_sync_start_for_odd_field_aligned_to_half_line(true)
        .set_analog_vertical_sync_end_for_even_field_aligned_to_half_line(false)
        .set_analog_vertical_sync_start_for_even_field_aligned_to_half_line(false);
  }

  // The following register field configuration matches the preferred register
  // value provided in the Amlogic-provided code (except for the DE signal
  // polarity, which the drivers should set by themselves for consistency with
  // the transmitter configuration).
  //
  // The datasheets and the Amlogic-provided code deviate in some register field
  // configurations: The S912 datasheet mentions that the register should be
  // set to 0x140 for 720p, while the Amlogic-provided code use 0x4040 for all
  // progressive outputs.
  HdmiEncoderModeConfig& ConfigureForHighDefinitionProgressive() {
    return set_is_output_interlaced(false)
        .set_vertical_sync_offset_for_odd_field_enabled(false)
        .set_vertical_active_on_for_odd_field_enabled(false)
        .set_analog_equalizing_pulse_for_odd_field_enabled(false)
        .set_analog_equalizing_pulse_enabled(false)
        .set_bit7_undocumented(false)
        .set_analog_hsync_and_equalizing_pulse_switch_in_center(true)
        .set_analog_vertical_sync_for_odd_field_enabled(false)
        .set_second_vertical_sync_in_a_line_enabled(false)
        .set_analog_vertical_sync_end_for_odd_field_aligned_to_half_line(false)
        .set_analog_vertical_sync_start_for_odd_field_aligned_to_half_line(false)
        .set_analog_vertical_sync_end_for_even_field_aligned_to_half_line(false)
        .set_analog_vertical_sync_start_for_even_field_aligned_to_half_line(false);
  }

  // The following register field configuration matches the preferred register
  // value provided in the S912 datasheet for 480p or 576p.
  HdmiEncoderModeConfig& ConfigureForStandardDefinitionProgressive() {
    return set_is_output_interlaced(false)
        .set_vertical_sync_offset_for_odd_field_enabled(false)
        .set_vertical_active_on_for_odd_field_enabled(false)
        .set_analog_equalizing_pulse_for_odd_field_enabled(false)
        .set_analog_equalizing_pulse_enabled(false)
        .set_bit7_undocumented(false)
        .set_analog_hsync_and_equalizing_pulse_switch_in_center(false)
        .set_analog_vertical_sync_for_odd_field_enabled(false)
        .set_second_vertical_sync_in_a_line_enabled(false)
        .set_analog_vertical_sync_end_for_odd_field_aligned_to_half_line(false)
        .set_analog_vertical_sync_start_for_odd_field_aligned_to_half_line(false)
        .set_analog_vertical_sync_end_for_even_field_aligned_to_half_line(false)
        .set_analog_vertical_sync_start_for_even_field_aligned_to_half_line(false);
  }
};

// ENCP_VIDEO_MODE_ADV
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 382.
class HdmiEncoderAdvancedModeConfig
    : public hwreg::RegisterBase<HdmiEncoderAdvancedModeConfig, uint32_t> {
 public:
  // Selection of the downsampling multiplier (reciprocal of the downsampling
  // ratio) from the VIU to the VIU FIFO (VFIFO).
  //
  // A downsampling multiplier of N means that the VIU FIFO only takes one of
  // every N pixels from the VIU's output.
  enum class ViuFifoDownsamplingMultiplierSelection : uint8_t {
    k1 = 0,
    k2 = 1,
    k4 = 2,
    k8 = 3,
  };
  static hwreg::RegisterAddr<HdmiEncoderAdvancedModeConfig> Get() {
    return {0x1b8e * sizeof(uint32_t)};
  }

  // This field is undocumented on Amlogic datasheets.
  // Amlogic-provided code directly writes 0 to this field regardless of its
  // original value, so we can believe that zero is a safe setting for this
  // field.
  DEF_RSVDZ_FIELD(31, 16);

  DEF_FIELD(15, 14, sp_timing_control);

  DEF_BIT(13, cr_bypasses_limiter);
  DEF_BIT(12, cb_bypasses_limiter);
  DEF_BIT(11, y_bypasses_limiter);
  DEF_BIT(10, gamma_rgb_input_selection);

  // This field is undocumented on Amlogic datasheets.
  // Amlogic-provided code directly writes 0 to this field regardless of its
  // original value, so we can believe that zero is a safe setting for this
  // field.
  DEF_RSVDZ_FIELD(9, 8);

  // True iff the hue adjustment matrix (controlled by ENCP_VIDEO_MATRIX_CB
  // and ENCP_VIDEO_MATRIX_CR) is enabled.
  DEF_BIT(7, hue_matrix_enabled);

  // Bits 6-4 are related to YPbPr (analog output) which seem to be unused
  // for the HDMI encoder.
  DEF_BIT(6, pb_pr_swapped);
  DEF_BIT(5, pb_pr_hsync_enabled);
  DEF_BIT(4, ypbpr_gain_as_hdtv_type);

  // True iff the VIU FIFO (VFIFO) which samples the VIU output pixels is
  // enabled.
  DEF_BIT(3, viu_fifo_enabled);

  // It's preferred to use the `viu_fifo_downsampling_multiplier()` and
  // `set_viu_fifo_downsampling_multiplier()` helper methods.
  DEF_ENUM_FIELD(ViuFifoDownsamplingMultiplierSelection, 2, 0,
                 viu_fifo_downsampling_multiplier_selection);

  int viu_fifo_downsampling_multiplier() const {
    switch (viu_fifo_downsampling_multiplier_selection()) {
      case ViuFifoDownsamplingMultiplierSelection::k1:
        return 1;
      case ViuFifoDownsamplingMultiplierSelection::k2:
        return 2;
      case ViuFifoDownsamplingMultiplierSelection::k4:
        return 4;
      case ViuFifoDownsamplingMultiplierSelection::k8:
        return 8;
    }
  }

  // `multiplier` must be 1, 2, 4 or 8.
  HdmiEncoderAdvancedModeConfig& set_viu_fifo_downsampling_multiplier(int multiplier) {
    switch (multiplier) {
      case 1:
        return set_viu_fifo_downsampling_multiplier_selection(
            ViuFifoDownsamplingMultiplierSelection::k1);
      case 2:
        return set_viu_fifo_downsampling_multiplier_selection(
            ViuFifoDownsamplingMultiplierSelection::k2);
      case 4:
        return set_viu_fifo_downsampling_multiplier_selection(
            ViuFifoDownsamplingMultiplierSelection::k4);
      case 8:
        return set_viu_fifo_downsampling_multiplier_selection(
            ViuFifoDownsamplingMultiplierSelection::k8);
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid downsampling multiplier: %d", multiplier);
    return *this;
  }
};

// ENCP_VIDEO_MAX_PXCNT (Max pixel counter)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 382.
class HdmiEncoderHorizontalTotal
    : public hwreg::RegisterBase<HdmiEncoderHorizontalTotal, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderHorizontalTotal> Get() {
    return {0x1b97 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // The horizontal period is `pixels_minus_one + 1` pixels.
  //
  // ENCP horizontal pixel registers must be set to values within the range
  // [0..pixels_minus_one].
  //
  // For a horizontal range of pixels [start..end]:
  // - If start <= end, the pixels are within the same line.
  // - If start > end, the pixels [start..pixels_minus_one] are
  //   on one line, while pixels [0..end] are rolled over to the next line.
  //
  // It's preferred to use `pixels()` and `set_pixels()` helper methods.
  DEF_FIELD(12, 0, pixels_minus_one);

  int pixels() const { return static_cast<int>(pixels_minus_one()) + 1; }

  // `pixels` must be within [1, 2^13].
  HdmiEncoderHorizontalTotal& set_pixels(int pixels) {
    ZX_DEBUG_ASSERT(pixels >= 1);
    ZX_DEBUG_ASSERT(pixels <= (1 << 13));
    return set_pixels_minus_one(pixels - 1);
  }
};

// The duration of the horizontal active (pixels) part of the video signal for
// each line.
//
// This register only configures the timing of the video signal output;
// the DE (data enabled) signal produced for the HDMI transmitter is
// controlled by the `HdmiEncoderDataEnableHorizontalActiveStart` and
// `HdmiEncoderDataEnableHorizontalActiveEnd` register pair.

// ENCP_VIDEO_HAVON_BEGIN (Horizontal active video start point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoHorizontalActiveStart
    : public hwreg::RegisterBase<HdmiEncoderVideoHorizontalActiveStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoHorizontalActiveStart> Get() {
    return {0x1ba4 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // Start point (inclusive) of active video signals for each line, in pixels.
  DEF_FIELD(12, 0, pixels);
};

// ENCP_VIDEO_HAVON_END (Horizontal active video end point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoHorizontalActiveEnd
    : public hwreg::RegisterBase<HdmiEncoderVideoHorizontalActiveEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoHorizontalActiveEnd> Get() {
    return {0x1ba3 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // End point (inclusive) of active video signals for each line, in pixels.
  DEF_FIELD(12, 0, pixels);
};

// The duration of the horizontal sync (blank) part of the video signal for
// each line.
//
// This register only configures the timing of the video signal output;
// the actual HSYNC signal produced for the HDMI transmitter is controlled by
// the `HdmiEncoderHorizontalSyncStart` and `HdmiEncoderHorizontalSyncEnd`
// register pair.

// ENCP_VIDEO_HSO_BEGIN (Digital Hsync out start point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoHorizontalSyncStart
    : public hwreg::RegisterBase<HdmiEncoderVideoHorizontalSyncStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoHorizontalSyncStart> Get() {
    return {0x1ba7 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // Start point (inclusive) of the horizontal sync part of the video signal
  // for each line, in pixels.
  DEF_FIELD(12, 0, pixels);
};

// ENCP_VIDEO_HSO_END (Digital Hsync out end point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoHorizontalSyncEnd
    : public hwreg::RegisterBase<HdmiEncoderVideoHorizontalSyncEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoHorizontalSyncEnd> Get() {
    return {0x1ba8 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // One (1) plus the end point (inclusive) of the horizontal sync part of the
  // video signal for each line, in pixels.
  //
  // The register is originally named "hsync out end point", and the end point
  // is exclusive from the range. For consistency with other register pairs, we
  // rename the field to its current name.
  //
  // It's preferred to use `pixels()` and `set_pixels()` helper methods to
  // get / set its value.
  DEF_FIELD(12, 0, pixels_plus_one);

  int pixels() const { return static_cast<int>(pixels_plus_one()) - 1; }

  // `pixels` must be within [0, 2^13 - 1].
  HdmiEncoderVideoHorizontalSyncEnd& set_pixels(int pixels) {
    ZX_DEBUG_ASSERT(pixels >= 0);
    ZX_DEBUG_ASSERT(pixels <= (1 << 13) - 1);
    return set_pixels_plus_one(pixels + 1);
  }
};

// ENCP_VIDEO_MAX_LNCNT (Max line counter)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", pages 383-384.
class HdmiEncoderVerticalTotal : public hwreg::RegisterBase<HdmiEncoderVerticalTotal, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVerticalTotal> Get() { return {0x1bae * sizeof(uint32_t)}; }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // In the S912 datasheet, only bits 10-0 are used.
  //
  // Given that the S912 SoC (and other SoCs that use the same encoder design,
  // including S905D2, S905D3 and A311D) supports 3840 x 2160 output, bit 11
  // should also be supported as well.
  //
  // Experiments on A311D shows that the bit 11 is also supported, thus we use
  // bits 11-0 for the field.

  // The vertical period is set to `lines_minus_one + 1` lines.
  //
  // ENCP vertical line registers must be set to values within the range
  // [0..lines_minus_one].
  //
  // For a vertical range of lines [start..end]:
  // - If start <= end, the lines are within the same frame.
  // - If start > end, the lines [start..pixels_minus_one] are
  //   on one frame, while lines [0..end] are rolled over to the next frame.
  //
  // It's preferred to use `lines()` and `set_lines()` methods to get / set
  // its value.
  DEF_FIELD(11, 0, lines_minus_one);

  int pixels() const { return static_cast<int>(lines_minus_one()) + 1; }

  // `lines` must be within [1, 2^12].
  HdmiEncoderVerticalTotal& set_lines(int lines) {
    ZX_DEBUG_ASSERT(lines >= 1);
    ZX_DEBUG_ASSERT(lines <= (1 << 12));
    return set_lines_minus_one(lines - 1);
  }
};

// The duration of the vertical active part of the video signal for each frame
// (for progressive output), or for each even field (for interlaced output).
//
// This register only configures the timing of the video signal output;
// the DE (display enabled) signal produced for the HDMI transmitter is
// controlled by the `HdmiEncoderDataEnableVerticalActiveStart` and
// `HdmiEncoderDataEnableVerticalActiveEnd` register pair.

// ENCP_VIDEO_VAVON_BLINE (Vertical active video start line)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoVerticalActiveStart
    : public hwreg::RegisterBase<HdmiEncoderVideoVerticalActiveStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoVerticalActiveStart> Get() {
    return {0x1ba6 * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // On the S912 datasheet, only bits 10-0 are used.
  //
  // Given that the S912 SoC (and other SoCs that use the same encoder design,
  // including S905D2, S905D3 and A311D) supports 3840 x 2160 output, the bit
  // 11 should also be supported as well.
  //
  // Experiments on A311D shows that the bit 11 is also supported, thus we use
  // bits 11-0 for the field.

  // Start line (inclusive) of active video signals for each field, in lines.
  DEF_FIELD(11, 0, lines);
};

// ENCP_VIDEO_VAVON_ELINE (Vertical active video end line)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoVerticalActiveEnd
    : public hwreg::RegisterBase<HdmiEncoderVideoVerticalActiveEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoVerticalActiveEnd> Get() {
    return {0x1baf * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // On the S912 datasheet, only bits 10-0 are used.
  //
  // Given that the S912 SoC (and other SoCs that use the same encoder design,
  // including S905D2, S905D3 and A311D) supports 3840 x 2160 output, the bit
  // 11 should also be supported as well.
  //
  // Experiments on A311D shows that the bit 11 is also supported, thus we use
  // bits 11-0 for the field.

  // End line (inclusive) of active video signals for each field, in lines.
  DEF_FIELD(11, 0, lines);
};

// The duration of the vertical sync pulse in the video signal for each frame.
//
// This register only configures the timing of the video signal output;
// the actual VSYNC signal produced for the HDMI transmitter is controlled by
// the `HdmiEncoderVerticalSyncStart` and `HdmiEncoderVerticalSyncEnd`
// register pair.

// ENCP_VIDEO_VSO_BLINE (Digital Vsync out start point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoVerticalSyncStart
    : public hwreg::RegisterBase<HdmiEncoderVideoVerticalSyncStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoVerticalSyncStart> Get() {
    return {0x1bab * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // Start line (inclusive) of the Vertical sync part of the video signal
  // for each field, in lines.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  DEF_FIELD(11, 0, lines);
};

// ENCP_VIDEO_VSO_ELINE (Digital Vsync out end line)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoVerticalSyncEnd
    : public hwreg::RegisterBase<HdmiEncoderVideoVerticalSyncEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoVerticalSyncEnd> Get() {
    return {0x1bac * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // One (1) plus the end line (inclusive) of the Vertical sync part of the
  // video signal for each line, in pixels.
  //
  // The register is originally named "hsync out end point", and the end point
  // is exclusive from the range. For consistency with other register pairs, we
  // rename the field to its current name.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  //
  // It's preferred to use `lines()` and `set_lines()`
  // helper methods to get / set its value.
  DEF_FIELD(11, 0, lines_plus_one);

  int lines() const { return static_cast<int>(lines_plus_one()) - 1; }

  // `lines` must be within [0, 2^12 - 1].
  HdmiEncoderVideoVerticalSyncEnd& set_lines(int lines) {
    ZX_DEBUG_ASSERT(lines >= 0);
    ZX_DEBUG_ASSERT(lines <= (1 << 12) - 1);
    return set_lines_plus_one(lines + 1);
  }
};

// The horizontal start and end points for Vertical sync parts of the video
// signal for each line.
//
// The actual function of this register is unclear. Theoretically, it
// should not affect the output video signal, because during Vsync there should
// be no video signal at all regardless of the start/end horizontal points.
//
// Amlogic-provided code has multiple different configurations on this register
// and its corresponding `End` register. Experiment shows that start == 0 and
// end == 0 is a valid pair on Khadas VIM3 (using Amlogic A311D).

// ENCP_VIDEO_VSO_BEGIN (Digital Vsync out start point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoVerticalSyncHorizontalStart
    : public hwreg::RegisterBase<HdmiEncoderVideoVerticalSyncHorizontalStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoVerticalSyncHorizontalStart> Get() {
    return {0x1ba9 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // Start point of the horizontal part of the video signal
  // for each Vertical sync pulse on each line, in pixels.
  DEF_FIELD(12, 0, pixels);
};

// ENCP_VIDEO_VSO_END (Digital Vsync out end point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 383.
class HdmiEncoderVideoVerticalSyncHorizontalEnd
    : public hwreg::RegisterBase<HdmiEncoderVideoVerticalSyncHorizontalEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoVerticalSyncHorizontalEnd> Get() {
    return {0x1baa * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // End point of the horizontal sync part of the video signal for each
  // Vertical sync pulse on each line, in pixels.
  DEF_FIELD(12, 0, pixels);
};

// The duration of the horizontal active part of the DE (data enabled) timing
// signal for each line.
//
// This register only configures the DE timing signal produced for the
// HDMI transmitter; the timing of the video signal output is controlled by the
// `HdmiEncoderVideoHorizontalActiveStart` and `HdmiEncoderVideoHorizontalActiveEnd`
// register pair.

// ENCP_DE_H_BEGIN (DVI/HDMI horizontal active video start point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 392.
class HdmiEncoderDataEnableHorizontalActiveStart
    : public hwreg::RegisterBase<HdmiEncoderDataEnableHorizontalActiveStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderDataEnableHorizontalActiveStart> Get() {
    return {0x1c3a * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // Start point (inclusive) of horizontal active signals for each line, in
  // pixels.
  DEF_FIELD(12, 0, pixels);
};

// ENCP_DE_H_END (DVI/HDMI horizontal active video end point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 392.
class HdmiEncoderDataEnableHorizontalActiveEnd
    : public hwreg::RegisterBase<HdmiEncoderDataEnableHorizontalActiveEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderDataEnableHorizontalActiveEnd> Get() {
    return {0x1c3b * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // One (1) plus the end point (inclusive) of horizontal active signals for
  // each line, in pixels.
  //
  // It's preferred to use `pixels()` and `set_pixels()` helper methods to
  // get / set its value.
  DEF_FIELD(12, 0, pixels_plus_one);

  int pixels() const { return static_cast<int>(pixels_plus_one()) - 1; }

  // `pixels` must be within [0, 2^13 - 1].
  HdmiEncoderDataEnableHorizontalActiveEnd& set_pixels(int pixels) {
    ZX_DEBUG_ASSERT(pixels >= 0);
    ZX_DEBUG_ASSERT(pixels <= (1 << 13) - 1);
    return set_pixels_plus_one(pixels + 1);
  }
};

// The duration of the horizontal sync part of the HSYNC timing signal for each
// line.
//
// This register only configures the HSYNC timing signal produced for the
// HDMI transmitter; the timing of the video signal output is controlled by the
// `HdmiEncoderVideoHorizontalSyncStart` and `HdmiEncoderVideoHorizontalSyncEnd`
// register pair.

// ENCP_DVI_HSO_BEGIN (DVI/HDMI Hsync out start point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 391.
class HdmiEncoderHorizontalSyncStart
    : public hwreg::RegisterBase<HdmiEncoderHorizontalSyncStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderHorizontalSyncStart> Get() {
    return {0x1c30 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // Start point (inclusive) of the horizontal sync signal for each line, in
  // pixels.
  DEF_FIELD(12, 0, pixels);
};

// ENCP_VIDEO_HSO_END (Digital Hsync out end point)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 391.
class HdmiEncoderHorizontalSyncEnd
    : public hwreg::RegisterBase<HdmiEncoderHorizontalSyncEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderHorizontalSyncEnd> Get() {
    return {0x1c31 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // One (1) plus the end point (inclusive) of the horizontal sync signal for
  // each line, in pixels.
  //
  // It's preferred to use `pixels()` and `set_pixels()` helper methods to
  // get / set its value.
  DEF_FIELD(12, 0, pixels_plus_one);

  int pixels() const { return static_cast<int>(pixels_plus_one()) - 1; }

  // `pixels` must be within [0, 2^13 - 1].
  HdmiEncoderHorizontalSyncEnd& set_pixels(int pixels) {
    ZX_DEBUG_ASSERT(pixels >= 0);
    ZX_DEBUG_ASSERT(pixels <= (1 << 13) - 1);
    return set_pixels_plus_one(pixels + 1);
  }
};

// The duration of the vertical active part of the DE (data enabled) timing
// signal for each field (progressive) or for each even field (interlaced).
//
// This register only configures the DE timing signal produced for the
// HDMI transmitter; the timing of the video signal output is controlled by the
// `HdmiEncoderVideoVerticalActiveStart` and `HdmiEncoderVideoVerticalActiveEnd`
// register pair.

// ENCP_DE_V_BEGIN_EVEN (DVI/HDMI vertical active video start line for even
// field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 392.
class HdmiEncoderDataEnableVerticalActiveStart
    : public hwreg::RegisterBase<HdmiEncoderDataEnableVerticalActiveStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderDataEnableVerticalActiveStart> Get() {
    return {0x1c3c * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // Start line (inclusive) of vertical active signals for each field, in
  // lines.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  DEF_FIELD(11, 0, lines);
};

// ENCP_DE_V_END_EVEN (DVI/HDMI vertical active video end line for even field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 392.
class HdmiEncoderDataEnableVerticalActiveEnd
    : public hwreg::RegisterBase<HdmiEncoderDataEnableVerticalActiveEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderDataEnableVerticalActiveEnd> Get() {
    return {0x1c3d * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // One (1) plus the end line (inclusive) of vertical active signals for each
  // field, in lines.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  //
  // It's preferred to use `lines()` and `set_lines()` helper methods to
  // get / set its value.
  DEF_FIELD(11, 0, lines_plus_one);

  int lines() const { return static_cast<int>(lines_plus_one()) - 1; }

  // `vertical_active_end_line` must be within [0, 2^12 - 1].
  HdmiEncoderDataEnableVerticalActiveEnd& set_lines(int lines) {
    ZX_DEBUG_ASSERT(lines >= 0);
    ZX_DEBUG_ASSERT(lines <= (1 << 12) - 1);
    return set_lines_plus_one(lines + 1);
  }
};

// The duration of the vertical active (pixels) part of the DE (data enabled)
// timing signal for each odd field. Used for interlaced output only.

// ENCP_DE_V_BEGIN_ODD (DVI/HDMI vertical active video start line for odd field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 392.
class HdmiEncoderDataEnableVerticalActiveStartOddFields
    : public hwreg::RegisterBase<HdmiEncoderDataEnableVerticalActiveStartOddFields, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderDataEnableVerticalActiveStartOddFields> Get() {
    return {0x1c3e * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // Start line (inclusive) of vertical active signals for each field, in
  // lines.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  DEF_FIELD(11, 0, lines);
};

// ENCP_DE_V_END_ODD (DVI/HDMI vertical active video end line for odd field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 392.
class HdmiEncoderDataEnableVerticalActiveEndOddFields
    : public hwreg::RegisterBase<HdmiEncoderDataEnableVerticalActiveEndOddFields, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderDataEnableVerticalActiveEndOddFields> Get() {
    return {0x1c3f * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // One (1) plus the end line (inclusive) of vertical active signals for each
  // field, in lines.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  //
  // It's preferred to use `lines()` and `set_lines()` helper methods to
  // get / set its value.
  DEF_FIELD(11, 0, lines_plus_one);

  int lines() const { return static_cast<int>(lines_plus_one()) - 1; }

  // `lines` must be within [0, 2^12 - 1].
  HdmiEncoderDataEnableVerticalActiveEndOddFields& set_lines(int lines) {
    ZX_DEBUG_ASSERT(lines >= 0);
    ZX_DEBUG_ASSERT(lines <= (1 << 12) - 1);
    return set_lines_plus_one(lines + 1);
  }
};

// The duration of the vertical sync part of the VSYNC timing signal for each
// field (progressive) or each even field (interlaced).
//
// This register only configures the VSYNC timing signal produced for the
// HDMI transmitter; the timing of the video signal output is controlled by the
// `HdmiEncoderVideoVerticalSyncStart` and `HdmiEncoderVideoVerticalSyncEnd`
// register pair.

// ENCP_DVI_VSO_BLINE_EVEN (DVI/HDMI Vsync out start line for even field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 391.
class HdmiEncoderVerticalSyncStart
    : public hwreg::RegisterBase<HdmiEncoderVerticalSyncStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVerticalSyncStart> Get() {
    return {0x1c32 * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // Start line (inclusive) of the vertical sync signal for each field, in
  // lines.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  DEF_FIELD(11, 0, lines);
};

// ENCP_VIDEO_VSO_ELINE_EVEN (Digital Vsync out end line for even field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 392.
class HdmiEncoderVerticalSyncEnd
    : public hwreg::RegisterBase<HdmiEncoderVerticalSyncEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVerticalSyncEnd> Get() {
    return {0x1c34 * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // One (1) plus the end line (inclusive) of the vertical sync signal for each
  // field, in lines.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  //
  // It's preferred to use `lines()` and `set_lines()` helper methods to
  // get / set its value.
  DEF_FIELD(11, 0, lines_plus_one);

  int lines() const { return static_cast<int>(lines_plus_one()) - 1; }

  // `lines` must be within [0, 2^12 - 1].
  HdmiEncoderVerticalSyncEnd& set_lines(int lines) {
    ZX_DEBUG_ASSERT(lines >= 0);
    ZX_DEBUG_ASSERT(lines <= (1 << 12) - 1);
    return set_lines_plus_one(lines + 1);
  }
};

// The duration of the vertical sync part of the VSYNC timing signal for each
// odd field. Used for interlaced output only.

// ENCP_DVI_VSO_BLINE_ODD (DVI/HDMI Vsync out start line for odd field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 392.
class HdmiEncoderVerticalSyncStartOddFields
    : public hwreg::RegisterBase<HdmiEncoderVerticalSyncStartOddFields, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVerticalSyncStartOddFields> Get() {
    return {0x1c33 * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // Start line (inclusive) of the vertical sync signal for each field, in
  // lines.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  DEF_FIELD(11, 0, lines);
};

// ENCP_VIDEO_VSO_ELINE_ODD (Digital Vsync out end line for odd field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 392.
class HdmiEncoderVerticalSyncEndOddFields
    : public hwreg::RegisterBase<HdmiEncoderVerticalSyncEndOddFields, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVerticalSyncEndOddFields> Get() {
    return {0x1c35 * sizeof(uint32_t)};
  }

  // Bits 31-12 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 12);

  // One (1) plus the end line (inclusive) of the vertical sync signal for each
  // field, in lines.
  //
  // S912 datasheet only specifies bits 10-0 are used for this field. However,
  // experiments confirm that bit 11 is functional on S912 and SoCs with similar
  // encoder designs (S905D2, S905D3, A311D) due to their support for the
  // 3840x2160 resolution. Therefore, this code utilizes bits 11-0 for the
  // field.
  //
  // It's preferred to use `lines()` and `set_lines()` helper methods to
  // get / set its value.
  DEF_FIELD(11, 0, lines_plus_one);

  int lines() const { return static_cast<int>(lines_plus_one()) - 1; }

  // `lines` must be within [0, 2^12 - 1].
  HdmiEncoderVerticalSyncEndOddFields& set_lines(int lines) {
    ZX_DEBUG_ASSERT(lines >= 0);
    ZX_DEBUG_ASSERT(lines <= (1 << 12) - 1);
    return set_lines_plus_one(lines + 1);
  }
};

// The horizontal duration (start and end points) for Vertical sync parts of
// the VSYNC signal on each line, for each field (progressive) or for each even
// field (interlaced).
//
// The actual function of this register is unclear. Theoretically, it
// should not affect the output VSYNC signal, because during Vsync the vsync
// signal should be high throughout the line.
//
// Amlogic-provided code always set both start and end points to the Hsync
// offset. Experiments on Khadas VIM3 (Amlogic A311D) show that this
// configuration works.

// ENCP_DVI_VSO_BEGIN_EVN (DVI/HDMI Interface VSO start position for even field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 389.
class HdmiEncoderVerticalSyncHorizontalStart
    : public hwreg::RegisterBase<HdmiEncoderVerticalSyncHorizontalStart, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVerticalSyncHorizontalStart> Get() {
    return {0x1c36 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // Start point of the horizontal part of the VSYNC signal for each Vertical
  // sync pulse on each line, in pixels.
  DEF_FIELD(12, 0, pixels);
};

// ENCP_DVI_VSO_END_EVN (DVI/HDMI Interface VSO end position for even field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 389.
class HdmiEncoderVerticalSyncHorizontalEnd
    : public hwreg::RegisterBase<HdmiEncoderVerticalSyncHorizontalEnd, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVideoVerticalSyncHorizontalEnd> Get() {
    return {0x1c38 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // End point of the horizontal sync part of the VSYNC signal for each
  // Vertical sync pulse on each line, in pixels.
  DEF_FIELD(12, 0, pixels);
};

// The horizontal start and end points for Vertical sync parts of the VSYNC
// signal on each line for each odd field. Used for interlaced output only.
//
// The actual function of this register is unclear. Theoretically, it
// should not affect the output VSYNC signal, because during Vsync the vsync
// signal should be high throughout the line.
//
// Amlogic-provided code always set both start and end points to the Hsync
// offset.

// ENCP_DVI_VSO_BEGIN_ODD (DVI/HDMI Interface VSO start position for odd field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 389.
class HdmiEncoderVerticalSyncHorizontalStartOddFields
    : public hwreg::RegisterBase<HdmiEncoderVerticalSyncHorizontalStartOddFields, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVerticalSyncHorizontalStartOddFields> Get() {
    return {0x1c07 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // Start point of the horizontal part of the VSYNC signal for each Vertical
  // sync pulse on each line, in pixels.
  DEF_FIELD(12, 0, pixels);
};

// ENCP_DVI_VSO_END_ODD (DVI/HDMI Interface VSO end position for odd field)
//
// S912 Datasheet, Section 27.5 "CVBS and LCD", page 389.
class HdmiEncoderVerticalSyncHorizontalEndOddFields
    : public hwreg::RegisterBase<HdmiEncoderVerticalSyncHorizontalEndOddFields, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiEncoderVerticalSyncHorizontalEndOddFields> Get() {
    return {0x1c09 * sizeof(uint32_t)};
  }

  // Bits 31-13 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 13);

  // End point of the horizontal sync part of the VSYNC signal for each
  // Vertical sync pulse on each line, in pixels.
  DEF_FIELD(12, 0, pixels);
};

// VPU_HDMI_SETTING
//
// S905D2 Datasheet, Section 7.2.3.1 "VPU Registers", pages 313-314.
// A311D Datasheet, Section 10.2.3.1 "VPU Registers", pages 668-669.
// S905D3 Datasheet, Section 9.2.3.1 "VPU Registers", pages 260-261.
class HdmiEncoderTransmitterBridgeSetting
    : public hwreg::RegisterBase<HdmiEncoderTransmitterBridgeSetting, uint32_t> {
 public:
  enum class SourceEncoderSelection : uint8_t {
    // No encoder is selected. The bridge is disabled.
    kNone = 0b00,
    // ENCI (Interlaced encoder)
    kInterlaced = 0b01,
    // ENCP (Progressive encoder)
    kProgressive = 0b10,
  };

  // The reordered color components for output, assuming the input color
  // components are in the (component0, component1, component2) order.
  enum class ColorComponentMappingFrom012 : uint8_t {
    // Outputs (component0, component1, component2).
    kTo012 = 0,
    // Outputs (component1, component2, component0).
    kTo120 = 1,
    // Outputs (component1, component0, component2).
    kTo102 = 2,
    // Outputs (component2, component0, component1).
    kTo201 = 3,
    // Outputs (component2, component1, component0).
    kTo210 = 4,
    // Outputs (component0, component2, component1).
    kTo021 = 5,
  };

  static hwreg::RegisterAddr<HdmiEncoderTransmitterBridgeSetting> Get() {
    return {0x271b * sizeof(uint32_t)};
  }

  // Bits 31-16 are not documented in the datasheet.
  // We model these bits as reserved must be zero because Amlogic-provided code
  // unconditionally sets them to zero.
  DEF_RSVDZ_FIELD(31, 16);

  // While setting the FIFO read / write downsampling multipliers, drivers
  // must guarantee that the FIFO read sample rate equals to the FIFO write
  // sample rate, i.e.
  //   (Encoder pixel rate) / fifo_read_downsampling_multiplier()
  // = (Transmitter pixel rate) / fifo_write_downsampling_multiplier()

  // Downsampling multiplier (reciprocal of the downsampling ratio) of the
  // Encoder-Transmitter FIFO reading pixel data from the encoder, minus one.
  //
  // A downsampling multiplier of N means that the FIFO performs a read
  // operation once every N cycles of the "read clock" (rd_clk), which is
  // possibly the encoder clock.
  //
  // It's preferred to use the `fifo_read_downsampling_multiplier()` and
  // `set_fifo_read_downsampling_multiplier()` helper methods to get / set its
  // value.
  DEF_FIELD(15, 12, fifo_read_downsampling_multiplier_minus_one);

  // Downsampling multiplier (reciprocal of the downsampling ratio) of the
  // Encoder-Transmitter FIFO writing pixel data to the transmitter, minus one.
  //
  // A downsampling multiplier of N means that the FIFO performs a write
  // operation once every N cycles of the "write clock" (wr_clk), which is
  // possibly the HDMI transmitter pixel clock.
  //
  // It's preferred to use the `fifo_write_downsampling_multiplier()` and
  // `set_fifo_write_downsampling_multiplier()` helper methods to get / set its
  // value.
  DEF_FIELD(11, 8, fifo_write_downsampling_multiplier_minus_one);

  // Sets the ordering of color components reordered by the encoder-transmitter
  // bridge for the transmitter to read.
  DEF_ENUM_FIELD(ColorComponentMappingFrom012, 7, 5, color_component_mapping);

  // If true, the polarity of the clock output to the external DVI interface
  // (not the internal HDMI interface) is positive (active high).
  //
  // None of the Amlogic SoCs we use (S905D2, A311D and S905D3) support
  // external DVI interfaces. Amlogic-provided code always sets this bit to 0.
  //
  // It's preferred to use `dvi_clock_polarity()` and `set_dvi_clock_polarity()`
  // helper methods to get / set its value.
  DEF_BIT(4, is_dvi_clock_polarity_positive);

  // If true, the polarity of the VSYNC signal the bridge outputs is positive
  // (active high).
  //
  // It's preferred to use `vsync_polarity()` and `set_vsync_polarity()` helper
  // methods to get / set its value.
  DEF_BIT(3, is_vsync_polarity_positive);

  // If true, the polarity of the HSYNC signal the bridge outputs is positive
  // (active high).
  //
  // It's preferred to use `hsync_polarity()` and `set_hsync_polarity()` helper
  // methods to get / set its value.
  DEF_BIT(2, is_hsync_polarity_positive);

  DEF_ENUM_FIELD(SourceEncoderSelection, 1, 0, source_encoder_selection);

  int fifo_read_downsampling_multiplier() const {
    return static_cast<int>(fifo_read_downsampling_multiplier_minus_one()) + 1;
  }

  // `multiplier` must be a valid integer in [1, 16].
  HdmiEncoderTransmitterBridgeSetting& set_fifo_read_downsampling_multiplier(int multiplier) {
    ZX_DEBUG_ASSERT(multiplier >= 1);
    ZX_DEBUG_ASSERT(multiplier <= 16);
    return set_fifo_read_downsampling_multiplier_minus_one(multiplier - 1);
  }

  int fifo_write_downsampling_multiplier() const {
    return static_cast<int>(fifo_write_downsampling_multiplier_minus_one()) + 1;
  }

  // `multiplier` must be a valid integer in [1, 16].
  HdmiEncoderTransmitterBridgeSetting& set_fifo_write_downsampling_multiplier(int multiplier) {
    ZX_DEBUG_ASSERT(multiplier >= 1);
    ZX_DEBUG_ASSERT(multiplier <= 16);
    return set_fifo_write_downsampling_multiplier_minus_one(multiplier - 1);
  }

  display::SyncPolarity dvi_clock_polarity() const {
    return is_dvi_clock_polarity_positive() ? display::SyncPolarity::kPositive
                                            : display::SyncPolarity::kNegative;
  }

  HdmiEncoderTransmitterBridgeSetting& set_dvi_clock_polarity(display::SyncPolarity polarity) {
    return set_is_dvi_clock_polarity_positive(polarity == display::SyncPolarity::kPositive);
  }

  display::SyncPolarity vsync_polarity() const {
    return is_vsync_polarity_positive() ? display::SyncPolarity::kPositive
                                        : display::SyncPolarity::kNegative;
  }

  HdmiEncoderTransmitterBridgeSetting& set_vsync_polarity(display::SyncPolarity polarity) {
    return set_is_vsync_polarity_positive(polarity == display::SyncPolarity::kPositive);
  }

  display::SyncPolarity hsync_polarity() const {
    return is_hsync_polarity_positive() ? display::SyncPolarity::kPositive
                                        : display::SyncPolarity::kNegative;
  }

  HdmiEncoderTransmitterBridgeSetting& set_hsync_polarity(display::SyncPolarity polarity) {
    return set_is_hsync_polarity_positive(polarity == display::SyncPolarity::kPositive);
  }
};

// VPU_HDMI_DITH_CNTL (10-bit to 8-bit dither control register)
//
// S905D2 Datasheet, Section 7.2.3.1 "VPU Registers", page 335.
// A311D Datasheet, Section 10.2.3.1 "VPU Registers", pages 687-688.
// S905D3 Datasheet, Section 9.2.3.1 "VPU Registers", page 280.
class HdmiEncoderTransmitterBridge10BitTo8BitDitheringControl
    : public hwreg::RegisterBase<HdmiEncoderTransmitterBridge10BitTo8BitDitheringControl,
                                 uint32_t> {
 public:
  static auto Get() {
    return hwreg::RegisterAddr<HdmiEncoderTransmitterBridge10BitTo8BitDitheringControl>(
        0x27fc * sizeof(uint32_t));
  }

  // Bits 21-5 and Bit 0 are documented in the Amlogic datasheets; these bits
  // are not documented in this driver yet because the current driver
  // implementation does not support dithering.
  // TODO(https://fxbug.dev/319165154): Document the configuration bits and
  // related registers.

  // If true, the 10-bit to 8-bit dithering is enabled.
  DEF_BIT(4, enabled);

  // If true, the polarity of the VSYNC signal the dithering module outputs is
  // inverted from the encoder's output.
  DEF_BIT(3, vsync_polarity_inverted);

  // If true, the polarity of the VSYNC signal the dithering module outputs is
  // inverted from the encoder's output.
  DEF_BIT(2, hsync_polarity_inverted);
};

// VPU_HDMI_FMT_CTRL (12-bit to 10-bit dither control register)
//
// S905D2 Datasheet, Section 7.2.3.1 "VPU Registers", pages 318-319.
// A311D Datasheet, Section 10.2.3.1 "VPU Registers", pages 673-674.
// S905D3 Datasheet, Section 9.2.3.1 "VPU Registers", pages 265-266.
class HdmiEncoderTransmitterBridge12BitTo10BitDitheringControl
    : public hwreg::RegisterBase<HdmiEncoderTransmitterBridge12BitTo10BitDitheringControl,
                                 uint32_t> {
 public:
  static auto Get() {
    return hwreg::RegisterAddr<HdmiEncoderTransmitterBridge12BitTo10BitDitheringControl>(
        0x2743 * sizeof(uint32_t));
  }

  // Bits 21-11, bits 9-5 and bits 3-0 are documented in the Amlogic datasheets;
  // These bits are not documented in this driver yet because the current
  // driver implementation doesn't support dithering.
  // TODO(https://fxbug.dev/319165154): Document the configuration bits and
  // related registers.

  // If true, the 12-bit colors are rounded down to 10-bit colors.
  //
  // `rounding_enabled` and `dithering_enabled` must not be both true.
  DEF_BIT(10, rounding_enabled);

  // If true, the 12-bit to 10-bit dithering is enabled.
  //
  // `rounding_enabled` and `dithering_enabled` must not be both true.
  //
  // This bit is documented as "dither 10-b to 8-b enable" in Amlogic
  // datasheets, which is a typo per the Amlogic-provided code.
  DEF_BIT(4, dithering_enabled);
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_ENCODER_REGS_H_
