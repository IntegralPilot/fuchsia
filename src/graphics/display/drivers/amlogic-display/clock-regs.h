// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_CLOCK_REGS_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_CLOCK_REGS_H_

#include <zircon/assert.h>

#include <cstdint>
#include <span>

#include <hwreg/bitfields.h>

#include "src/graphics/display/drivers/amlogic-display/fixed-point-util.h"

namespace amlogic_display {

// # Amlogic Clock Subsystem
//
// On Amlogic SoCs, the clock subsystem consists of **phased-lock loops (PLLs)**
// generating clock signals using oscillators, and **clock trees** which are
// exclusively made up of digital logic (such as muxes and frequency dividers).
//
// ## Video Clock Tree
//
// The **video clock tree** is a set of muxes and frequency dividers providing
// clocks for encoders, HDMI / DSI transmitter, display timing controllers,
// and video digital-analog converters.
//
// The video clock tree takes the following clock sources:
//
// - "vid_pll"
//   Video "PLL", which is actually an output signal of the HDMI clock tree
//   taking the HDMI PLL as its source.
//   * A311D Datasheet, Section 8.7.1.3 HDMI Clock Tree, Page 113
//   * S905D2 Datasheet, Section 6.6.2.3 HDMI Clock Tree, Page 97
//   * S905D3 Datasheet, Section 6.7.2.3 HDMI Clock Tree, Page 97
// - "gp0_pll"
//   General-purpose PLL 0.
//   * A311D Datasheet, Section 8.7.2.3 GP0 PLL, Page 117
//   * S905D2 Datasheet, Section 6.6.3.3 GP0 PLL, Page 101
//   * S905D3 Datasheet, Section 6.7.3.2 GP0 PLL, Page 100
// - "hifi_pll"
//   HiFi PLL.
//   * A311D Datasheet, Section 8.7.2.5 HIFI PLL, Page 118
//   * S905D2 Datasheet, Section 6.6.3.5 HIFI PLL, Page 102
//   * S905D3 Datasheet, Section 6.7.3.3 HIFI PLL, Page 101
// - "mp1_clk"
//   Also known as "MPLL1", "MPLL_DDS_CLK1". The fixed-frequency PLL (MPLL,
//   also known as FIX_PLL) is divided by a programmable frequency divider,
//   providing a clock with frequency of up to 500MHz.
//   References for all MPLL / FIX_PLL outputs:
//   * A311D Datasheet, Section 8.7.2.5 MPLL (Fixed PLL), Page 119
//   * S905D2 Datasheet, Section 6.6.3.6 MPLL (Fixed PLL), Page 103
//   * S905D3 Datasheet, Section 6.7.3.7 MPLL  Page 104
// - "fclk_div3"
//   Also known as "MPLL_CLK_OUT_DIV3". The fixed-frequency MPLL is divided by a
//   fixed-value frequency divider, providing a 666MHz fixed-frequency clock.
// - "fclk_div4"
//   Also known as "MPLL_CLK_OUT_DIV4". The fixed-frequency MPLL is divided by a
//   fixed-value frequency divider, providing a 500MHz fixed-frequency clock.
// - "fclk_div5"
//   Also known as "MPLL_CLK_OUT_DIV5". The fixed-frequency MPLL is divided by a
//   fixed-value frequency divider, providing a 400MHz fixed-frequency clock.
// - "fclk_div7"
//   Also known as "MPLL_CLK_OUT_DIV7". The fixed-frequency MPLL is divided by a
//   fixed-value frequency divider, providing a 285.7MHz fixed-frequency clock.
//
// It provides the following clock signals:
// - "cts_tcon" / "tcon_clko" (Timing controller)
// - "lcd_an_clk_ph2" (LCD Analog clock for PHY2)
// - "lcd_an_clk_ph3" (LCD Analog clock for PHY3)
// - "cts_enci_clk" (ENCI (Interlaced Encoder) clock)
// - "cts_encl_clk" (ENCL (LVDS Encoder) clock)
// - "cts_encp_clk" (ENCP (Progressive Encoder) clock)
// - "hdmi_tx_pixel" (HDMI Transmitter pixel clock)
// - "cts_vdac_clk" (Video digital-analog converter clock)
//
// The following sections of the Amlogic datasheets are good for understanding
// the PLLs and the dividers that make up the clock tree sources:
// - A311D
//   * Section 8.7.1 "Clock" > "Overview" (pages 108-109) has Table 8-9 "A311D
//     PLLs" and Figure 8-6 "Clock Connections";
//   * Section 8.7.2 "Frequency Calculation and Setting" (pages 115-121) has
//     per-PLL subsections showing diagrams.
//   * Section 8.7.1.3 "HDMI Clock Tree" (pages 112-113) has the clock tree
//     diagram (Figure 8-12).
// - S905D2
//   * Section 6.6.1 "Clock" > "Overview" (page 91) has Table 6-5 "A311D
//     PLLs".
//   * Section 6.6.2 "Clock Trees" (pages 91-92) has Figure 6-5 "Clock
//     Connections";
//   * Section 6.6.3 "Frequency Calculation and Setting" (pages 99-106) has
//     per-PLL subsections showing diagrams.
//   * Section 6.6.2.3 "HDMI Clock Tree" (pages 96-97) has the clock tree
//     diagram (Figure 6-11).
// - S905D3
//   * Section 6.7.1 "Clock" > "Overview" (page 90) has Table 6-5 "A311D
//     PLLs".
//   * Section 6.7.2 "Clock Trees" (pages 91-92) has Figure 6-5 "Clock
//     Connections";
//   * Section 6.7.3 "Frequency Calculation and Setting" (pages 99-106) has
//     per-PLL subsections showing diagrams.
//   * Section 6.7.2.3 "HDMI Clock Tree" (pages 96-97) has the clock tree
//     diagram (Figure 6-11).
//
// The detailed diagram of the video clock tree is shown in the following
// section of the Amlogic datasheets:
//
// A311D Datasheet, Figure 8-13 "Video Clock Tree", Section 8.7.1.4 EE Clock
// Tree, Page 114.
// S905D2 Datasheet, Figure 6-12 "Video Clock Tree", Section 6.6.2.4 EE Clock
// Tree, Page 98.
// S905D3 Datasheet, Figure 6-13 "Video Clock Tree", Section 6.7.2.4 EE Clock
// Tree, Page 99.

// Video clock tree has two muxes for input signals, named video clock 1 / mux 1
// (VID_CLK) and video clock 2 / mux 2 (VIID_CLK / V2).
//
// Each video clock mux is followed by a programmable divisor (/N0 for mux 1
// and /N2 for mux 2) and then multiple fixed divisors (/2, /4, /6 and /12).
// Each encoder / HDMI transmitter / video digital-to-analog converter (DAC)
// clock signal has its own mux, to select a clock from those provided by the
// above divisors.
//
// The output of video clock 1 is also used to generate the timing
// controller signal and LCD analog clocks.
enum class VideoClock {
  kVideoClock1 = 1,
  kVideoClock2 = 2,
};

// Selection of video clock muxes.
//
// The mux value <-> clock source mapping is shown in the following diagram
// of the Amlogic datasheets:
// A311D Datasheet, Figure 8-13 "Video Clock Tree", Section 8.7.1.4 EE Clock
// Tree, Page 114.
// S905D2 Datasheet, Figure 6-12 "Video Clock Tree", Section 6.6.2.4 EE Clock
// Tree, Page 98.
// S905D3 Datasheet, Figure 6-13 "Video Clock Tree", Section 6.7.2.4 EE Clock
// Tree, Page 99.
enum class VideoClockMuxSource : uint32_t {
  kVideoPll = 0,            // vid_pll
  kGeneralPurpose0Pll = 1,  // gp0_pll
  kHifiPll = 2,             // hifi_pll
  kMpll1 = 3,               // mp1_clk
  kFixed666Mhz = 4,         // fclk_div3
  kFixed500Mhz = 5,         // fclk_div4
  kFixed400Mhz = 6,         // fclk_div5
  kFixed285_7Mhz = 7,       // fclk_div7
};

// Selection of video clock and dividers for encoder clock muxes.
//
// The mux value <-> clock source mapping is shown in the following diagram
// of the Amlogic datasheets:
// A311D Datasheet, Figure 8-13 "Video Clock Tree", Section 8.7.1.4 EE Clock
// Tree, Page 114.
// S905D2 Datasheet, Figure 6-12 "Video Clock Tree", Section 6.6.2.4 EE Clock
// Tree, Page 98.
// S905D3 Datasheet, Figure 6-13 "Video Clock Tree", Section 6.7.2.4 EE Clock
// Tree, Page 99.
enum class EncoderClockSource : uint32_t {
  // "VideoClock1" is first divided by the programmable divider "/N0" before
  // being divided by the fixed divider. So the actual frequency is
  // (Selected Video Clock 1 input) / (N0) / (1, 2, 4, 16, or 12)
  kVideoClock1 = 0,
  kVideoClock1Div2 = 1,
  kVideoClock1Div4 = 2,
  kVideoClock1Div6 = 3,
  kVideoClock1Div12 = 4,

