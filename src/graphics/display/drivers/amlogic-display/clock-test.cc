// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/clock.h"

#include <lib/device-protocol/display-panel.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>

#include <cstdint>
#include <vector>

#include <gtest/gtest.h>

#include "src/graphics/display/drivers/amlogic-display/panel-config.h"
#include "src/lib/testing/predicates/status.h"

namespace amlogic_display {

namespace {

// All the PanelConfig pointers are non-null in the returned array.
const std::vector<const PanelConfig*> kPanelConfigsForTesting = [] {
  std::vector<const PanelConfig*> panel_configs;
  const display::PanelType kPanelIds[] = {
      display::PanelType::kBoeTv070wsmFitipowerJd9364Astro,
      display::PanelType::kInnoluxP070acbFitipowerJd9364,
      display::PanelType::kInnoluxP101dezFitipowerJd9364,
      display::PanelType::kBoeTv101wxmFitipowerJd9364,
      display::PanelType::kKdKd070d82FitipowerJd9364,
      display::PanelType::kBoeTv070wsmFitipowerJd9364Nelson,
  };
  for (const display::PanelType panel : kPanelIds) {
    const PanelConfig* panel_config = GetPanelConfig(panel);
    ZX_ASSERT(panel_config != nullptr);
    panel_configs.push_back(panel_config);
  }
  return panel_configs;
}();

class AmlogicDisplayClockTest : public ::testing::Test {
 private:
  fdf_testing::ScopedGlobalLogger logger_;
};

// For now, simply test that timing calculations don't segfault.
TEST_F(AmlogicDisplayClockTest, PanelTiming) {
  for (const PanelConfig* panel_config : kPanelConfigsForTesting) {
    ASSERT_NE(panel_config, nullptr);
    SCOPED_TRACE(::testing::Message() << panel_config->name);
    Clock::CalculateLcdTiming(panel_config->display_timing);
  }
}

TEST_F(AmlogicDisplayClockTest, PllTiming_ValidMode) {
  for (const PanelConfig* panel_config : kPanelConfigsForTesting) {
    ASSERT_NE(panel_config, nullptr);
    SCOPED_TRACE(::testing::Message() << panel_config->name);
    zx::result<HdmiPllConfigForMipiDsi> pll_r =
        Clock::GenerateHPLL(panel_config->display_timing.pixel_clock_frequency_hz,
                            panel_config->maximum_per_data_lane_bit_per_second());
    EXPECT_OK(pll_r);
  }
}

// The LCD vendor-provided display settings hardcode the HDMI PLL / DSI
// clock ratio while the settings below requires the clock ratios to be
// calculated automatically.
//
// The following tests ensure that the calculated clock ratios match the
// hardcoded values removed in Ie2c4721b14a92977ef31dd2951dc4cac207cb60e.

class PllTimingHdmiPllClockRatioCalculatedCorrectly : public ::testing::Test {
 private:
  fdf_testing::ScopedGlobalLogger logger_;
};

TEST_F(PllTimingHdmiPllClockRatioCalculatedCorrectly, BoeTv070wsmFitipowerJd9364Astro) {
  const PanelConfig* panel_config =
      GetPanelConfig(display::PanelType::kBoeTv070wsmFitipowerJd9364Astro);
  ASSERT_NE(panel_config, nullptr);
  zx::result<HdmiPllConfigForMipiDsi> pll_config =
      Clock::GenerateHPLL(panel_config->display_timing.pixel_clock_frequency_hz,
                          panel_config->maximum_per_data_lane_bit_per_second());
  static constexpr int kExpectedHdmiPllClockRatio = 8;
  EXPECT_OK(pll_config);
  EXPECT_EQ(kExpectedHdmiPllClockRatio, static_cast<int>(pll_config->clock_factor));
}

TEST_F(PllTimingHdmiPllClockRatioCalculatedCorrectly, InnoluxP070acbFitipowerJd9364) {
  const PanelConfig* panel_config =
      GetPanelConfig(display::PanelType::kInnoluxP070acbFitipowerJd9364);
  ASSERT_NE(panel_config, nullptr);
  zx::result<HdmiPllConfigForMipiDsi> pll_config =
      Clock::GenerateHPLL(panel_config->display_timing.pixel_clock_frequency_hz,
                          panel_config->maximum_per_data_lane_bit_per_second());
  static constexpr int kExpectedHdmiPllClockRatio = 8;
  EXPECT_OK(pll_config);
  EXPECT_EQ(kExpectedHdmiPllClockRatio, static_cast<int>(pll_config->clock_factor));
}

TEST_F(PllTimingHdmiPllClockRatioCalculatedCorrectly, InnoluxP101dezFitipowerJd9364) {
  const PanelConfig* panel_config =
      GetPanelConfig(display::PanelType::kInnoluxP101dezFitipowerJd9364);
  ASSERT_NE(panel_config, nullptr);
  zx::result<HdmiPllConfigForMipiDsi> pll_config =
      Clock::GenerateHPLL(panel_config->display_timing.pixel_clock_frequency_hz,
                          panel_config->maximum_per_data_lane_bit_per_second());
  static constexpr int kExpectedHdmiPllClockRatio = 8;
  EXPECT_OK(pll_config);
  EXPECT_EQ(kExpectedHdmiPllClockRatio, static_cast<int>(pll_config->clock_factor));
}

TEST_F(PllTimingHdmiPllClockRatioCalculatedCorrectly, BoeTv101wxmFitipowerJd9364) {
  const PanelConfig* panel_config = GetPanelConfig(display::PanelType::kBoeTv101wxmFitipowerJd9364);
  ASSERT_NE(panel_config, nullptr);
  zx::result<HdmiPllConfigForMipiDsi> pll_config =
      Clock::GenerateHPLL(panel_config->display_timing.pixel_clock_frequency_hz,
                          panel_config->maximum_per_data_lane_bit_per_second());
  static constexpr int kExpectedHdmiPllClockRatio = 8;
  EXPECT_OK(pll_config);
  EXPECT_EQ(kExpectedHdmiPllClockRatio, static_cast<int>(pll_config->clock_factor));
}

}  // namespace

}  // namespace amlogic_display
