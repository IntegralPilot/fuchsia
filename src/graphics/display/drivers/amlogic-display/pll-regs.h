// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_PLL_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_PLL_REGS_H_

#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <cstdint>

#include <hwreg/bitfields.h>

namespace amlogic_display {

// Phased-Lock Loop (PLL) Control Registers
//
// PLLs are part of the Amlogic SoC clock subsystem, taking the crystal
// oscillator as the input and generating clock signals of given frequencies
// used by clock trees or device PHYs.
//
// ## HDMI PLL
//
// The HDMI PLL (also known as "HDMI TX PLL" on some Amlogic SoCs, like A311D2,
// to differentiate with the "HDMI RX PLL") takes the 24 MHz crystal oscillator
// as the input and generates clock signals HDMI_CLK_OUT (usage unknown) and
// HDMI_CLK_OUT2 (used by the HDMI clock tree).
//
// The output frequency can be calculated using the following formula:
//
//  HDMI_CLK_OUT2 = Freq(Oscillator) * (numerator_int +/- numerator_fractional)
//                  / denominator / OD1 / OD2 / OD3
//
// where the "numerator" ("M2" in the PLL diagram) and "demominator" ("N2" in
// the PLL diagram) modulates the Digitally-Controlled Oscillator to produce an
// output between 3GHz and 6GHz. Output frequency dividers (OD1, OD2 and OD3)
// further divide the frequency so that the input can be used by the HDMI and
// Video clock tree.
//
// The HDMI PLL supports spread spectrum clocking, and supports latching the
// value of the numerator.
//
// The diagram of the PLL is available at:
// A311D Datasheet, Section 8.7.2.8 "HDMI PLL", Page 120.
// S905D2 Datasheet, Section 6.6.3.7 "HDMI PLL", Page 104.
//
// For S905D3, the PLL register descriptions (in Section 6.7.6) are almost
// identical as those of A311D and S905D2 (with minor differences), however,
// the diagram in the section "6.7.3.8 HDMI PLL" is completely different from
// the other SoCs and contradicts the register description. Information from
// A311D2 datasheet (Section 7.6.3.7 "HDMI RX PLL", Page 161-162) shows that
// the diagram actually refers to the "HDMI RX PLL".
//
// Both experiments on Nelson (using S905D3) and Amlogic-provided code
// show that: The "HDMI RX PLL" doesn't exist on S905D3; S905D3 uses the
// same HDMI PLL as that of A311D and S905D2; the register description tables
// (in Section 6.7.6) are accurate.

// HHI_HDMI_PLL_CNTL0 - HDMI PLL Control Register 0.
//
// A311D Datasheet, Section 8.7.2.8 "HDMI PLL", Page 120;
//   Section 8.7.6 "Register Descriptions", Page 178.
// S905D2 Datasheet, Section 6.6.3.7 "HDMI PLL", Page 104;
//   Section 6.6.6 "Register Descriptions", Page 164.
// S905D3 Datasheet, Section 6.7.6 "Register Descriptions", Page 150.
class HdmiPllControl0 : public hwreg::RegisterBase<HdmiPllControl0, uint32_t> {
 public:
  // Values are from the following table:
  //
  // A311D Datasheet, Section 8.7.2.8 "HDMI PLL", Table 6-13 "A311D HDMI PLL OD
  //   Control Table", Page 120.
  // S905D2 Datasheet, Section 6.6.3.7 "HDMI PLL", Table 6-13 "S905D2 HDMI PLL
  //   OD Control Table", Page 104;
  enum class OutputDividerSelection : uint32_t {
    k1 = 0,
    k2 = 1,
    k4 = 2,
  };

  static auto Get() { return hwreg::RegisterAddr<HdmiPllControl0>(0xc8 * sizeof(uint32_t)); }

  // Read-only. From Amlogic-provided code, the PLL is considered locked iff
  // `is_locked` and `is_locked_a` are both true.
  DEF_BIT(31, is_locked);

  // Read-only. From Amlogic-provided code, the PLL is considered locked iff
  // `is_locked` and `is_locked_a` are both true.
  DEF_BIT(30, is_locked_a);

