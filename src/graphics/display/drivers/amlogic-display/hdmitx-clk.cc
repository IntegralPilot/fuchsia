// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/logging/cpp/logger.h>
#include <unistd.h>
#include <zircon/assert.h>

#include <cinttypes>

#include "src/graphics/display/drivers/amlogic-display/clock-regs.h"
#include "src/graphics/display/drivers/amlogic-display/fixed-point-util.h"
#include "src/graphics/display/drivers/amlogic-display/hdmi-host.h"
#include "src/graphics/display/drivers/amlogic-display/pll-regs.h"
#include "src/graphics/display/drivers/amlogic-display/vpu-regs.h"

namespace amlogic_display {

void HdmiHost::WaitForPllLocked() {
  bool err = false;
  do {
    unsigned int st = 0;
    int cnt = 10000;
    while (cnt--) {
      usleep(5);
      auto reg = HdmiPllControl0::Get().ReadFrom(&hhi_mmio_);
      st = (reg.is_locked() == 1) && (reg.is_locked_a() == 1);
      if (st) {
        err = false;
        break;
      } else { /* reset hpll */
        HdmiPllControl0::Get().ReadFrom(&hhi_mmio_).set_reset(true).WriteTo(&hhi_mmio_);
        HdmiPllControl0::Get().ReadFrom(&hhi_mmio_).set_reset(false).WriteTo(&hhi_mmio_);
      }
    }
    fdf::info("HDMI PLL reset {} times", 10000 - cnt);
    if (cnt <= 0)
      err = true;
  } while (err);
}

namespace {

VideoInputUnitEncoderMuxControl::Encoder EncoderSelectionFromViuType(viu_type type) {
  switch (type) {
    case VIU_ENCL:
      return VideoInputUnitEncoderMuxControl::Encoder::kLcd;
    case VIU_ENCI:
      return VideoInputUnitEncoderMuxControl::Encoder::kInterlaced;
    case VIU_ENCP:
      return VideoInputUnitEncoderMuxControl::Encoder::kProgressive;
    case VIU_ENCT:
      return VideoInputUnitEncoderMuxControl::Encoder::kTvPanel;
  }
  fdf::error("Incorrect VIU type: {}", static_cast<int>(type));
  return VideoInputUnitEncoderMuxControl::Encoder::kLcd;
}

}  // namespace

void HdmiHost::ConfigurePll(const pll_param& pll_params) {
  // Set VIU Mux Ctrl
  if (pll_params.viu_channel == 1) {
    VideoInputUnitEncoderMuxControl::Get()
        .ReadFrom(&vpu_mmio_)
        .set_vsync_shared_by_viu_blocks(false)
        .set_viu1_encoder_selection(
            EncoderSelectionFromViuType(static_cast<viu_type>(pll_params.viu_type)))
        .WriteTo(&vpu_mmio_);
  } else {
    VideoInputUnitEncoderMuxControl::Get()
        .ReadFrom(&vpu_mmio_)
        .set_vsync_shared_by_viu_blocks(false)
        .set_viu2_encoder_selection(
            EncoderSelectionFromViuType(static_cast<viu_type>(pll_params.viu_type)))
        .WriteTo(&vpu_mmio_);
  }
  HdmiClockControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_hdmi_tx_system_clock_selection(
          HdmiClockControl::HdmiTxSystemClockSource::kExternalOscillator24Mhz)
      .SetHdmiTxSystemClockDivider(1)
      .set_hdmi_tx_system_clock_enabled(true)
      .WriteTo(&hhi_mmio_);

  ConfigureHpllClkOut(pll_params.hdmi_pll_vco_output_frequency_hz);

  HdmiPllControl0::Get()
      .ReadFrom(&hhi_mmio_)
      .SetOutputDivider1(pll_params.output_divider1)
      .SetOutputDivider2(pll_params.output_divider2)
      .SetOutputDivider3(pll_params.output_divider3)
      .WriteTo(&hhi_mmio_);

  ConfigureHdmiClockTree(pll_params.hdmi_clock_tree_vid_pll_divider);

  VideoClock1Control::Get()
      .ReadFrom(&hhi_mmio_)
      .set_mux_source(VideoClockMuxSource::kVideoPll)
      .WriteTo(&hhi_mmio_);
  VideoClock1Divider::Get()
      .ReadFrom(&hhi_mmio_)
      .SetDivider0((pll_params.video_clock1_divider == 0) ? 1 : (pll_params.video_clock1_divider))
      .WriteTo(&hhi_mmio_);
  VideoClock1Control::Get()
      .ReadFrom(&hhi_mmio_)
      .set_div4_enabled(true)
      .set_div2_enabled(true)
      .set_div1_enabled(true)
      .WriteTo(&hhi_mmio_);

  HdmiClockControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_hdmi_tx_pixel_clock_selection(HdmiClockControl::HdmiTxPixelClockSource::kVideoClock1)
      .WriteTo(&hhi_mmio_);

  VideoClockOutputControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_hdmi_tx_pixel_clock_enabled(true)
      .WriteTo(&hhi_mmio_);

  if (pll_params.encp_clock_divider != -1) {
    VideoClock1Divider::Get()
        .ReadFrom(&hhi_mmio_)
        .set_encp_clock_selection(EncoderClockSource::kVideoClock1)
        .WriteTo(&hhi_mmio_);
    VideoClockOutputControl::Get()
        .ReadFrom(&hhi_mmio_)
        .set_encoder_progressive_enabled(true)
        .WriteTo(&hhi_mmio_);
    VideoClock1Control::Get().ReadFrom(&hhi_mmio_).set_divider0_enabled(true).WriteTo(&hhi_mmio_);
  }
  if (pll_params.enci_clock_divider != -1) {
    VideoClock1Divider::Get()
        .ReadFrom(&hhi_mmio_)
        .set_enci_clock_selection(EncoderClockSource::kVideoClock1)
        .WriteTo(&hhi_mmio_);
    VideoClockOutputControl::Get()
        .ReadFrom(&hhi_mmio_)
        .set_encoder_interlaced_enabled(true)
        .WriteTo(&hhi_mmio_);
    VideoClock1Control::Get().ReadFrom(&hhi_mmio_).set_divider0_enabled(true).WriteTo(&hhi_mmio_);
  }
}

// TODO(https://fxbug.dev/328135383): Unify the PLL configuration logic for
// HDMI and MIPI-DSI output.
void HdmiHost::ConfigureHpllClkOut(int64_t expected_hdmi_pll_vco_output_frequency_hz) {
  static constexpr int64_t kExternalOscillatorFrequencyHz = 24'000'000;

  static constexpr int32_t kMinHdmiPllMultiplierInteger = 1;
  static constexpr int32_t kMaxHdmiPllMultiplierInteger = 255;

  // TODO(https://fxbug.dev/328177521): Instead of asserting, we should check
  // the validity of the expected VCO output frequency before configuring the
  // PLL.
  ZX_ASSERT(expected_hdmi_pll_vco_output_frequency_hz >=
            kExternalOscillatorFrequencyHz * kMinHdmiPllMultiplierInteger);
  ZX_ASSERT(expected_hdmi_pll_vco_output_frequency_hz <=
            kExternalOscillatorFrequencyHz * kMaxHdmiPllMultiplierInteger);

  // The assertion above guarantees that `pll_multiplier_integer` is always
  // >= kMinHdmiPllMultiplierInteger and <= kMaxHdmiPllMultiplierInteger, so
  // it can be stored as an int32_t value.
  const int32_t pll_multiplier_integer = static_cast<int32_t>(
      expected_hdmi_pll_vco_output_frequency_hz / kExternalOscillatorFrequencyHz);

  static constexpr int32_t kPllMultiplierFractionScalingRatio = 1 << 17;
  // The result is in range [0, 2^17), so it can be stored as an int32_t
  // value.
  const int32_t pll_multiplier_fraction = static_cast<int32_t>(
      (expected_hdmi_pll_vco_output_frequency_hz % kExternalOscillatorFrequencyHz) *
      kPllMultiplierFractionScalingRatio / kExternalOscillatorFrequencyHz);

  fdf::debug("HDMI PLL VCO configured: desired multiplier = {} + {} / {}", pll_multiplier_integer,
             pll_multiplier_fraction, kPllMultiplierFractionScalingRatio);
  fdf::debug("HDMI PLL VCO output frequency: {} Hz", expected_hdmi_pll_vco_output_frequency_hz);

  // The HdmiPllControl{0-6} register values are from Amlogic-provided code.
  HdmiPllControl0::Get()
      .ReadFrom(&hhi_mmio_)
      .set_reset(false)
      .set_pll_enabled(false)
      .set_numerator_fraction_enabled(true)
      .set_hdmi_clock_out2_enabled(true)
      .set_hdmi_clock_out_enabled(true)
      .SetOutputDivider1(4)
      .SetOutputDivider2(4)
      // The original Amlogic-provided code sets `output_divider3_selection`
      // to 0b11, which doesn't map to any valid value in the datasheet. Here
      // we changed it to 4 to make it always valid.
      // Note that this value will be replaced eventually, so it won't affect
      // the final HDMI PLL clock output.
      .SetOutputDivider3(4)
      .SetDenominator(1)
      .SetNumeratorInteger(pll_multiplier_integer)
      .WriteTo(&hhi_mmio_);

  /* Enable and reset */
  HdmiPllControl0::Get()
      .ReadFrom(&hhi_mmio_)
      .set_pll_enabled(true)
      .set_reset(true)
      .WriteTo(&hhi_mmio_);

  HdmiPllControl1::Get()
      .FromValue(0)
      .set_numerator_fraction_is_negative(false)
      .set_numerator_fraction_u1_17(pll_multiplier_fraction)
      .WriteTo(&hhi_mmio_);

  auto control2 = HdmiPllControl2::Get()
                      .FromValue(0)
                      .set_reference_frequency_selection(0)
                      .set_os_ssc(0)
                      .set_spread_range_multiplier(0)
                      .set_spread_spectrum_clocking_enabled(false)
                      .set_spread_range_selection(0)
                      .set_spread_spectrum_mode(0)
                      .WriteTo(&hhi_mmio_);
  ZX_DEBUG_ASSERT_MSG(control2.reg_value() == 0x00000000,
                      "HDMI PLL Control Register 2 value (0x%08x)", control2.reg_value());

  auto control3 = HdmiPllControl3::Get()
                      .FromValue(0)
                      .set_afc_bypass(false)
                      .set_afc_clock_selection(0)
                      .set_code_new(false)
                      .set_dco_numerator_enabled(false)
                      .set_dco_sigma_delta_modulator_enabled(true)
                      .set_div2(false)
                      .set_div_mode(1)
                      .set_fast_lock(false)
                      .set_fb_pre_div(false)
                      .set_filter_mode(1)
                      .set_fix_enabled(true)
                      .set_freq_shift_enabled(false)
                      .set_load(true)
                      .set_load_enabled(false)
                      .set_lock_f(false)
                      .set_pulse_width_enabled(true)
                      .set_sdmnc_enabled(false)
                      .set_sdmnc_mode(0)
                      .set_sdmnc_range(0)
                      .set_tdc_enabled(true)
                      .set_tdc_mode_selection(1)
                      .set_wait_enabled(true);

  auto control4 = HdmiPllControl4::Get()
                      .FromValue(0)
                      .set_alpha(3)
                      .set_rho(3)
                      .set_lambda1(7)
                      .set_lambda0(7)
                      .set_acq_gain(1)
                      .set_filter_pvt2(2)
                      .set_filter_pvt1(9)
                      .set_pfd_gain(0);

  /* G12A HDMI PLL Needs specific parameters for 5.4GHz */
  if (pll_multiplier_integer >= 0xf7) {
    control3.set_afc_clock_selection(1)
        .set_code_new(true)
        .set_pulse_width_enabled(false)
        .set_sdmnc_mode(1);
    ZX_DEBUG_ASSERT_MSG(control3.reg_value() == 0x6a685c00,
                        "HDMI PLL Control Register 3 value (0x%08x)", control3.reg_value());
    control4.set_alpha(1).set_rho(1).set_lambda1(5).set_lambda0(5).set_pfd_gain(3);
    ZX_DEBUG_ASSERT_MSG(control4.reg_value() == 0x11551293,
                        "HDMI PLL Control Register 4 value (0x%08x)", control4.reg_value());
  } else {
    ZX_DEBUG_ASSERT_MSG(control3.reg_value() == 0x0a691c00,
                        "HDMI PLL Control Register 3 value (0x%08x)", control3.reg_value());
    ZX_DEBUG_ASSERT_MSG(control4.reg_value() == 0x33771290,
                        "HDMI PLL Control Register 4 value (0x%08x)", control4.reg_value());
  }
  control3.WriteTo(&hhi_mmio_);
  control4.WriteTo(&hhi_mmio_);

  auto control5 = HdmiPllControl5::Get()
                      .FromValue(0)
                      .set_adj_vco_ldo(0x03)
                      .set_lm_w(0x09)
                      .set_lm_s(0x27)
                      .set_reve(0x2000)
                      .WriteTo(&hhi_mmio_);
  ZX_DEBUG_ASSERT_MSG(control5.reg_value() == 0x39272000,
                      "HDMI PLL Control Register 5 value (0x%08x)", control5.reg_value());

  /* Reset PLL */
  HdmiPllControl0::Get().ReadFrom(&hhi_mmio_).set_reset(true).WriteTo(&hhi_mmio_);

  /* UN-Reset PLL */
  HdmiPllControl0::Get().ReadFrom(&hhi_mmio_).set_reset(false).WriteTo(&hhi_mmio_);

  /* Poll for lock bits */
  WaitForPllLocked();
}

void HdmiHost::ConfigureHdmiClockTree(int divider_ratio) {
  ZX_DEBUG_ASSERT_MSG(std::find(HdmiClockTreeControl::kSupportedFrequencyDividerRatios.begin(),
                                HdmiClockTreeControl::kSupportedFrequencyDividerRatios.end(),
                                ToU28_4(divider_ratio)) !=
                          HdmiClockTreeControl::kSupportedFrequencyDividerRatios.end(),
                      "HDMI clock tree divider ratio %d is not supported.", divider_ratio);

  // TODO(https://fxbug.dev/42086073): When the divider ratio is 6.25, some
  // Amlogic-provided code triggers a software reset of the `vid_pll_div` clock
  // before setting the HDMI clock tree, while some other Amlogic-provided code
  // doesn't do any reset.
  //
  // Currently fractional divider ratios are not supported; this needs to
  // be addressed once we add fraction divider ratio support.

  HdmiClockTreeControl hdmi_clock_tree_control = HdmiClockTreeControl::Get().ReadFrom(&hhi_mmio_);
  hdmi_clock_tree_control.set_clock_output_enabled(false).WriteTo(&hhi_mmio_);

  // This implementation deviates from the Amlogic-provided code.
  //
  // The Amlogic-provided code changes the pattern generator enablement, mode
  // selection and the state in different register writes, while the current
  // implementation changes all of them at the same time. Experiments on Khadas
  // VIM3 (A311D) show that our implementation works correctly.
  hdmi_clock_tree_control.ReadFrom(&hhi_mmio_)
      .SetFrequencyDividerRatio(ToU28_4(divider_ratio))
      .set_preset_pattern_update_enabled(true)
      .WriteTo(&hhi_mmio_);

  hdmi_clock_tree_control.ReadFrom(&hhi_mmio_)
      .set_preset_pattern_update_enabled(false)
      .WriteTo(&hhi_mmio_);

  hdmi_clock_tree_control.ReadFrom(&hhi_mmio_).set_clock_output_enabled(true).WriteTo(&hhi_mmio_);
}

}  // namespace amlogic_display