  // "VideoClock2" is first divided by the programmable divider "/N2" before
  // being divided by the fixed divider. So the actual frequency is
  // (Selected Video Clock 2 input) / (N2) / (1, 2, 4, 16, or 12)
  kVideoClock2 = 8,
  kVideoClock2Div2 = 9,
  kVideoClock2Div4 = 10,
  kVideoClock2Div6 = 11,
  kVideoClock2Div12 = 12,
};

// HHI_VIID_CLK_DIV
//
// A311D Datasheet, Section 8.7.6 Register Descriptions, Page 146.
// S905D2 Datasheet, Section 6.6.6 Register Descriptions, Page 126.
// S905D3 Datasheet, Section 6.7.6 Register Descriptions, Page 131.
class VideoClock2Divider : public hwreg::RegisterBase<VideoClock2Divider, uint32_t> {
 public:
  static constexpr int kMinDivider2 = 1;
  static constexpr int kMaxDivider2 = 256;
  static_assert(kMaxDivider2 == 1 << (7 - 0 + 1));

  static hwreg::RegisterAddr<VideoClock2Divider> Get() { return {0x4a * sizeof(uint32_t)}; }

  DEF_ENUM_FIELD(EncoderClockSource, 31, 28, video_dac_clock_selection);

  // Bits 27-24 and 23-20 document the DAC1 and DAC2 clock selections in A311D,
  // S905D2 and S905D3 documentations. These clocks don't exist in the clock
  // trees and are not used by this driver, so we don't define these bits.

  // Iff true, overrides the Video DAC clock source selection in
  // "video_dac_clock_selection" field to "adc_pll_clk_b2".
  DEF_BIT(19, video_dac_clock_selects_adc_pll_clock_b2);

  DEF_RSVDZ_BIT(18);

  // Iff true, resets the divider (/N2) for video clock 2.
  DEF_BIT(17, divider_reset);

  // Iff true, enables the divider (/N2) for video clock 2.
  DEF_BIT(16, divider_enabled);

  DEF_ENUM_FIELD(EncoderClockSource, 15, 12, encl_clock_selection);

  // Bits 14-8 are defined as unused in the datasheets, which contradicts the
  // definition of bits 15-12 in the same table. Experiments on VIM3 (A311D),
  // Astro (S905D2) and Nelson (S905D3) show that only bits 11-8 are unused.
  DEF_RSVDZ_FIELD(11, 8);

  // Also known as "/N2" in the Video Clock Tree diagram.
  //
  // Prefer `Divider2()` and `SetDivider2()` to accessing the field directly.
  DEF_FIELD(7, 0, divider2_minus_one);

  VideoClock2Divider& SetDivider2(int divider2) {
    ZX_DEBUG_ASSERT(divider2 >= kMinDivider2);
    ZX_DEBUG_ASSERT(divider2 <= kMaxDivider2);
    return set_divider2_minus_one(divider2 - 1);
  }

  int Divider2() const { return divider2_minus_one() + 1; }
};

// HHI_VIID_CLK_CNTL
//
// A311D Datasheet, Section 8.7.6 Register Descriptions, Page 146.
// S905D2 Datasheet, Section 6.6.6 Register Descriptions, Page 126-127.
// S905D3 Datasheet, Section 6.7.6 Register Descriptions, Page 132.
class VideoClock2Control : public hwreg::RegisterBase<VideoClock2Control, uint32_t> {
 public:
  static hwreg::RegisterAddr<VideoClock2Control> Get() { return {0x4b * sizeof(uint32_t)}; }

  DEF_RSVDZ_FIELD(31, 20);

  // If false, the input and output signals for the video clock 2 divider are
  // gated.
  //
  // The input signal may be also gated by the `divider_enabled` bit of the
  // `VideoClock2Divider` register.
  DEF_BIT(19, clock_enabled);

  DEF_ENUM_FIELD(VideoClockMuxSource, 18, 16, mux_source);

  // This is a "level triggered" signal. Drivers reset the clock dividers by
  // first setting the bit to 1, sleeping for 10 us (empirical value from VIM3
  // using Amlogic A311D chip) and then setting the bit to 0.
  DEF_BIT(15, soft_reset);

  DEF_RSVDZ_FIELD(12, 5);

  DEF_BIT(4, div12_enabled);
  DEF_BIT(3, div6_enabled);
  DEF_BIT(2, div4_enabled);
  DEF_BIT(1, div2_enabled);
  DEF_BIT(0, div1_enabled);
};

// HHI_VID_CLK_DIV
//
// A311D Datasheet, Section 8.7.6 Register Descriptions, Page 150.
// S905D2 Datasheet, Section 6.6.6 Register Descriptions, Page 136.
// S905D3 Datasheet, Section 6.7.6 Register Descriptions, Page 129.
class VideoClock1Divider : public hwreg::RegisterBase<VideoClock1Divider, uint32_t> {
 public:
  static constexpr int kMinDivider1 = 1;
  static constexpr int kMaxDivider1 = 256;
  static_assert(kMaxDivider1 == 1 << (15 - 8 + 1));