  // Resets the PLL. Per Amlogic-provided code, to reset / reconfigure the PLL,
  // drivers set this bit to 1, then set PLL control registers, and finally
  // set it to 0 and wait for the PLL to lock.
  DEF_BIT(29, reset);
  DEF_BIT(28, pll_enabled);

  // Bit 27 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code indicates that this bit is set true iff the
  // fractional part of the numerator is non-zero.
  DEF_BIT(27, numerator_fraction_enabled);

  // Bit 26 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(26);

  // Gate-controls the "HDMI_CLK_OUT2" signal.
  DEF_BIT(25, hdmi_clock_out2_enabled);

  // Gate-controls the "HDMI_CLK_OUT" signal.
  DEF_BIT(24, hdmi_clock_out_enabled);

  // Bits 23-22 are not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(23, 22);

  // Known as "HDMI_DPLL_OD2<1:0>" in the datasheet.
  //
  // It's preferred to use `OutputDivider3()` and `SetOutputDivider3()` to
  // direct field manipulation.
  DEF_ENUM_FIELD(OutputDividerSelection, 21, 20, output_divider3_selection);

  // Known as "HDMI_DPLL_OD<3:2>" in the datasheet.
  //
  // It's preferred to use `OutputDivider2()` and `SetOutputDivider2()` to
  // direct field manipulation.
  DEF_ENUM_FIELD(OutputDividerSelection, 19, 18, output_divider2_selection);

  // Known as "HDMI_DPLL_OD<1:0>" in the datasheet.
  //
  // It's preferred to use `OutputDivider1()` and `SetOutputDivider1()` to
  // direct field manipulation.
  DEF_ENUM_FIELD(OutputDividerSelection, 17, 16, output_divider1_selection);

  // Bit 15 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(15);

  // The actual value of the denominator.
  //
  // Amlogic-provided code requires this value be one on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931).
  //
  // It's preferred to use `SetDenomiator()` to direct field manipulation.
  DEF_FIELD(14, 10, denominator);
  static constexpr int kMinAllowedDenominator = 1;
  static constexpr int kMaxAllowedDenominator = 1;

  // Bits 9-8 are not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(9, 8);

  // The actual value of the integral part of the numerator.
  //
  // Amlogic-provided code requires this value be in the range [2, 511] on all
  // Fuchsia-supported platforms (A311D, S905D2, S905D3, and T931). However,
  // the datasheet mentions that this field only uses bit 7-0, so this field
  // must not exceed 255.
  //
  // Also note that on all Fuchsia-supported platforms, the crystal oscillator
  // is fixed to 24MHz, and the frequency of the HDMI PLL VCO cannot exceed
  // 6GHz; so in this specific case, the maximum allowed `numerator_integer`
  // should be
  //   6000 / 24 + 2 = 251
  // when `numerator_fraction` <= -1.
  //
  // Given all the constraints above, for maximum convenience, compatibility and
  // extensibility (to use other oscillators), we keep the original bit
  // definition of the field (bits 7-0), and require the value must be in the
  // range of [2, 255]. Drivers must check that the final VCO frequency does
  // not exceed the 6GHz hardware limit.
  //
  // It's preferred to use `SetNumeratorInteger()` to direct field manipulation.
  DEF_FIELD(7, 0, numerator_integer);
  static constexpr int kMinAllowedNumeratorInteger = 2;
  static constexpr int kMaxAllowedNumeratorInteger = 255;

  int OutputDivider3() const {
    OutputDividerSelection divider_selection = output_divider3_selection();
    switch (divider_selection) {
      case OutputDividerSelection::k1:
        return 1;
      case OutputDividerSelection::k2:
        return 2;
      case OutputDividerSelection::k4:
        return 4;
    }
    fdf::warn("Invalid output divider 3 selection: {}", static_cast<uint32_t>(divider_selection));
    return 0;
  }

