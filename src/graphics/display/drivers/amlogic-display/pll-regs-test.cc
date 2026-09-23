// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/pll-regs.h"

#include <gtest/gtest.h>

namespace amlogic_display {

namespace {

TEST(HdmiPllControl0, OutputDivider3) {
  HdmiPllControl0 reg = HdmiPllControl0::Get().FromValue(0);
  reg.SetOutputDivider3(1);
  EXPECT_EQ(reg.OutputDivider3(), 1);
  EXPECT_EQ(reg.output_divider3_selection(), HdmiPllControl0::OutputDividerSelection::k1);
  reg.SetOutputDivider3(2);
  EXPECT_EQ(reg.OutputDivider3(), 2);
  EXPECT_EQ(reg.output_divider3_selection(), HdmiPllControl0::OutputDividerSelection::k2);
  reg.SetOutputDivider3(4);
  EXPECT_EQ(reg.OutputDivider3(), 4);
  EXPECT_EQ(reg.output_divider3_selection(), HdmiPllControl0::OutputDividerSelection::k4);
}

TEST(HdmiPllControl0, OutputDivider2) {
  HdmiPllControl0 reg = HdmiPllControl0::Get().FromValue(0);
  reg.SetOutputDivider2(1);
  EXPECT_EQ(reg.OutputDivider2(), 1);
  EXPECT_EQ(reg.output_divider2_selection(), HdmiPllControl0::OutputDividerSelection::k1);
  reg.SetOutputDivider2(2);
  EXPECT_EQ(reg.OutputDivider2(), 2);
  EXPECT_EQ(reg.output_divider2_selection(), HdmiPllControl0::OutputDividerSelection::k2);
  reg.SetOutputDivider2(4);
  EXPECT_EQ(reg.OutputDivider2(), 4);
  EXPECT_EQ(reg.output_divider2_selection(), HdmiPllControl0::OutputDividerSelection::k4);
}

TEST(HdmiPllControl0, OutputDivider1) {
  HdmiPllControl0 reg = HdmiPllControl0::Get().FromValue(0);
  reg.SetOutputDivider1(1);
  EXPECT_EQ(reg.OutputDivider1(), 1);
  EXPECT_EQ(reg.output_divider1_selection(), HdmiPllControl0::OutputDividerSelection::k1);
  reg.SetOutputDivider1(2);
  EXPECT_EQ(reg.OutputDivider1(), 2);
  EXPECT_EQ(reg.output_divider1_selection(), HdmiPllControl0::OutputDividerSelection::k2);
  reg.SetOutputDivider1(4);
  EXPECT_EQ(reg.OutputDivider1(), 4);
  EXPECT_EQ(reg.output_divider1_selection(), HdmiPllControl0::OutputDividerSelection::k4);
}

TEST(HdmiPllControl0, SetNumeratorInteger) {
  HdmiPllControl0 reg = HdmiPllControl0::Get().FromValue(0);
  reg.SetNumeratorInteger(2);
  EXPECT_EQ(reg.numerator_integer(), 2u);
  reg.SetNumeratorInteger(128);
  EXPECT_EQ(reg.numerator_integer(), 128u);
  reg.SetNumeratorInteger(255);
  EXPECT_EQ(reg.numerator_integer(), 255u);
}

TEST(HdmiPllControl0, SetDenominator) {
  HdmiPllControl0 reg = HdmiPllControl0::Get().FromValue(0);
  reg.SetDenominator(1);
  EXPECT_EQ(reg.denominator(), 1u);
}

// Tests if the control register values with given configuration match the
// predefined values used in Amlogic-provided code.
//
// This tests the configuration for MIPI DSI displays with no fraction set in
// the PLL numerator.
TEST(HdmiPllControl, PresetConfigurationDsiNoFraction) {
  auto control2 = HdmiPllControl2::Get()
                      .FromValue(0)
                      .set_reference_frequency_selection(0)
                      .set_os_ssc(0)
                      .set_spread_range_multiplier(0)
                      .set_spread_spectrum_clocking_enabled(false)
                      .set_spread_range_selection(0)
                      .set_spread_spectrum_mode(0);
  EXPECT_EQ(control2.reg_value(), 0x00000000u);

  auto control3 = HdmiPllControl3::Get()
                      .FromValue(0)
                      .set_afc_bypass(false)
                      .set_afc_clock_selection(1)
                      .set_code_new(false)
                      .set_dco_numerator_enabled(false)
                      .set_dco_sigma_delta_modulator_enabled(true)
                      .set_div2(false)
                      .set_div_mode(0)
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
  EXPECT_EQ(control3.reg_value(), 0x48681c00u);

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
  EXPECT_EQ(control4.reg_value(), 0x33771290u);

  auto control5 = HdmiPllControl5::Get()
                      .FromValue(0)
                      .set_adj_vco_ldo(0x3)
                      .set_lm_w(0x9)
                      .set_lm_s(0x27)
                      .set_reve(0x2000);
  EXPECT_EQ(control5.reg_value(), 0x39272000u);

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
                      .set_sdmnc_power(0);
  EXPECT_EQ(control6.reg_value(), 0x56540000u);
}

// Tests if the control register values with given configuration match the
// predefined values used in Amlogic-provided code.
//
// This tests the configuration for MIPI DSI displays with fraction set in
// the PLL numerator.
TEST(HdmiPllControl, PresetConfigurationDsiWithFraction) {
  auto control2 = HdmiPllControl2::Get()
                      .FromValue(0)
                      .set_reference_frequency_selection(0)
                      .set_os_ssc(0)
                      .set_spread_range_multiplier(0)
                      .set_spread_spectrum_clocking_enabled(false)
                      .set_spread_range_selection(0)
                      .set_spread_spectrum_mode(0);
  EXPECT_EQ(control2.reg_value(), 0x00000000u);

  auto control3 = HdmiPllControl3::Get()
                      .FromValue(0)
                      .set_afc_bypass(false)
                      .set_afc_clock_selection(1)
                      .set_code_new(true)
                      .set_dco_numerator_enabled(false)
                      .set_dco_sigma_delta_modulator_enabled(true)
                      .set_div2(false)
                      .set_div_mode(1)
                      .set_fast_lock(false)
                      .set_fb_pre_div(false)
                      .set_filter_mode(0)
                      .set_fix_enabled(true)
                      .set_freq_shift_enabled(false)
                      .set_load(true)
                      .set_load_enabled(false)
                      .set_lock_f(false)
                      .set_pulse_width_enabled(false)
                      .set_sdmnc_enabled(false)
                      .set_sdmnc_mode(1)
                      .set_sdmnc_range(0)
                      .set_tdc_enabled(true)
                      .set_tdc_mode_selection(1)
                      .set_wait_enabled(true);
  EXPECT_EQ(control3.reg_value(), 0x6a285c00u);

  auto control4 = HdmiPllControl4::Get()
                      .FromValue(0)
                      .set_alpha(6)
                      .set_rho(5)
                      .set_lambda1(7)
                      .set_lambda0(7)
                      .set_acq_gain(1)
                      .set_filter_pvt2(2)
                      .set_filter_pvt1(9)
                      .set_pfd_gain(0);
  EXPECT_EQ(control4.reg_value(), 0x65771290u);

  auto control5 = HdmiPllControl5::Get()
                      .FromValue(0)
                      .set_adj_vco_ldo(0x3)
                      .set_lm_w(0x9)
                      .set_lm_s(0x27)
                      .set_reve(0x2000);
  EXPECT_EQ(control5.reg_value(), 0x39272000u);

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
                      .set_sdmnc_power(0);
  EXPECT_EQ(control6.reg_value(), 0x56540000u);
}

// Tests if the control register values with given configuration match the
// predefined values used in Amlogic-provided code.
//
// This tests the configuration for HDMI displays with 5.94 GHz (or higher)
// PLL output.
TEST(HdmiPllControl, PresetConfigurationHdmi5940MhzOrHigher) {
  auto control2 = HdmiPllControl2::Get()
                      .FromValue(0)
                      .set_reference_frequency_selection(0)
                      .set_os_ssc(0)
                      .set_spread_range_multiplier(0)
                      .set_spread_spectrum_clocking_enabled(false)
                      .set_spread_range_selection(0)
                      .set_spread_spectrum_mode(0);
  EXPECT_EQ(control2.reg_value(), 0x00000000u);

  auto control3 = HdmiPllControl3::Get()
                      .FromValue(0)
                      .set_afc_bypass(false)
                      .set_afc_clock_selection(1)
                      .set_code_new(true)
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
                      .set_pulse_width_enabled(false)
                      .set_sdmnc_enabled(false)
                      .set_sdmnc_mode(1)
                      .set_sdmnc_range(0)
                      .set_tdc_enabled(true)
                      .set_tdc_mode_selection(1)
                      .set_wait_enabled(true);
  EXPECT_EQ(control3.reg_value(), 0x6a685c00u);

  auto control4 = HdmiPllControl4::Get()
                      .FromValue(0)
                      .set_alpha(1)
                      .set_rho(1)
                      .set_lambda1(5)
                      .set_lambda0(5)
                      .set_acq_gain(1)
                      .set_filter_pvt2(2)
                      .set_filter_pvt1(9)
                      .set_pfd_gain(3);
  EXPECT_EQ(control4.reg_value(), 0x11551293u);

  auto control5 = HdmiPllControl5::Get()
                      .FromValue(0)
                      .set_adj_vco_ldo(0x3)
                      .set_lm_w(0x9)
                      .set_lm_s(0x27)
                      .set_reve(0x2000);
  EXPECT_EQ(control5.reg_value(), 0x39272000u);

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
                      .set_sdmnc_power(0);
  EXPECT_EQ(control6.reg_value(), 0x56540000u);
}

// Tests if the control register values with given configuration match the
// predefined values used in Amlogic-provided code.
//
// This tests the configuration for HDMI displays with PLL output frequency
// lower than 5.94 GHz.
TEST(HdmiPllControl, PresetConfigurationHdmiLowerThan5940Mhz) {
  auto control2 = HdmiPllControl2::Get()
                      .FromValue(0)
                      .set_reference_frequency_selection(0)
                      .set_os_ssc(0)
                      .set_spread_range_multiplier(0)
                      .set_spread_spectrum_clocking_enabled(false)
                      .set_spread_range_selection(0)
                      .set_spread_spectrum_mode(0);
  EXPECT_EQ(control2.reg_value(), 0x00000000u);

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
  EXPECT_EQ(control3.reg_value(), 0x0a691c00u);

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
  EXPECT_EQ(control4.reg_value(), 0x33771290u);

  auto control5 = HdmiPllControl5::Get()
                      .FromValue(0)
                      .set_adj_vco_ldo(0x3)
                      .set_lm_w(0x9)
                      .set_lm_s(0x27)
                      .set_reve(0x2000);
  EXPECT_EQ(control5.reg_value(), 0x39272000u);

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
                      .set_sdmnc_power(0);
  EXPECT_EQ(control6.reg_value(), 0x56540000u);
}

}  // namespace

}  // namespace amlogic_display