  static constexpr int kMinDivider0 = 1;
  static constexpr int kMaxDivider0 = 256;
  static_assert(kMaxDivider0 == 1 << (7 - 0 + 1));

  static hwreg::RegisterAddr<VideoClock1Divider> Get() { return {0x59 * sizeof(uint32_t)}; }

  DEF_ENUM_FIELD(EncoderClockSource, 31, 28, enci_clock_selection);

  DEF_ENUM_FIELD(EncoderClockSource, 27, 24, encp_clock_selection);

  DEF_ENUM_FIELD(EncoderClockSource, 23, 20, enct_clock_selection);

  DEF_RSVDZ_FIELD(19, 18);

  // Iff true, resets the dividers (/N0 and /N1) for video clock 1.
  DEF_BIT(17, dividers_reset);

  // Iff true, enables the dividers (/N0 and /N1) for video clock 1.
  // Divider /N0 / /N1 works iff `dividers_enabled` and `divider0/1_enabled`
  // field in `VideoClock1Control` register are both true.
  DEF_BIT(16, dividers_enabled);

  // Also known as "/N1" in the Video Clock Tree diagram.
  //
  // Prefer `Divider1()` and `SetDivider1()` to accessing the field directly.
  DEF_FIELD(15, 8, divider1_minus_one);

  VideoClock1Divider& SetDivider1(int divider1) {
    ZX_DEBUG_ASSERT(divider1 >= kMinDivider1);
    ZX_DEBUG_ASSERT(divider1 <= kMaxDivider1);
    return set_divider1_minus_one(divider1 - 1);
  }

  int Divider1() const { return divider1_minus_one() + 1; }

  // Also known as "/N0" in the Video Clock Tree diagram.
  //
  // Prefer `Divider0()` and `SetDivider0()` to accessing the field directly.
  DEF_FIELD(7, 0, divider0_minus_one);

  VideoClock1Divider& SetDivider0(int divider0) {
    ZX_DEBUG_ASSERT(divider0 >= kMinDivider0);
    ZX_DEBUG_ASSERT(divider0 <= kMaxDivider0);
    return set_divider0_minus_one(divider0 - 1);
  }

  int Divider0() const { return divider0_minus_one() + 1; }
};

// HHI_VID_CLK_CNTL
//
// A311D Datasheet, Section 8.7.6 Register Descriptions, Page 151.
// S905D2 Datasheet, Section 6.6.6 Register Descriptions, Page 137.
// S905D3 Datasheet, Section 6.7.6 Register Descriptions, Page 129-130.
class VideoClock1Control : public hwreg::RegisterBase<VideoClock1Control, uint32_t> {
 public:
  enum class LcdAnalogClockSelection : uint32_t {
    kVideoClock1Div6 = 0,
    kVideoClock1Div12 = 1,
  };

  static hwreg::RegisterAddr<VideoClock1Control> Get() { return {0x5f * sizeof(uint32_t)}; }

  // Bits 31-21 control the clock generation module for timing controller clock
  // (cts_tcon). The subfield definition is not in the register description
  // table but is mentioned in the clock tree diagram.
  //
  // A311D Datasheet, Section 8.7.1 Clock Trees, Page 114.
  // S905D2 Datasheet, Section 6.6.2 Clock Trees, Page 98.
  // S905D3 Datasheet, Section 6.7.2 Clock Trees, Page 99.
  DEF_FIELD(31, 21, timing_controller_clock_control);

  // If false, the output signal of divider /N1 is gated.
  //
  // Divider /N1 works iff `divider1_enabled` and the `dividers_enabled`
  // field in `VideoClock1Divider` register are both true.
  DEF_BIT(20, divider1_enabled);

  // If false, the output signal of divider /N0 is gated.
  //
  // Divider /N0 works iff `divider0_enabled` and the `dividers_enabled`
  // field in `VideoClock1Divider` register are both true.
  DEF_BIT(19, divider0_enabled);

  DEF_ENUM_FIELD(VideoClockMuxSource, 18, 16, mux_source);

  // This is a "level triggered" signal. Drivers reset the clock dividers by
  // first setting the bit to 1, sleeping for 10 us (empirical value from VIM3
  // using A311D chip) and then setting the bit to 0.
  DEF_BIT(15, soft_reset);

  // Enables the mux for the clock "lcd_an_clk_ph2" and "lcd_an_clk_ph3".
  DEF_BIT(14, lcd_analog_clock_mux_enabled);

  // "Video Clock Tree" diagrams use the bit 11 on register 0x1a
  // (HHI_GP1_PLL_CNTL2) which doesn't match the register definitions on the
  // same datasheet. Experiments on VIM3 (Amlogic A311D) shows that this bit
  // is the correct bit to select input source for LCD analog clocks.
  DEF_ENUM_FIELD(LcdAnalogClockSelection, 13, 13, lcd_analog_clock_selection);

  DEF_RSVDZ_FIELD(12, 5);

  DEF_BIT(4, div12_enabled);
  DEF_BIT(3, div6_enabled);
  DEF_BIT(2, div4_enabled);
  DEF_BIT(1, div2_enabled);
  DEF_BIT(0, div1_enabled);
};

// HHI_VID_CLK_CNTL2
//
// A311D Datasheet, Section 8.7.6 Register Descriptions, Page 152.
// S905D2 Datasheet, Section 6.6.6 Register Descriptions, Page 137.
// S905D3 Datasheet, Section 6.7.6 Register Descriptions, Page 130.
class VideoClockOutputControl : public hwreg::RegisterBase<VideoClockOutputControl, uint32_t> {
 public:
  static hwreg::RegisterAddr<VideoClockOutputControl> Get() { return {0x65 * sizeof(uint32_t)}; }

  DEF_RSVDZ_FIELD(15, 9);