  HdmiPllControl0& SetOutputDivider3(int divider) {
    switch (divider) {
      case 1:
        return set_output_divider3_selection(OutputDividerSelection::k1);
      case 2:
        return set_output_divider3_selection(OutputDividerSelection::k2);
      case 4:
        return set_output_divider3_selection(OutputDividerSelection::k4);
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid output divider 3: %d", divider);
    return *this;
  }

  int OutputDivider2() const {
    OutputDividerSelection divider_selection = output_divider2_selection();
    switch (divider_selection) {
      case OutputDividerSelection::k1:
        return 1;
      case OutputDividerSelection::k2:
        return 2;
      case OutputDividerSelection::k4:
        return 4;
    }
    fdf::warn("Invalid output divider 2 selection: {}", static_cast<uint32_t>(divider_selection));
    return 0;
  }

  HdmiPllControl0& SetOutputDivider2(int divider) {
    switch (divider) {
      case 1:
        return set_output_divider2_selection(OutputDividerSelection::k1);
      case 2:
        return set_output_divider2_selection(OutputDividerSelection::k2);
      case 4:
        return set_output_divider2_selection(OutputDividerSelection::k4);
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid output divider 2: %d", divider);
    return *this;
  }

  int OutputDivider1() const {
    OutputDividerSelection divider_selection = output_divider1_selection();
    switch (divider_selection) {
      case OutputDividerSelection::k1:
        return 1;
      case OutputDividerSelection::k2:
        return 2;
      case OutputDividerSelection::k4:
        return 4;
    }
    fdf::warn("Invalid output divider 1 selection: {}", static_cast<uint32_t>(divider_selection));
    return 0;
  }

  HdmiPllControl0& SetOutputDivider1(int divider) {
    switch (divider) {
      case 1:
        return set_output_divider1_selection(OutputDividerSelection::k1);
      case 2:
        return set_output_divider1_selection(OutputDividerSelection::k2);
      case 4:
        return set_output_divider1_selection(OutputDividerSelection::k4);
    }
    ZX_DEBUG_ASSERT_MSG(false, "Invalid output divider 1: %d", divider);
    return *this;
  }

  HdmiPllControl0& SetDenominator(int denominator) {
    if (denominator >= kMinAllowedDenominator && denominator <= kMaxAllowedDenominator) {
      return set_denominator(denominator);
    }
    ZX_DEBUG_ASSERT_MSG(false, "Unsupported PLL denominator: %d", denominator);
    return *this;
  }

  HdmiPllControl0& SetNumeratorInteger(int numerator_integer) {
    if (numerator_integer >= kMinAllowedNumeratorInteger &&
        numerator_integer <= kMaxAllowedNumeratorInteger) {
      return set_numerator_integer(numerator_integer);
    }
    ZX_DEBUG_ASSERT_MSG(false, "PLL numerator integer part (%d) out of range [%d, %d]",
                        numerator_integer, kMinAllowedNumeratorInteger,
                        kMaxAllowedNumeratorInteger);
    return *this;
  }
};

// HHI_HDMI_PLL_CNTL1 - HDMI PLL Control Register 1.
//
// A311D Datasheet, Section 8.7.2.8 "HDMI PLL", Page 120;
//   Section 8.7.6 "Register Descriptions", Page 178.
// S905D2 Datasheet, Section 6.6.3.7 "HDMI PLL", Page 104;
//   Section 6.6.6 "Register Descriptions", Page 164-165.
// S905D3 Datasheet, Section 6.7.6 "Register Descriptions", Page 150.
class HdmiPllControl1 : public hwreg::RegisterBase<HdmiPllControl1, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<HdmiPllControl1>(0xc9 * sizeof(uint32_t)); }

  // Bits 31-19 are not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(31, 19);

  // Iff true, the numerator will be "numerator_integer - numerator_fraction";
  // otherwise the numerator will be "numerator_integer + numerator_fraction".
  DEF_BIT(18, numerator_fraction_is_negative);

