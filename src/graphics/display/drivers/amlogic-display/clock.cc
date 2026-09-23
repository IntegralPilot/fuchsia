// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/clock.h"

#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cstdint>

#include <fbl/alloc_checker.h>

#include "src/graphics/display/drivers/amlogic-display/board-resources.h"
#include "src/graphics/display/drivers/amlogic-display/clock-regs.h"
#include "src/graphics/display/drivers/amlogic-display/common.h"
#include "src/graphics/display/drivers/amlogic-display/dsi.h"
#include "src/graphics/display/drivers/amlogic-display/fixed-point-util.h"
#include "src/graphics/display/drivers/amlogic-display/hhi-regs.h"
#include "src/graphics/display/drivers/amlogic-display/pll-regs.h"
#include "src/graphics/display/drivers/amlogic-display/vpu-regs.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"

namespace amlogic_display {

Clock::Clock(fdf::MmioBuffer vpu_mmio, fdf::MmioBuffer hhi_mmio, bool clock_enabled)
    : vpu_mmio_(std::move(vpu_mmio)),
      hhi_mmio_(std::move(hhi_mmio)),
      clock_enabled_(clock_enabled) {}

// static
LcdTiming Clock::CalculateLcdTiming(const display::DisplayTiming& d) {
  LcdTiming out;
  // Calculate and store DataEnable horizontal and vertical start/stop times
  const uint32_t de_hstart = d.horizontal_total_px() - d.horizontal_active_px - 1;
  const uint32_t de_vstart = d.vertical_total_lines() - d.vertical_active_lines;
  out.vid_pixel_on = de_hstart;
  out.vid_line_on = de_vstart;

  // Calculate and Store HSync horizontal and vertical start/stop times
  const uint32_t hstart = (de_hstart + d.horizontal_total_px() - d.horizontal_back_porch_px -
                           d.horizontal_sync_width_px) %
                          d.horizontal_total_px();
  const uint32_t hend =
      (de_hstart + d.horizontal_total_px() - d.horizontal_back_porch_px) % d.horizontal_total_px();
  out.hs_hs_addr = hstart;
  out.hs_he_addr = hend;

  // Calculate and Store VSync horizontal and vertical start/stop times
  out.vs_hs_addr = (hstart + d.horizontal_total_px()) % d.horizontal_total_px();
  out.vs_he_addr = out.vs_hs_addr;
  const uint32_t vstart = (de_vstart + d.vertical_total_lines() - d.vertical_back_porch_lines -
                           d.vertical_sync_width_lines) %
                          d.vertical_total_lines();
  const uint32_t vend = (de_vstart + d.vertical_total_lines() - d.vertical_back_porch_lines) %
                        d.vertical_total_lines();
  out.vs_vs_addr = vstart;
  out.vs_ve_addr = vend;
  return out;
}

zx::result<> Clock::WaitForHdmiPllToLock() {
  uint32_t pll_lock;

  constexpr int kMaxPllLockAttempt = 3;
  for (int lock_attempts = 0; lock_attempts < kMaxPllLockAttempt; lock_attempts++) {
    fdf::trace("Waiting for PLL Lock: ({}/3).", lock_attempts + 1);

    // The configurations used in retries are from Amlogic-provided code which
    // is undocumented.
    if (lock_attempts == 1) {
      HdmiPllControl3::Get().ReadFrom(&hhi_mmio_).set_afc_bypass(true).WriteTo(&hhi_mmio_);
    } else if (lock_attempts == 2) {
      auto control6 = HdmiPllControl6::Get()
                          .FromValue(0)
                          .set_afc_hold_t(1)
                          .set_lkw_sel(1)
                          .set_dco_sdm_clk_sel(1)
                          .set_afc_in(1)
                          .set_afc_nt(1)
                          .set_vc_in(1)
                          .set_lock_long(1)
                          .set_freq_shift_v(0)
                          .set_data_sel(0)
                          .set_sdmnc_ulms(0)
                          .set_sdmnc_power(0)
                          .WriteTo(&hhi_mmio_);
      ZX_DEBUG_ASSERT_MSG(control6.reg_value() == 0x55540000,
                          "HDMI PLL Control Register 6 value (0x%08x)", control6.reg_value());
    }

    int retries = 1000;
    while ((pll_lock = HdmiPllControl0::Get().ReadFrom(&hhi_mmio_).is_locked()) != true &&
           retries--) {
      zx_nanosleep(zx_deadline_after(ZX_USEC(50)));
    }
    if (pll_lock) {
      return zx::ok();
    }
  }

  fdf::error("Failed to lock HDMI PLL after {} attempts.", kMaxPllLockAttempt);
  return zx::error(ZX_ERR_UNAVAILABLE);
}

// static
zx::result<HdmiPllConfigForMipiDsi> Clock::GenerateHPLL(
    int64_t pixel_clock_frequency_hz, int64_t maximum_per_data_lane_bit_per_second) {
  HdmiPllConfigForMipiDsi pll_cfg;
  // Requested Pixel clock
  if (pixel_clock_frequency_hz > kMaxPixelClockFrequencyHz) {
    fdf::error("Pixel clock out of range ({} Hz)", pixel_clock_frequency_hz);
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  constexpr uint32_t kMinClockFactor = 1u;
  constexpr uint32_t kMaxClockFactor = 255u;

  // If clock factor is not specified in display panel configuration, the driver
  // will find the first valid clock factor in between kMinClockFactor and
  // kMaxClockFactor (both inclusive).
  uint32_t clock_factor_min = kMinClockFactor;
  uint32_t clock_factor_max = kMaxClockFactor;

  for (uint32_t clock_factor = clock_factor_min; clock_factor <= clock_factor_max; clock_factor++) {
    pll_cfg.clock_factor = clock_factor;

    // Desired PLL Frequency based on pixel clock needed
    const int64_t requested_pll_frequency_hz = pixel_clock_frequency_hz * pll_cfg.clock_factor;

    // Make sure all clocks are within range
    // If these values are not within range, we will not have a valid display
    const int64_t dphy_data_lane_bit_rate_max_hz = maximum_per_data_lane_bit_per_second;
    const int64_t dphy_data_lane_bit_rate_min_hz =
        dphy_data_lane_bit_rate_max_hz - pixel_clock_frequency_hz;
    if ((requested_pll_frequency_hz < dphy_data_lane_bit_rate_min_hz) ||
        (requested_pll_frequency_hz > dphy_data_lane_bit_rate_max_hz)) {
      fdf::trace("Calculated clocks out of range for xd = {}, skipped", clock_factor);
      continue;
    }

    // Now that we have valid frequency ranges, let's calculated all the PLL-related
    // multipliers/dividers
    // [fin] * [m/n] = [voltage_controlled_oscillator_output_frequency_hz]
    // [voltage_controlled_oscillator_output_frequency_khz] / [output_divider1]
    // / [output_divider2] / [output_divider3]
    // = requested_pll_frequency
    //
    // The PLL output is used for the MIPI D-PHY clock lane; its frequency
    // must be equal to the MIPI D-PHY data lane bit rate.
    int32_t output_divider3 = (1 << (MAX_OD_SEL - 1));
    while (output_divider3 != 0) {
      const int64_t output_divider3_input_frequency_hz =
          requested_pll_frequency_hz * output_divider3;
      int output_divider2 = output_divider3;
      while (output_divider2 != 0) {
        const int64_t output_divider2_input_frequency_hz =
            output_divider3_input_frequency_hz * output_divider2;
        int32_t output_divider1 = output_divider2;
        while (output_divider1 != 0) {
          const int64_t output_divider1_input_frequency_hz =
              output_divider2_input_frequency_hz * output_divider1;
          const int64_t voltage_controlled_oscillator_output_frequency_hz =
              output_divider1_input_frequency_hz;

          if ((voltage_controlled_oscillator_output_frequency_hz >=
               kMinVoltageControlledOscillatorFrequencyHz) &&
              (voltage_controlled_oscillator_output_frequency_hz <=
               kMaxVoltageControlledOscillatorFrequencyHz)) {
            // within range!
            pll_cfg.output_divider1 = output_divider1;
            pll_cfg.output_divider2 = output_divider2;
            pll_cfg.output_divider3 = output_divider3;
            pll_cfg.pll_frequency_hz = requested_pll_frequency_hz;
            fdf::trace("od1={}, od2={}, od3={}", output_divider1, output_divider2, output_divider3);
            fdf::trace("pll_fvco={}", voltage_controlled_oscillator_output_frequency_hz);
            pll_cfg.pll_voltage_controlled_oscillator_output_frequency_hz =
                voltage_controlled_oscillator_output_frequency_hz;

            // For simplicity, assume pll_divider = 1.
            pll_cfg.pll_divider = 1;

            // Calculate pll_multiplier such that
            // kExternalOscillatorFrequencyHz x pll_multiplier =
            // voltage_controlled_oscillator_output_frequency_hz
            pll_cfg.pll_multiplier_integer = static_cast<int32_t>(
                voltage_controlled_oscillator_output_frequency_hz / kExternalOscillatorFrequencyHz);
            pll_cfg.pll_multiplier_fraction = (voltage_controlled_oscillator_output_frequency_hz %
                                               kExternalOscillatorFrequencyHz) *
                                              PLL_FRAC_RANGE / kExternalOscillatorFrequencyHz;

            fdf::trace("m={}, n={}, frac=0x{:x}", pll_cfg.pll_multiplier_integer,
                       pll_cfg.pll_divider, pll_cfg.pll_multiplier_fraction);
            pll_cfg.dphy_data_lane_bits_per_second = pll_cfg.pll_frequency_hz;  // Hz

            return zx::ok(std::move(pll_cfg));
          }
          output_divider1 >>= 1;
        }
        output_divider2 >>= 1;
      }
      output_divider3 >>= 1;
    }
  }

  fdf::error("Could not generate correct PLL values for: ");
  fdf::error("  pixel_clock_frequency_hz = {}", pixel_clock_frequency_hz);
  fdf::error("  max_per_data_lane_bit_rate_hz = {}", maximum_per_data_lane_bit_per_second);
  return zx::error(ZX_ERR_INTERNAL);
}

void Clock::Disable() {
  if (!clock_enabled_) {
    return;
  }
  vpu_mmio_.Write32(0, ENCL_VIDEO_EN);

  VideoClockOutputControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_encoder_lvds_enabled(false)
      .WriteTo(&hhi_mmio_);
  VideoClock2Control::Get()
      .ReadFrom(&hhi_mmio_)
      .set_div1_enabled(false)
      .set_div2_enabled(false)
      .set_div4_enabled(false)
      .set_div6_enabled(false)
      .set_div12_enabled(false)
      .WriteTo(&hhi_mmio_);
  VideoClock2Control::Get().ReadFrom(&hhi_mmio_).set_clock_enabled(false).WriteTo(&hhi_mmio_);

  // disable pll
  HdmiPllControl0::Get().ReadFrom(&hhi_mmio_).set_pll_enabled(false).WriteTo(&hhi_mmio_);
  clock_enabled_ = false;
}

zx::result<> Clock::Enable(const PanelConfig& panel_config) {
  if (clock_enabled_) {
    return zx::ok();
  }

  // Populate internal LCD timing structure based on predefined tables
  lcd_timing_ = CalculateLcdTiming(panel_config.display_timing);
  zx::result<HdmiPllConfigForMipiDsi> pll_result =
      GenerateHPLL(panel_config.display_timing.pixel_clock_frequency_hz,
                   panel_config.maximum_per_data_lane_bit_per_second());
  if (pll_result.is_error()) {
    fdf::error("Failed to generate HDMI PLL and Video clock tree configuration: {}", pll_result);
    return pll_result.take_error();
  }
  pll_cfg_ = std::move(pll_result).value();

  const bool use_fraction = pll_cfg_.pll_multiplier_fraction != 0;

  HdmiPllControl0::Get()
      .FromValue(0)
      .set_pll_enabled(true)
      .set_hdmi_clock_out2_enabled(true)
      .SetDenominator(pll_cfg_.pll_divider)
      .SetNumeratorInteger(pll_cfg_.pll_multiplier_integer)
      .SetOutputDivider1(pll_cfg_.output_divider1)
      .SetOutputDivider2(pll_cfg_.output_divider2)
      .SetOutputDivider3(pll_cfg_.output_divider3)
      .set_numerator_fraction_enabled(use_fraction)
      .WriteTo(&hhi_mmio_);

  HdmiPllControl1::Get()
      .FromValue(0)
      .set_numerator_fraction_is_negative(false)
      .set_numerator_fraction_u1_17(pll_cfg_.pll_multiplier_fraction)
      .WriteTo(&hhi_mmio_);

  // The HdmiPllControl{2-6} register values are from Amlogic-provided code.
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
                      .set_afc_clock_selection(1)
                      .set_code_new(false)
                      .set_dco_numerator_enabled(false)
                      .set_dco_sigma_delta_modulator_enabled(true)
                      .set_div_mode(0)
                      .set_div2(false)
                      .set_fast_lock(false)
                      .set_fb_pre_div(false)
                      .set_filter_mode(1)
                      .set_fix_enabled(true)
                      .set_freq_shift_enabled(false)
                      .set_load(true)
                      .set_load_enabled(false)
                      .set_lock_f(false)
                      .set_pulse_width_enabled(false)
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

  if (use_fraction) {
    control3.set_code_new(true).set_div_mode(1).set_filter_mode(0).set_sdmnc_mode(1);
    ZX_DEBUG_ASSERT_MSG(control3.reg_value() == 0x6a285c00,
                        "HDMI PLL Control Register 3 value (0x%08x)", control3.reg_value());
    control4.set_alpha(6).set_rho(5);
    ZX_DEBUG_ASSERT_MSG(control4.reg_value() == 0x65771290,
                        "HDMI PLL Control Register 4 value (0x%08x)", control4.reg_value());
  } else {
    ZX_DEBUG_ASSERT_MSG(control3.reg_value() == 0x48681c00,
                        "HDMI PLL Control Register 3 value (0x%08x)", control3.reg_value());
    ZX_DEBUG_ASSERT_MSG(control4.reg_value() == 0x33771290,
                        "HDMI PLL Control Register 4 value (0x%08x)", control4.reg_value());
  }
  control3.WriteTo(&hhi_mmio_);
  control4.WriteTo(&hhi_mmio_);

  auto control5 = HdmiPllControl5::Get()
                      .FromValue(0)
                      .set_adj_vco_ldo(0x3)
                      .set_lm_w(0x9)
                      .set_lm_s(0x27)
                      .set_reve(0x2000)
                      .WriteTo(&hhi_mmio_);
  ZX_DEBUG_ASSERT_MSG(control5.reg_value() == 0x39272000,
                      "HDMI PLL Control Register 5 value (0x%08x)", control5.reg_value());

  auto control6 = HdmiPllControl6::Get()
                      .FromValue(0)
                      .set_afc_hold_t(1)
                      .set_lkw_sel(1)
                      .set_dco_sdm_clk_sel(1)
                      .set_afc_in(2)
                      .set_afc_nt(1)
                      .set_vc_in(1)
                      .set_lock_long(1)
                      .set_freq_shift_v(0)
                      .set_data_sel(0)
                      .set_sdmnc_ulms(0)
                      .set_sdmnc_power(0)
                      .WriteTo(&hhi_mmio_);
  ZX_DEBUG_ASSERT_MSG(control6.reg_value() == 0x56540000,
                      "HDMI PLL Control Register 6 value (0x%08x)", control6.reg_value());

  // reset dpll
  HdmiPllControl0::Get().ReadFrom(&hhi_mmio_).set_reset(true).WriteTo(&hhi_mmio_);
  zx_nanosleep(zx_deadline_after(ZX_USEC(100)));
  // release from reset
  HdmiPllControl0::Get().ReadFrom(&hhi_mmio_).set_reset(false).WriteTo(&hhi_mmio_);

  zx_nanosleep(zx_deadline_after(ZX_USEC(50)));
  zx::result<> wait_for_pll_lock_result = WaitForHdmiPllToLock();
  if (!wait_for_pll_lock_result.is_ok()) {
    fdf::error("Failed to lock HDMI PLL: {}", wait_for_pll_lock_result);
    return wait_for_pll_lock_result.take_error();
  }

  // Disable Video Clock mux 2 since we are changing its input selection.
  VideoClock2Control::Get().ReadFrom(&hhi_mmio_).set_clock_enabled(false).WriteTo(&hhi_mmio_);
  zx_nanosleep(zx_deadline_after(ZX_USEC(5)));

  // Sets the HDMI clock tree frequency division ratio to 1.

  // Disable the HDMI clock tree output
  HdmiClockTreeControl hdmi_clock_tree_control = HdmiClockTreeControl::Get().ReadFrom(&hhi_mmio_);
  hdmi_clock_tree_control.set_clock_output_enabled(false).WriteTo(&hhi_mmio_);

  hdmi_clock_tree_control.set_preset_pattern_update_enabled(false).WriteTo(&hhi_mmio_);
  hdmi_clock_tree_control.SetFrequencyDividerRatio(ToU28_4(1.0)).WriteTo(&hhi_mmio_);

  // Enable the final output clock
  hdmi_clock_tree_control.set_clock_output_enabled(true).WriteTo(&hhi_mmio_);

  // Enable DSI measure clocks.
  VideoInputMeasureClockControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_dsi_measure_clock_selection(
          VideoInputMeasureClockControl::ClockSource::kExternalOscillator24Mhz)
      .WriteTo(&hhi_mmio_);
  VideoInputMeasureClockControl::Get()
      .ReadFrom(&hhi_mmio_)
      .SetDsiMeasureClockDivider(1)
      .WriteTo(&hhi_mmio_);
  VideoInputMeasureClockControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_dsi_measure_clock_enabled(true)
      .WriteTo(&hhi_mmio_);

  // Use Video PLL (vid_pll) as MIPI_DSY PHY clock source.
  MipiDsiPhyClockControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_clock_source(MipiDsiPhyClockControl::ClockSource::kVideoPll)
      .WriteTo(&hhi_mmio_);
  // Enable MIPI-DSY PHY clock.
  MipiDsiPhyClockControl::Get().ReadFrom(&hhi_mmio_).set_enabled(true).WriteTo(&hhi_mmio_);
  // Set divider to 1.
  // TODO(https://fxbug.dev/42082049): This should occur before enabling the clock.
  MipiDsiPhyClockControl::Get().ReadFrom(&hhi_mmio_).SetDivider(1).WriteTo(&hhi_mmio_);

  // Set the Video clock 2 divider.
  VideoClock2Divider::Get()
      .ReadFrom(&hhi_mmio_)
      .SetDivider2(pll_cfg_.clock_factor)
      .WriteTo(&hhi_mmio_);
  zx_nanosleep(zx_deadline_after(ZX_USEC(5)));

  VideoClock2Control::Get()
      .ReadFrom(&hhi_mmio_)
      .set_mux_source(VideoClockMuxSource::kVideoPll)
      .WriteTo(&hhi_mmio_);
  VideoClock2Control::Get().ReadFrom(&hhi_mmio_).set_clock_enabled(true).WriteTo(&hhi_mmio_);
  zx_nanosleep(zx_deadline_after(ZX_USEC(2)));

  // Select video clock 2 for ENCL clock.
  VideoClock2Divider::Get()
      .ReadFrom(&hhi_mmio_)
      .set_encl_clock_selection(EncoderClockSource::kVideoClock2)
      .WriteTo(&hhi_mmio_);
  // Enable video clock 2 divider.
  VideoClock2Divider::Get().ReadFrom(&hhi_mmio_).set_divider_enabled(true).WriteTo(&hhi_mmio_);
  zx_nanosleep(zx_deadline_after(ZX_USEC(5)));

  VideoClock2Control::Get().ReadFrom(&hhi_mmio_).set_div1_enabled(true).WriteTo(&hhi_mmio_);
  VideoClock2Control::Get().ReadFrom(&hhi_mmio_).set_soft_reset(true).WriteTo(&hhi_mmio_);
  zx_nanosleep(zx_deadline_after(ZX_USEC(10)));
  VideoClock2Control::Get().ReadFrom(&hhi_mmio_).set_soft_reset(false).WriteTo(&hhi_mmio_);
  zx_nanosleep(zx_deadline_after(ZX_USEC(5)));

  // Enable ENCL clock output.
  VideoClockOutputControl::Get()
      .ReadFrom(&hhi_mmio_)
      .set_encoder_lvds_enabled(true)
      .WriteTo(&hhi_mmio_);

  zx_nanosleep(zx_deadline_after(ZX_MSEC(10)));

  vpu_mmio_.Write32(0, ENCL_VIDEO_EN);

  // Connect both VIUs (Video Input Units) to the LCD Encoder.
  VideoInputUnitEncoderMuxControl::Get()
      .ReadFrom(&vpu_mmio_)
      .set_vsync_shared_by_viu_blocks(false)
      .set_viu1_encoder_selection(VideoInputUnitEncoderMuxControl::Encoder::kLcd)
      .set_viu2_encoder_selection(VideoInputUnitEncoderMuxControl::Encoder::kLcd)
      .WriteTo(&vpu_mmio_);

  // Undocumented registers below
  vpu_mmio_.Write32(0x8000, ENCL_VIDEO_MODE);      // bit[15] shadown en
  vpu_mmio_.Write32(0x0418, ENCL_VIDEO_MODE_ADV);  // Sampling rate: 1

  // bypass filter -- Undocumented registers
  const display::DisplayTiming& display_timing = panel_config.display_timing;
  vpu_mmio_.Write32(0x1000, ENCL_VIDEO_FILT_CTRL);
  vpu_mmio_.Write32(display_timing.horizontal_total_px() - 1, ENCL_VIDEO_MAX_PXCNT);
  vpu_mmio_.Write32(display_timing.vertical_total_lines() - 1, ENCL_VIDEO_MAX_LNCNT);
  vpu_mmio_.Write32(lcd_timing_.vid_pixel_on, ENCL_VIDEO_HAVON_BEGIN);
  vpu_mmio_.Write32(display_timing.horizontal_active_px - 1 + lcd_timing_.vid_pixel_on,
                    ENCL_VIDEO_HAVON_END);
  vpu_mmio_.Write32(lcd_timing_.vid_line_on, ENCL_VIDEO_VAVON_BLINE);
  vpu_mmio_.Write32(display_timing.vertical_active_lines - 1 + lcd_timing_.vid_line_on,
                    ENCL_VIDEO_VAVON_ELINE);
  vpu_mmio_.Write32(lcd_timing_.hs_hs_addr, ENCL_VIDEO_HSO_BEGIN);
  vpu_mmio_.Write32(lcd_timing_.hs_he_addr, ENCL_VIDEO_HSO_END);
  vpu_mmio_.Write32(lcd_timing_.vs_hs_addr, ENCL_VIDEO_VSO_BEGIN);
  vpu_mmio_.Write32(lcd_timing_.vs_he_addr, ENCL_VIDEO_VSO_END);
  vpu_mmio_.Write32(lcd_timing_.vs_vs_addr, ENCL_VIDEO_VSO_BLINE);
  vpu_mmio_.Write32(lcd_timing_.vs_ve_addr, ENCL_VIDEO_VSO_ELINE);
  vpu_mmio_.Write32(3, ENCL_VIDEO_RGBIN_CTRL);
  vpu_mmio_.Write32(1, ENCL_VIDEO_EN);

  vpu_mmio_.Write32(0, L_RGB_BASE_ADDR);
  vpu_mmio_.Write32(0x400, L_RGB_COEFF_ADDR);
  vpu_mmio_.Write32(0x400, L_DITH_CNTL_ADDR);

  // The driver behavior here deviates from the Amlogic-provided code.
  //
  // The Amlogic-provided code sets up the timing controller (TCON) within the
  // LCD Encoder to generate Display Enable (DE), Horizontal Sync (HSYNC) and
  // Vertical Sync (VSYNC) timing signals.
  //
  // These signals are useful for LVDS or TTL LCD interfaces, but not for the
  // MIPI-DSI interface. The MIPI-DSI Host Controller IP block always sets the
  // output timings using the values from its own control registers, and it
  // doesn't use the outputs from the timing controller.
  //
  // Therefore, this driver doesn't set the timing controller registers and
  // keeps the timing controller disabled. This was tested on Nelson (S905D3)
  // and Khadas VIM3 (A311D) boards.

  vpu_mmio_.Write32(vpu_mmio_.Read32(VPP_MISC) & ~(VPP_OUT_SATURATE), VPP_MISC);

  // Ready to be used
  clock_enabled_ = true;
  return zx::ok();
}

void Clock::SetVideoOn(bool on) { vpu_mmio_.Write32(on, ENCL_VIDEO_EN); }

// static
zx::result<std::unique_ptr<Clock>> Clock::Create(
    fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device,
    bool already_enabled) {
  ZX_DEBUG_ASSERT(platform_device.is_valid());

  zx::result<fdf::MmioBuffer> vpu_mmio_result = MapMmio(kMmioNameVpu, platform_device);
  if (vpu_mmio_result.is_error()) {
    return vpu_mmio_result.take_error();
  }
  fdf::MmioBuffer vpu_mmio = std::move(vpu_mmio_result).value();

  zx::result<fdf::MmioBuffer> hhi_mmio_result = MapMmio(kMmioNameHhi, platform_device);
  if (hhi_mmio_result.is_error()) {
    return hhi_mmio_result.take_error();
  }
  fdf::MmioBuffer hhi_mmio = std::move(hhi_mmio_result).value();

  fbl::AllocChecker alloc_checker;
  auto clock =
      fbl::make_unique_checked<Clock>(&alloc_checker, std::move(vpu_mmio), std::move(hhi_mmio),
                                      /*clock_enabled=*/already_enabled);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for Clock");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(clock));
}

}  // namespace amlogic_display