  DEF_BIT(8, analog_tv_demodulator_video_dac_clock_enabled);
  DEF_BIT(7, lcd_analog_clock_phy2_enabled);
  DEF_BIT(6, lcd_analog_clock_phy3_enabled);
  DEF_BIT(5, hdmi_tx_pixel_clock_enabled);
  DEF_BIT(4, video_dac_clock_enabled);
  DEF_BIT(3, encoder_lvds_enabled);
  DEF_BIT(2, encoder_progressive_enabled);
  DEF_BIT(1, encoder_tv_enabled);
  DEF_BIT(0, encoder_interlaced_enabled);
};

// HHI_HDMI_CLK_CNTL - Configures "cts_hdmitx_sys_clk" and
// "cts_hdmitx_pixel_clk".
//
// A311D Datasheet, Section 8.7.6 Register Descriptions, Page 157.
// S905D2 Datasheet, Section 6.6.6 Register Descriptions, Page 142.
// S905D3 Datasheet, Section 6.7.6 Register Descriptions, Page 133.
class HdmiClockControl : public hwreg::RegisterBase<HdmiClockControl, uint32_t> {
 public:
  // Selection of video clock muxes.
  //
  // The mux value <-> clock source mapping is shown in the following diagram
  // of the Amlogic datasheets:
  // A311D Datasheet, Figure 8-13 "Video Clock Tree", Section 8.7.1.4 EE Clock
  // Tree, Page 114.
  // S905D2 Datasheet, Figure 6-12 "Video Clock Tree", Section 6.6.2.4 EE Clock
  // Tree, Page 98.
  // S905D3 Datasheet, Figure 6-13 "Video Clock Tree", Section 6.7.2.4 EE Clock
  // Tree, Page 99.
  enum class HdmiTxPixelClockSource : uint32_t {
    // "VideoClock1" is divided by the programmable divider "/N0" before
    // being divided by the fixed divider. So the actual frequency is
    // (Selected Video Clock 1 input) / (N0) / (1, 2, 4, 16, or 12)
    kVideoClock1 = 0,
    kVideoClock1Div2 = 1,
    kVideoClock1Div4 = 2,
    kVideoClock1Div6 = 3,
    kVideoClock1Div12 = 4,

    // "VideoClock2" is divided by the programmable divider "/N2" before
    // being divided by the fixed divider. So the actual frequency is
    // (Selected Video Clock 2 input) / (N2) / (1, 2, 4, 16, or 12)
    kVideoClock2 = 8,
    kVideoClock2Div2 = 9,
    kVideoClock2Div4 = 10,
    kVideoClock2Div6 = 11,
    kVideoClock2Div12 = 12,

    // "cts_tcon" clock provided by the video clock tree.
    //
    // This value is not documented in S905D3 datasheets. However, experiments
    // on a Nelson device (Amlogic S905D3) show that the value is the same as
    // other devices.
    kTimingControllerClock = 15,
  };

  enum class HdmiTxSystemClockSource : uint32_t {
    kExternalOscillator24Mhz = 0,  // xtal
    kFixed500Mhz = 1,              // fclk_div4
    kFixed666Mhz = 2,              // fclk_div3
    kFixed400Mhz = 3,              // fclk_div5
  };

  static constexpr int kMinHdmiTxSystemClockDivider = 1;
  static constexpr int kMaxHdmiTxSystemClockDivider = 128;
  static_assert(kMaxHdmiTxSystemClockDivider == 1 << (6 - 0 + 1));

  static hwreg::RegisterAddr<HdmiClockControl> Get() { return {0x73 * sizeof(uint32_t)}; }

  DEF_RSVDZ_FIELD(31, 20);

  DEF_ENUM_FIELD(HdmiTxPixelClockSource, 19, 16, hdmi_tx_pixel_clock_selection);

  DEF_RSVDZ_FIELD(15, 11);

  DEF_ENUM_FIELD(HdmiTxSystemClockSource, 10, 9, hdmi_tx_system_clock_selection);

  DEF_BIT(8, hdmi_tx_system_clock_enabled);

  DEF_RSVDZ_BIT(7);

  // Prefer `HdmiTxSystemClockDivider()` and `SetHdmiTxSystemClockDivider()` to
  // accessing the field directly.
  DEF_FIELD(6, 0, hdmi_tx_system_clock_divider_minus_one);

  HdmiClockControl& SetHdmiTxSystemClockDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinHdmiTxSystemClockDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxHdmiTxSystemClockDivider);
    return set_hdmi_tx_system_clock_divider_minus_one(divider - 1);
  }

  int HdmiTxSystemClockDivider() const { return hdmi_tx_system_clock_divider_minus_one() + 1; }
};

// ## EE (Everything Else) Clock Tree
//
// EE Clock Tree consists of clocks in the EE power domain, which includes
// (but not limited to) codecs, Mali GPUs, PWM controllers, Video Processing
// Unit (VPU) and video signal transmitters.
//
// Video Clock Tree is technically part of the EE Clock Tree, with a more
// complicated muxing and frequency divider logic.
//
// Details of sources and frequency dividers for each clock is available at:
// A311D Datasheet, Section 8.7.1.4 "EE Clock Tree", Page 113.
// S905D2 Datasheet, Section 6.7.1.4 "EE Clock Tree", Page 97.
// S905D3 Datasheet, Section 6.7.2.4 "EE Clock Tree", Page 98.
//
// Below we only list all the registers configuring clocks used for display.
//
// ### Branched clock inputs
//
// Some of the clocks have a final mux with two identical branches to facilitate
// fast clock transitions without frequency glitches. The unused branch can be
// configured for the new clock speed, and then the final mux is switched over
// for a quick transition. The branches have to be symmetrical for this to be
// easy to use.
//
// Similar dynamic muxes are also documented in other clock trees. The A53
// clock tree on S905D2, the A53/A73 clock tree on A311D, and the A55 clock
// tree on S905D3 all have similar designs documented.
//
// A311D Datasheet, Section 8.7.1.1 "A53/A73 Clock Tree", Pages 109-111.
// S905D2 Datasheet, Section 6.7.1.1 "A53 Clock Tree", Pages 92-94.
// S905D3 Datasheet, Section 6.7.2.1 "A55 Clock Tree", Pages 92-94.
//
// ### Frequency Dividers
//
// The control register fields store "division ratio - 1" for each frequency
// divider.
//
// This is not documented in A311D / S905D2 / S905D3 datasheets, but experiments
// on VIM3 (using A311D), Astro (using S905D2) and Nelson (using S905D3) and
// Amlogic-provided code has verified this. Besides, datasheets of new
// generation chips (for example, A311D2) have mentioned that, "if you want
// div8, set to 7".
//
// A311D2 Datasheet, Table 7-153 "CLKCTRL_CPU_CLKC_CTRL", Page 199.

// HHI_VPU_CLKC_CNTL - Configures the "cts_vpu_clkc" clock signal.
//
// The circuit has two branches, and a final mux that chooses between one of
// them. Each branch has an input mux connected to several clock sources,
// followed by a frequency divider.
//
// The mapping between register fields and branches is not available in the
// register description table, but in the EE Clock Tree table only.
//
// A311D Datasheet, Section 8.7.1.4 "EE Clock Tree", row "cts_vpu_clkc",
//   Page 113; Section 8.7.6 Register Descriptions, Page 155.
// S905D2 Datasheet, Section 6.6.6 Register Descriptions, Page 132;
//   Section 6.7.1.4 "EE Clock Tree", row "cts_vpu_clkc", Page 97.
// S905D3 Datasheet, Section 6.7.6 Register Descriptions, Page 141.
//    Section 6.7.2.4 "EE Clock Tree", row "cts_vpu_clkc", Page 98.
class VpuClockCControl : public hwreg::RegisterBase<VpuClockCControl, uint32_t> {
 public:
  enum class FinalMuxSource : uint32_t {
    kBranch0 = 0,
    kBranch1 = 1,
  };