  // A U1.17 format (1 integral bit and 17 fractional bits) unsigned fixed-point
  // fraction.
  // See https://source.android.com/docs/core/audio/data_formats#q for details
  // of the Um.n notation.
  //
  // TODO(fxbug.dev/132161): Add helpers to convert between fixed-point numbers
  // and rational numbers / floating point numbers.
  DEF_FIELD(17, 0, numerator_fraction_u1_17);
};

// HHI_HDMI_PLL_CNTL2 - HDMI PLL Control Register 2.
//
// The datasheets only provide names for each field without any other per-field
// definition. Field documentation in this class is only speculation based on
// the abbreviations and behavior of Amlogic-provided code, and may not reflect
// the actual hardware configuration.
//
// A311D Datasheet, Section 8.7.2.8 "HDMI PLL", Page 120;
//   Section 8.7.6 "Register Descriptions", Page 178.
// S905D2 Datasheet, Section 6.6.3.7 "HDMI PLL", Page 104;
//   Section 6.6.6 "Register Descriptions", Page 165.
// S905D3 Datasheet, Section 6.7.6 "Register Descriptions", Page 150.
class HdmiPllControl2 : public hwreg::RegisterBase<HdmiPllControl2, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<HdmiPllControl2>(0xca * sizeof(uint32_t)); }

  // Bits 31-23 are not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(31, 23);

  // Known as "fref_sel" in datasheets.
  DEF_FIELD(22, 20, reference_frequency_selection);

  // Bits 19-18 are not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(19, 18);

  // Possibly stands for "Oscillator Spread Spectrum Clocking".
  DEF_FIELD(17, 16, os_ssc);

  // Spread range multiplier.
  //
  // From Amlogic-provided code, the spread range can be calculated as
  //  spread range (ppm) = 500 * `spread_spectrum_selection` * `spread_range_multiplier`.
  // where 1ppm = 10^-6.
  //
  // The valid range of `spread_range_multiplier` is unknown.
  //
  // Known as "ssc_str_m" in datasheets.
  DEF_FIELD(15, 12, spread_range_multiplier);

  // Bits 11-9 are not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(11, 9);

  // Enables spread spectrum clocking.
  //
  // Known as "ssc_en" in datasheets.
  DEF_BIT(8, spread_spectrum_clocking_enabled);

  // Spread range selector, where 1 stands for 500 ppm, 2 stands for 1000 ppm,
  // and `n` stands for `n * 500` ppm.
  //
  // Together with `spread_range_multiplier`, it determines the spectrum spread
  // range.
  //
  // The valid range of `spread_range_selection` is unknown.
  //
  // Known as "ssc_dep_sel" in datasheets.
  DEF_FIELD(7, 4, spread_range_selection);

  // Known as "ss_mode" in datasheets.
  DEF_FIELD(1, 0, spread_spectrum_mode);
};