  enum class ClockSource : uint32_t {
    kFixed500Mhz = 0,         // fclk_div4
    kFixed666Mhz = 1,         // fclk_div3
    kFixed400Mhz = 2,         // fclk_div5
    kFixed285_7Mhz = 3,       // fclk_div7
    kMpll1 = 4,               // mpll1
    kVideoPll = 5,            // vid_pll
    kMpll2 = 6,               // mpll2
    kGeneralPurpose0Pll = 7,  // gp0_pll
  };

  static constexpr int kMinBranchMuxDivider = 1;
  static constexpr int kMaxBranchMuxDivider = 128;

  static hwreg::RegisterAddr<VpuClockCControl> Get() { return {0x6d * sizeof(uint32_t)}; }

  DEF_ENUM_FIELD(FinalMuxSource, 31, 31, final_mux_selection);
  DEF_RSVDZ_FIELD(30, 29);
  DEF_ENUM_FIELD(ClockSource, 27, 25, branch1_mux_source);
  DEF_BIT(24, branch1_mux_enabled);
  DEF_RSVDZ_BIT(23);

  // Prefer `Branch1MuxDivider()` and `SetBranch1MuxDivider()` to accessing the field
  // directly.
  DEF_FIELD(22, 16, branch1_mux_divider_minus_one);
  static_assert(kMaxBranchMuxDivider == 1 << (22 - 16 + 1));

  // This field is undocumented on Amlogic datasheets.
  // Amlogic-provided code directly writes 0 to this field regardless of its
  // original value, so we can believe that zero is a safe setting for this
  // field.
  DEF_RSVDZ_FIELD(15, 12);

  DEF_ENUM_FIELD(ClockSource, 11, 9, branch0_mux_source);
  DEF_BIT(8, branch0_mux_enabled);
  DEF_RSVDZ_BIT(7);

  // Prefer `Branch0MuxDivider()` and `SetBranch0MuxDivider()` to accessing the field
  // directly.
  DEF_FIELD(6, 0, branch0_mux_divider_minus_one);
  static_assert(kMaxBranchMuxDivider == 1 << (6 - 0 + 1));

  VpuClockCControl& SetBranch1MuxDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinBranchMuxDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxBranchMuxDivider);
    return set_branch1_mux_divider_minus_one(divider - 1);
  }

  int Branch1MuxDivider() const { return branch1_mux_divider_minus_one() + 1; }

  VpuClockCControl& SetBranch0MuxDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinBranchMuxDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxBranchMuxDivider);
    return set_branch0_mux_divider_minus_one(divider - 1);
  }

  int Branch0MuxDivider() const { return branch0_mux_divider_minus_one() + 1; }
};

// HHI_VPU_CLK_CNTL - Configures the "cts_vpu_clk" clock signal.
//
// The circuit has two branches, and a final mux that chooses between one of
// them. Each branch has an input mux connected to several clock sources,
// followed by a frequency divider.
//
// The mapping between register fields and branches is not available in the
// register description table, but in the EE Clock Tree table only.
//
// A311D Datasheet, Section 8.7.1.4 "EE Clock Tree", row "cts_vpu_clk",
//   Page 113; Section 8.7.6 Register Descriptions, Page 156.
// S905D2 Datasheet, Section 6.7.1.4 "EE Clock Tree", row "cts_vpu_clk",
//   Page 97; Section 6.6.6 Register Descriptions, Page 132.
// S905D3 Datasheet, Section 6.7.2.4 "EE Clock Tree", row "cts_vpu_clk",
//   Page 98; Section 6.7.6 Register Descriptions, Page 142.
class VpuClockControl : public hwreg::RegisterBase<VpuClockControl, uint32_t> {
 public:
  enum class FinalMuxSource : uint32_t {
    kBranch0 = 0,
    kBranch1 = 1,
  };

  enum class ClockSource : uint32_t {
    kFixed666Mhz = 0,         // fclk_div3
    kFixed500Mhz = 1,         // fclk_div4
    kFixed400Mhz = 2,         // fclk_div5
    kFixed285_7Mhz = 3,       // fclk_div7
    kMpll1 = 4,               // mpll1
    kVideoPll = 5,            // vid_pll
    kHifiPll = 6,             // hifi_pll
    kGeneralPurpose0Pll = 7,  // gp0_pll
  };

  static constexpr int kMinBranchMuxDivider = 1;
  static constexpr int kMaxBranchMuxDivider = 128;

  static hwreg::RegisterAddr<VpuClockControl> Get() { return {0x6f * sizeof(uint32_t)}; }

  DEF_ENUM_FIELD(FinalMuxSource, 31, 31, final_mux_selection);
  DEF_RSVDZ_FIELD(30, 29);
  DEF_ENUM_FIELD(ClockSource, 27, 25, branch1_mux_source);
  DEF_BIT(24, branch1_mux_enabled);
  DEF_RSVDZ_BIT(23);

  // Prefer `Branch1MuxDivider()` and `SetBranch1MuxDivider()` to accessing the field
  // directly.
  DEF_FIELD(22, 16, branch1_mux_divider_minus_one);
  static_assert(kMaxBranchMuxDivider == 1 << (22 - 16 + 1));

  // This field is undocumented on Amlogic datasheets.
  // Amlogic-provided code directly writes 0 to this field regardless of its
  // original value, so we can believe that zero is a safe setting for this
  // field.
  DEF_RSVDZ_FIELD(15, 12);

  DEF_ENUM_FIELD(ClockSource, 11, 9, branch0_mux_source);
  DEF_BIT(8, branch0_mux_enabled);
  DEF_RSVDZ_BIT(7);

  // Prefer `Branch0MuxDivider()` and `SetBranch0MuxDivider()` to accessing the field
  // directly.
  DEF_FIELD(6, 0, branch0_mux_divider_minus_one);
  static_assert(kMaxBranchMuxDivider == 1 << (6 - 0 + 1));

  VpuClockControl& SetBranch1MuxDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinBranchMuxDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxBranchMuxDivider);
    return set_branch1_mux_divider_minus_one(divider - 1);
  }

  int Branch1MuxDivider() const { return branch1_mux_divider_minus_one() + 1; }

  VpuClockControl& SetBranch0MuxDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinBranchMuxDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxBranchMuxDivider);
    return set_branch0_mux_divider_minus_one(divider - 1);
  }

  int Branch0MuxDivider() const { return branch0_mux_divider_minus_one() + 1; }
};

// HHI_VAPBCLK_CNTL - Configures the "cts_vapbclk" and "cts_ge2d_clk" clock
// signal.
//
// The circuit has two branches, and a final mux that chooses between one of
// them. Each branch has an input mux connected to several clock sources,
// followed by a frequency divider.
//
// The mapping between register fields and branches are available in both the
// register description table, and the EE Clock Tree table.
//
// A311D Datasheet, Section 8.7.1.4 "EE Clock Tree", row "cts_vapbclk" and
//   "cts_ge2d_clk", Page 113; Section 8.7.6 Register Descriptions, Page 164.
// S905D2 Datasheet, Section 6.7.1.4 "EE Clock Tree", row "cts_vapbclk" and
//   "cts_ge2d_clk", Page 97; Section 6.6.6 Register Descriptions, Page 141.
// S905D3 Datasheet, Section 6.7.2.4 "EE Clock Tree", row "cts_vapbclk" and
//   "cts_ge2d_clk", Page 98; Section 6.7.6 Register Descriptions, Page 136.
class VideoAdvancedPeripheralBusClockControl
    : public hwreg::RegisterBase<VideoAdvancedPeripheralBusClockControl, uint32_t> {
 public:
  enum class FinalMuxSource : uint32_t {
    kBranch0 = 0,
    kBranch1 = 1,
  };

  enum class ClockSource : uint32_t {
    kFixed500Mhz = 0,    // fclk_div4
    kFixed666Mhz = 1,    // fclk_div3
    kFixed400Mhz = 2,    // fclk_div5
    kFixed285_7Mhz = 3,  // fclk_div7
    kMpll1 = 4,          // mpll1
    kVideoPll = 5,       // vid_pll
    kMpll2 = 6,          // mpll2
    kFixed800Mhz = 7,    // fclk_div2p5
  };

  static constexpr int kMinBranchMuxDivider = 1;
  static constexpr int kMaxBranchMuxDivider = 128;

  static hwreg::RegisterAddr<VideoAdvancedPeripheralBusClockControl> Get() {
    return {0x7d * sizeof(uint32_t)};
  }

  DEF_ENUM_FIELD(FinalMuxSource, 31, 31, final_mux_selection);

  // If false, the "cts_ge2d_clk" signal is gated. "cts_ge2d_clk" takes
  // "cts_vapbclk" output as its clock source and has no frequency dividers.
  //
  // This bit is named "enable" in A311D, S905D2 and S905D3 datasheet register
  // descriptions, but the "EE clock table" shows that it gates the
  // "cts_ge2d_clk" signal. Besides, A311D2 datasheet also documents it as
  // ""cts_ge2d_clk" enable" in the register descriptions.
  //
  // A311D2 Datasheet, Section 7.6.5 Register Descriptions, Page 200.
  DEF_BIT(30, ge2d_clock_enabled);

  DEF_RSVDZ_FIELD(29, 28);
  DEF_ENUM_FIELD(ClockSource, 27, 25, branch1_mux_source);
  DEF_BIT(24, branch1_mux_enabled);
  DEF_RSVDZ_BIT(23);

  // Prefer `Branch1MuxDivider()` and `SetBranch1MuxDivider()` to accessing the field
  // directly.
  DEF_FIELD(22, 16, branch1_mux_divider_minus_one);
  static_assert(kMaxBranchMuxDivider == 1 << (22 - 16 + 1));

  DEF_RSVDZ_FIELD(15, 12);
  DEF_ENUM_FIELD(ClockSource, 11, 9, branch0_mux_source);
  DEF_BIT(8, branch0_mux_enabled);
  DEF_RSVDZ_BIT(7);

  // Prefer `Branch0MuxDivider()` and `SetBranch0MuxDivider()` to accessing the field
  // directly.
  DEF_FIELD(6, 0, branch0_mux_divider_minus_one);
  static_assert(kMaxBranchMuxDivider == 1 << (6 - 0 + 1));

  VideoAdvancedPeripheralBusClockControl& SetBranch1MuxDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinBranchMuxDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxBranchMuxDivider);
    return set_branch1_mux_divider_minus_one(divider - 1);
  }

  int Branch1MuxDivider() const { return branch1_mux_divider_minus_one() + 1; }

  VideoAdvancedPeripheralBusClockControl& SetBranch0MuxDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinBranchMuxDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxBranchMuxDivider);
    return set_branch0_mux_divider_minus_one(divider - 1);
  }

  int Branch0MuxDivider() const { return branch0_mux_divider_minus_one() + 1; }
};

// HHI_VPU_CLKB_CNTL - Configures the "cts_vpu_clkb" and "cts_vpu_clkb_tmp"
// clock signals.
//
// The VPU Clock B (cts_vpu_clkb) first selects its source from a mux with
// VPU clock and 500, 400, 285.7 MHz fixed clocks, and then gets divided by
// divider 1 and divider 2.
//
// The datasheets describe it as two clock signals: the target clock signal
// "cts_vpu_clkb", and a temporary clock signal ("cts_vpu_clkb_tmp").
//
// "cts_vpu_clkb_tmp" takes inputs from PLLs, and "cts_vpu_clkb" takes inputs
// from only "cts_vpu_clkb_tmp", and clock has its own divider. Since the
// temporary clock signal is not used anywhere else, this is equivalent to our
// two-divider model described above.
//
// A311D Datasheet, Section 8.7.1.4 "EE Clock Tree", row "cts_vpu_clkb" and
//   "cts_vpu_clkb_tmp", Page 113; Section 8.7.6 Register Descriptions,
//   Page 164.
// S905D2 Datasheet, Section 6.7.1.4 "EE Clock Tree", row "cts_vpu_clkb" and
//   "cts_vpu_clkb_tmp", Page 97; Section 6.6.6 Register Descriptions, Page 141.
// S905D3 Datasheet, Section 6.7.2.4 "EE Clock Tree", row "cts_vpu_clkb" and
//   "cts_vpu_clkb_tmp", Page 98; Section 6.7.6 Register Descriptions, Page 136.
class VpuClockBControl : public hwreg::RegisterBase<VpuClockBControl, uint32_t> {
 public:
  // In the S905D3 datasheet, the mapping between selection values and clock
  // sources are not mentioned in the register description table, but only in
  // the EE Clock tree table, which matches the rest of the definitions.
  enum class ClockSource : uint32_t {
    kVpuClock = 0,       // cts_vpu_clk
    kFixed500Mhz = 1,    // fclk_div4
    kFixed400Mhz = 2,    // fclk_div5
    kFixed285_7Mhz = 3,  // fclk_div7
  };

  static constexpr int kMinDivider1 = 1;
  static constexpr int kMaxDivider1 = 16;

  static constexpr int kMinDivider2 = 1;
  static constexpr int kMaxDivider2 = 256;

  static hwreg::RegisterAddr<VpuClockBControl> Get() { return {0x83 * sizeof(uint32_t)}; }

  DEF_RSVDZ_FIELD(31, 25);

  // The clock is enabled only when both `divider1_enabled` and
  // `divider2_enabled` are true.
  DEF_BIT(24, divider1_enabled);

  DEF_ENUM_FIELD(ClockSource, 21, 20, clock_source);