// HHI_HDMI_PLL_CNTL3 - HDMI PLL Control Register 3.
//
// The datasheets only provide names for each field without any other per-field
// definition. Bit/field names are kept untouched except for minor readability
// improvements.
//
// A311D Datasheet, Section 8.7.2.8 "HDMI PLL", Page 120;
//   Section 8.7.6 "Register Descriptions", Page 178-179.
// S905D2 Datasheet, Section 6.6.3.7 "HDMI PLL", Page 104;
//   Section 6.6.6 "Register Descriptions", Page 165-166.
// S905D3 Datasheet, Section 6.7.6 "Register Descriptions", Page 150-151.
class HdmiPllControl3 : public hwreg::RegisterBase<HdmiPllControl3, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<HdmiPllControl3>(0xcb * sizeof(uint32_t)); }

  // "AFC" possibly stands for "automatic frequency calibration".
  DEF_BIT(31, afc_bypass);

  // Known as "afc_clk_sel" in datasheets.
  DEF_BIT(30, afc_clock_selection);

  DEF_BIT(29, code_new);

  // Enables the digitally-controlled oscillator (DCO) numerator(?).
  // Known as "dco_m_en" in datasheets.
  DEF_BIT(28, dco_numerator_enabled);

  // Enables the digitally-controlled oscillator (DCO) sigma-delta modulator(?).
  // Known as "dco_sdm_en" in datasheets.
  DEF_BIT(27, dco_sigma_delta_modulator_enabled);

  // This field is removed from S905D3 datasheets, while Amlogic-provided code
  // use the same value on this field for S905D3 and S905D2/A311D.
  DEF_BIT(26, div2);
  DEF_BIT(25, div_mode);
  DEF_BIT(24, fast_lock);
  DEF_BIT(23, fb_pre_div);
  DEF_BIT(22, filter_mode);

  // Known as "fix_en" in datasheets.
  DEF_BIT(21, fix_enabled);

  // Known as "freq_shift_en" in datasheets.
  DEF_BIT(20, freq_shift_enabled);

  DEF_BIT(19, load);

  // Known as "load_en" in datasheets.
  DEF_BIT(18, load_enabled);

  DEF_BIT(17, lock_f);

  // Known as "pulse_width_en" in datasheets.
  DEF_BIT(16, pulse_width_enabled);

  // "SDMNC" possibly stands for "Sigma-delta modulation noise cancellation".
  // Known as "sdmnc_en" in datasheets.
  DEF_BIT(15, sdmnc_enabled);

  DEF_BIT(14, sdmnc_mode);
  DEF_BIT(13, sdmnc_range);

  // "TDC" possibly stands for "Time-to-digital Converters".
  // Known as "tdc_en" in datasheets.
  DEF_BIT(12, tdc_enabled);

  // Known as "tdc_mode_sel" in datasheets.
  DEF_BIT(11, tdc_mode_selection);

  // Known as "wait_en" in datasheets.
  DEF_BIT(10, wait_enabled);

  // Bits 9-0 are not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(9, 0);
};

// HHI_HDMI_PLL_CNTL4 - HDMI PLL Control Register 4.
//
// The datasheets only provide names for each field without any other per-field
// definition. Bit/field names are kept untouched except for minor readability
// improvements.
//
// A311D Datasheet, Section 8.7.2.8 "HDMI PLL", Page 120;
//   Section 8.7.6 "Register Descriptions", Page 179-180.
// S905D2 Datasheet, Section 6.6.3.7 "HDMI PLL", Page 104;
//   Section 6.6.6 "Register Descriptions", Page 166.
// S905D3 Datasheet, Section 6.7.6 "Register Descriptions", Page 151.
class HdmiPllControl4 : public hwreg::RegisterBase<HdmiPllControl4, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<HdmiPllControl4>(0xcc * sizeof(uint32_t)); }

  // Bit 31 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(31);

  DEF_FIELD(30, 28, alpha);

  // Bit 27 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(27);

  // Known as "rou" in datasheets.
  DEF_FIELD(26, 24, rho);

  // Bit 23 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(23);

  DEF_FIELD(22, 20, lambda1);

  // Bit 19 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(19);

  DEF_FIELD(18, 16, lambda0);

  // Bits 15-14 are not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(15, 14);

  DEF_FIELD(13, 12, acq_gain);
  DEF_FIELD(11, 8, filter_pvt2);
  DEF_FIELD(7, 4, filter_pvt1);

  // Bits 3-2 are not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(3, 2);

  // "PFD" possibly stands for "phase frequency detector".
  DEF_FIELD(1, 0, pfd_gain);
};

// HHI_HDMI_PLL_CNTL5 - HDMI PLL Control Register 5.
//
// The datasheets only provide names for each field without any other per-field
// definition. Bit/field names are kept untouched except for minor readability
// improvements.
//
// A311D Datasheet, Section 8.7.2.8 "HDMI PLL", Page 120;
//   Section 8.7.6 "Register Descriptions", Page 180.
// S905D2 Datasheet, Section 6.6.3.7 "HDMI PLL", Page 104;
//   Section 6.6.6 "Register Descriptions", Page 166.
// S905D3 Datasheet, Section 6.7.6 "Register Descriptions", Page 151.
class HdmiPllControl5 : public hwreg::RegisterBase<HdmiPllControl5, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<HdmiPllControl5>(0xcd * sizeof(uint32_t)); }

  // Bit 31 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(31);

  // Possibly stands for "adjustable parameter of the low-dropout (LDO) voltage
  // regulator for the voltage-controlled oscillator (VCO)".
  DEF_FIELD(30, 28, adj_vco_ldo);
  DEF_FIELD(27, 24, lm_w);

  // Bits 23-22 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this field to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" field.
  DEF_RSVDZ_FIELD(23, 22);

  DEF_FIELD(21, 16, lm_s);
  DEF_FIELD(15, 0, reve);
};