  // Prefer `Divider1()` and `SetDivider1()` to accessing the field directly.
  DEF_FIELD(19, 16, divider1_minus_one);
  static_assert(kMaxDivider1 == 1 << (19 - 16 + 1));

  // Iff true, latches the register write until the next vpu_clkb_pulse signal.
  DEF_BIT(9, effective_after_vpu_clkb_pulse);

  // The clock is enabled only when both `divider1_enabled` and
  // `divider2_enabled` are true.
  DEF_BIT(8, divider2_enabled);

  // Prefer `Divider2()` and `SetDivider2()` to accessing the field directly.
  DEF_FIELD(7, 0, divider2_minus_one);
  static_assert(kMaxDivider2 == 1 << (7 - 0 + 1));

  VpuClockBControl& SetDivider1(int divider1) {
    ZX_DEBUG_ASSERT(divider1 >= kMinDivider1);
    ZX_DEBUG_ASSERT(divider1 <= kMaxDivider1);
    return set_divider1_minus_one(divider1 - 1);
  }

  int Divider1() const { return divider1_minus_one() + 1; }

  VpuClockBControl& SetDivider2(int divider2) {
    ZX_DEBUG_ASSERT(divider2 >= kMinDivider2);
    ZX_DEBUG_ASSERT(divider2 <= kMaxDivider2);
    return set_divider2_minus_one(divider2 - 1);
  }

  int Divider2() const { return divider2_minus_one() + 1; }
};

// HHI_VDIN_MEAS_CLK_CNTL - Configures the "cts_vdin_meas_clk" and
// "cts_dsi_meas_clk" clock signals.
//
// A311D Datasheet, Section 8.7.1.4 Clock Tree, Page 113; Section 8.7.6
//   Register Descriptions, Page 164.
// S905D2 Datasheet, Section 6.7.1.4 Clock Tree, Page 97; Section 6.6.6
//   Register Descriptions, Page 151.
// S905D3 Datasheet, Section 6.7.2.4 Clock Tree, Page 98; Section 6.7.6
//   Register Descriptions, Page 140.
class VideoInputMeasureClockControl
    : public hwreg::RegisterBase<VideoInputMeasureClockControl, uint32_t> {
 public:
  enum class ClockSource : uint32_t {
    kExternalOscillator24Mhz = 0,  // xtal
    kFixed500Mhz = 1,              // fclk_div4
    kFixed666Mhz = 2,              // fclk_div3
    kFixed400Mhz = 3,              // fclk_div5
    kVideoPll = 4,                 // vid_pll
    kGeneralPurpose0Pll = 5,       // gp0_pll
    kFixed1000Mhz = 6,             // fclk_div2
    kFixed285_7Mhz = 7,            // fclk_div7
  };

  static constexpr int kMinDsiMeasureClockDivider = 1;
  static constexpr int kMaxDsiMeasureClockDivider = 128;

  static constexpr int kMinVideoInputMeasureClockDivider = 1;
  static constexpr int kMaxVideoInputMeasureClockDivider = 128;

  static hwreg::RegisterAddr<VideoInputMeasureClockControl> Get() {
    return {0x94 * sizeof(uint32_t)};
  }

  DEF_RSVDZ_FIELD(31, 24);

  // In the S905D3 and S905D2 datasheets, bits 31-12 are documented as "unused"
  // in the register-level documentation but bits 23-12 (DSI measure clock
  // control) are mentioned in the EE clock tree table.
  //
  // Clock-measurement-based experiments on Astro (using S905D2) and Nelson
  // (using S905D3) show that the clock input value 0-7 have the definition
  // above on these devices.
  DEF_ENUM_FIELD(ClockSource, 23, 21, dsi_measure_clock_selection);

  DEF_BIT(20, dsi_measure_clock_enabled);

  // Prefer `DsiMeasureClockDivider()` and `SetDsiMeasureClockDivider()` to
  // accessing the field directly.
  DEF_FIELD(18, 12, dsi_measure_clock_divider_minus_one);
  static_assert(kMaxDsiMeasureClockDivider == 1 << (18 - 12 + 1));

  // In the S905D2 and A311D datasheets, the register description table listed
  // clock sources 0-7, while the EE clock tree table only listed clock source
  // 0-3.
  //
  // Clock-measurement-based experiments on Astro (using S905D2) and VIM3
  // (using A311D) show that the clock input value 0-7 have the definition
  // above on these devices.
  DEF_ENUM_FIELD(ClockSource, 11, 9, video_input_measure_clock_selection);

  DEF_BIT(8, video_input_measure_clock_enabled);

  // Prefer `VideoInputMeasureClockDivider()` and
  // `SetVideoInputMeasureClockDivider()` to accessing the field directly.
  DEF_FIELD(6, 0, video_input_measure_clock_divider_minus_one);
  static_assert(kMaxVideoInputMeasureClockDivider == 1 << (6 - 0 + 1));

  VideoInputMeasureClockControl& SetDsiMeasureClockDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinDsiMeasureClockDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxDsiMeasureClockDivider);
    return set_dsi_measure_clock_divider_minus_one(divider - 1);
  }

  int DsiMeasureClockDivider() const { return dsi_measure_clock_divider_minus_one() + 1; }

  VideoInputMeasureClockControl& SetVideoInputMeasureClockDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinVideoInputMeasureClockDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxVideoInputMeasureClockDivider);
    return set_video_input_measure_clock_divider_minus_one(divider - 1);
  }

  int VideoInputMeasureClockDivider() const {
    return video_input_measure_clock_divider_minus_one() + 1;
  }
};

// HHI_MIPIDSI_PHY_CLK_CNTL - Configures the "mipi_dsi_phy_clk" (also known as
// "cts_dsi_phy_clk") clock signal.
//
// This register is not documented in S905D3 datasheets but mentioned in S905D3
// EE clock tree table.
//
// A311D Datasheet, Section 8.7.1.4 Clock Tree, Page 113; Section 8.7.6
//   Register Descriptions, Page 164.
// S905D2 Datasheet, Section 6.7.1.4 Clock Tree, Page 97; Section 6.6.6
//   Register Descriptions, Page 151.
// S905D3 Datasheet, Section 6.7.2.4 Clock Tree, Page 98.
class MipiDsiPhyClockControl : public hwreg::RegisterBase<MipiDsiPhyClockControl, uint32_t> {
 public:
  enum class ClockSource : uint32_t {
    kVideoPll = 0,            // vid_pll
    kGeneralPurpose0Pll = 1,  // gp0_pll
    kHifiPll = 2,             // hifi_pll
    kMpll1 = 3,               // mpll1
    kFixed1000Mhz = 4,        // fclk_div2
    kFixed800Mhz = 5,         // fclk_div2p5
    kFixed666Mhz = 6,         // fclk_div3
    kFixed285_7Mhz = 7,       // fclk_div7
  };

  static constexpr int kMinDivider = 1;
  static constexpr int kMaxDivider = 128;
  static_assert(kMaxDivider == 1 << (6 - 0 + 1));

  static hwreg::RegisterAddr<MipiDsiPhyClockControl> Get() { return {0x95 * sizeof(uint32_t)}; }

  DEF_ENUM_FIELD(ClockSource, 14, 12, clock_source);

  DEF_BIT(8, enabled);

  // Prefer `Divider()` and `SetDivider()` to accessing the field directly.
  DEF_FIELD(6, 0, divider_minus_one);

  MipiDsiPhyClockControl& SetDivider(int divider) {
    ZX_DEBUG_ASSERT(divider >= kMinDivider);
    ZX_DEBUG_ASSERT(divider <= kMaxDivider);
    return set_divider_minus_one(divider - 1);
  }

  int Divider() const { return divider_minus_one() + 1; }
};

// ## HDMI Clock Tree
//
// The **HDMI Clock Tree** takes the HDMI PLL as its clock input and produces
// the "vid_pll_clk" clock signal to be used by the EE (Everything Else) clock
// tree, which includes the VPU (display engine). It uses a pattern repeater as
// a frequency divider circuit, which is controlled by the
// `HdmiClockTreeControl` register.

// Values for the `pattern_generator_mode_selection` field in `HdmiClockTreeControl`.
enum class HdmiClockTreePatternGeneratorModeSource : uint32_t {
  // Source 0: Repeating the lower 12 bits of the provided pattern.
  kRepeated12BitPattern = 0,
  // Source 1: Repeating the lower 14 bits of the provided pattern.
  kRepeated14BitPattern = 1,
  // Source 2: Repeating the lower 15 bits of the provided pattern.
  kRepeated15BitPattern = 2,
  // Source 3: Repeating a fixed 25-bit pattern:
  //           (MSB) 111 000 111 000 111 000 1111 000 (LSB)
  kFixed25BitPattern = 3,
};

// HHI_VID_PLL_CLK_DIV - Configures the "vid_pll_clk" clock signal.
//
// The HDMI clock tree has a pattern repeater that can repeat a given (or
// fixed) bit pattern, one bit at each clock cycle, which effectively acts as a
// frequency divider of the input clock signal. This register controls the
// behavior of the pattern generator.
//
// A311D Datasheet, Section 8.7.1.3 "HDMI Clock Tree", Page 112-113; Section
//   8.7.6 Register Descriptions, Page 153.
// S905D2 Datasheet, Section 6.6.2.3 "HDMI Clock Tree", Page 96-97; Section
//   6.6.6 Register Descriptions, Page 138-139.
// S905D3 Datasheet, Section 6.7.2.3 "HDMI Clock Tree", Page 96-97; Section
//   6.7.6 Register Descriptions, Page 131.
class HdmiClockTreeControl : public hwreg::RegisterBase<HdmiClockTreeControl, uint32_t> {
 public:
  static hwreg::RegisterAddr<HdmiClockTreeControl> Get() { return {0x68 * sizeof(uint32_t)}; }

  // Bits 31-24 are reserved.
  DEF_RSVDZ_FIELD(23, 20);

  // If false, the output clock vid_pll_clk is gated.
  DEF_BIT(19, clock_output_enabled);

  // If true, the output clock matches the HDMI PLL clock.
  //
  // When this bit is true, the pattern repeater configuration does not
  // influence the output signal.
  //
  // `Pattern()`, `PatternSize()` and `SetFrequencyDividerRatio()` helpers are
  // preferred over direct field manipulations.
  DEF_BIT(18, bypass_pattern_generators);

  // `Pattern()`, `PatternSize()` and `SetFrequencyDividerRatio()` helpers are
  // preferred over direct field manipulations.
  DEF_ENUM_FIELD(HdmiClockTreePatternGeneratorModeSource, 17, 16, pattern_generator_mode_selection);

  // If false, the `preset_pattern` field is ignored when the register is
  // written.
  DEF_BIT(15, preset_pattern_update_enabled);

  // The bits output by the pattern generator, when not in fixed pattern mode.
  //
  // For example, to get a clock signal at 1/5 of the HDMI PLL frequency (with a
  // 60/40 duty cycle), use 15-bit repeater (source 2) and set the pattern to
  // 0b111'00'111'00'111'00 to generate the following pattern (assuming the bits
  // are emitted from the least significant bit to the most significant bit):
  //
  // (output) 0 0 1 1 1 0 0 1 1 1 0 0 1 1 1
  //        1     ______    ______    ______
  //        0 ____      ____      ____
  //
  // The pattern must fulfill the following constraints:
  // - The pattern's least significant bit must be zero.
  // - If PatternSize() is non-zero, the bit `PatternSize() - 1` is the most
  //   significant bit set. In testing, this means the pattern will be at least
  //   `1 << PatternSize()` and less than `1 << (PatternSize() + 1)`.
  // - The number of 1 -> 0 bit transitions (when reading from the most
  //   significant bit to the least significant bit) in `Pattern()` equals
  //   `PatternSize() / (divider_ratio - 1.0)` for divider ratios greater than
  //   one.
  // - The maximum length of a consecutive sequence of ones or zeros will
  //   differ by at most 1 from the minimum length of a consecutive sequence
  //   of ones or zeros.
  //
  // `Pattern()`, `PatternSize()` and `SetFrequencyDividerRatio()` helpers are
  // preferred over direct field manipulations.
  DEF_FIELD(14, 0, pattern_generator_state);

  // The generated signal's period (cycle size), in bits.
  // Returns 0 if the pattern generator is bypassed.
  int PatternSize() const;

  // The pattern repeated by the pattern generator.
  // Returns 0 if the pattern generator is bypassed.
  uint32_t Pattern() const;

  // Frequency division ratios supported by the pattern generator.
  static constexpr uint32_t kSupportedFrequencyDividerRatiosArray[] = {
      ToU28_4(1.0),  ToU28_4(2.0), ToU28_4(2.5),  ToU28_4(3.0),  ToU28_4(3.5),
      ToU28_4(3.75), ToU28_4(4.0), ToU28_4(5.0),  ToU28_4(6.0),  ToU28_4(6.25),
      ToU28_4(7.0),  ToU28_4(7.5), ToU28_4(12.0), ToU28_4(14.0), ToU28_4(15.0),
  };
  static constexpr std::span<const uint32_t> kSupportedFrequencyDividerRatios =
      kSupportedFrequencyDividerRatiosArray;

  // Sets the pattern generator so that it works as a frequency divider with a
  // division ratio of `division_ratio_u28_4`.
  //
  // `division_ratio_u28_4` is a U28.4 format fixed-point fraction with 28
  // integer bits and 4 fractional bits.
  //
  // `division_ratio_u28_4` must be one of the values in
  // `kSupportedFrequencyDividerRatios`.
  HdmiClockTreeControl& SetFrequencyDividerRatio(uint32_t divider_ratio_u28_4);
};

}  // namespace amlogic_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_AMLOGIC_DISPLAY_CLOCK_REGS_H_