// HHI_HDMI_PLL_CNTL6 - HDMI PLL Control Register 6.
//
// A311D Datasheet, Section 8.7.2.8 "HDMI PLL", Page 120;
//   Section 8.7.6 "Register Descriptions", Page 180.
// S905D2 Datasheet, Section 6.6.3.7 "HDMI PLL", Page 104;
//   Section 6.6.6 "Register Descriptions", Page 167.
// S905D3 Datasheet, Section 6.7.6 "Register Descriptions", Page 152.
class HdmiPllControl6 : public hwreg::RegisterBase<HdmiPllControl6, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<HdmiPllControl6>(0xce * sizeof(uint32_t)); }

  DEF_FIELD(31, 30, afc_hold_t);
  DEF_FIELD(29, 28, lkw_sel);

  // Selects the clock for digitally-controlled oscillator (DCO) sigma-delta
  // modulator (?).
  DEF_FIELD(27, 26, dco_sdm_clk_sel);

  // "AFC" possibly stands for "automatic frequency calibration".
  DEF_FIELD(25, 24, afc_in);
  DEF_FIELD(23, 22, afc_nt);

  // On S905D2 and A311D datasheets, bit 20 is also named "VLOCK_CNTL_EN" bit
  // by all other HDMI PLL register descriptions and is said to control internal
  // muxing of the registers. From documentation of Amlogic S912, this is a bit
  // used by previous generation SoCs (which supports setting PLL multipliers
  // from HDMI TOP block) and not used by the current generation of SoC anymore.
  DEF_FIELD(21, 20, vc_in);

  DEF_FIELD(19, 18, lock_long);
  DEF_FIELD(17, 16, freq_shift_v);

  // Bit 15 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(15);

  DEF_FIELD(14, 12, data_sel);

  // Bit 11 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(11);

  // "SDMNC" possibly stands for "Sigma-delta modulation noise cancellation".
  DEF_FIELD(10, 8, sdmnc_ulms);

  // Bit 7 is not defined in any of the Amlogic datasheets.
  // Amlogic-provided code sets this bit to zero on all Fuchsia-supported
  // platforms (A311D, S905D2, S905D3, and T931). Thus, we define it as a
  // "reserved-zero" bit.
  DEF_RSVDZ_BIT(7);

  DEF_FIELD(6, 0, sdmnc_power);
};

// HHI_HDMI_PLL_STS - HDMI PLL Status Register.
//
// A311D Datasheet, Section 8.7.6 "Register Descriptions", Page 181.
// S905D2 Datasheet, Section 6.6.6 "Register Descriptions", Page 167.
// S905D3 Datasheet, Section 6.7.6 "Register Descriptions", Page 152.
class HdmiPllStatus : public hwreg::RegisterBase<HdmiPllStatus, uint32_t> {
 public:
  static auto Get() { return hwreg::RegisterAddr<HdmiPllStatus>(0xcf * sizeof(uint32_t)); }

  // The same as the `is_locked` field in `HdmiPllControl0`.
  DEF_BIT(31, is_locked);

  // The same as the `is_locked_a` field in `HdmiPllControl0`.
  DEF_BIT(30, is_locked_a);

  // "AFC" possibly stands for "automatic frequency calibration".
  DEF_BIT(29, afc_done);

  // "SDMNC" possibly stands for "Sigma-delta modulation noise cancellation".
  DEF_FIELD(22, 16, sdmnc_monitor);

  // Known as "out_rsv" in the datasheet.
  DEF_FIELD(9, 0, out_reserved);
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_PLL_REGS_H_
