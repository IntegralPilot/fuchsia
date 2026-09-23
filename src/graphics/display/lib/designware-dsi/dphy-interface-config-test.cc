// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/designware-dsi/dphy-interface-config.h"

#include <chrono>

#include <gtest/gtest.h>

namespace designware_dsi {

namespace {

TEST(PicosecondsTest, OnePicosecondIsOneTrillionthOfASecond) {
  static constexpr int64_t kOneTrillion = 1'000'000'000'000;
  EXPECT_EQ(std::chrono::seconds(1) / Picoseconds(1), kOneTrillion);
}

TEST(DphyInterfaceConfigTest, HighSpeedModeDataLaneTransmissionRate) {
  static constexpr DphyInterfaceConfig kDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_EQ(kDphyInterfaceConfig.high_speed_mode_data_lane_bits_per_second(), 800'000'000);
  EXPECT_EQ(kDphyInterfaceConfig.high_speed_mode_data_lane_bytes_per_second(), 100'000'000);
}

TEST(DphyInterfaceConfigTest, EscapeModeDataLaneTransmissionRate) {
  static constexpr DphyInterfaceConfig kDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_EQ(kDphyInterfaceConfig.escape_mode_data_lane_bits_per_second(), 20'000'000);
  EXPECT_EQ(kDphyInterfaceConfig.escape_mode_data_lane_bytes_per_second(), 2'500'000);
}

TEST(DphyInterfaceConfigTest, UnitInverval) {
  static constexpr DphyInterfaceConfig kDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  // Clock signal period = 1 / 400'000'000 Hz = 2.5 ns = 2 * Unit Interval,
  // so Unit Interval = 1.25 ns.
  EXPECT_EQ(kDphyInterfaceConfig.unit_interval(), Picoseconds(1'250));
}

TEST(DphyInterfaceConfigTest, LaneByteClockPeriod) {
  static constexpr DphyInterfaceConfig kDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  // Clock signal period = 1 / 400'000'000 Hz = 2.5 ns = 2 * Unit Interval,
  // so Unit Interval = 1.25 ns.
  EXPECT_EQ(kDphyInterfaceConfig.unit_interval(), Picoseconds(1'250));

  // lane byte clock period = Unit Interval * 8 = 10 ns.
  EXPECT_EQ(kDphyInterfaceConfig.lane_byte_clock_period(), Picoseconds(10'000));
}

TEST(DphyInterfaceConfigTest, DphyLaneModeTransitionDurations) {
  static constexpr DphyInterfaceConfig kDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  // Clock signal period = 1 / 400'000'000 Hz = 2.5 ns = 2 * Unit Interval,
  // so Unit Interval = 1.25 ns, lane byte clock period = Unit Interval * 8 = 10 ns.
  EXPECT_EQ(kDphyInterfaceConfig.lane_byte_clock_period(), Picoseconds(10'000));

  EXPECT_EQ(kDphyInterfaceConfig.max_data_lane_hs_to_lp_transition_duration(),
            Picoseconds(100'000));
  EXPECT_EQ(kDphyInterfaceConfig.max_data_lane_lp_to_hs_transition_duration(),
            Picoseconds(200'000));
  EXPECT_EQ(kDphyInterfaceConfig.max_clock_lane_hs_to_lp_transition_duration(),
            Picoseconds(300'000));
  EXPECT_EQ(kDphyInterfaceConfig.max_clock_lane_lp_to_hs_transition_duration(),
            Picoseconds(400'000));
}

TEST(DphyInterfaceConfigTest, Valid) {
  static constexpr DphyInterfaceConfig kDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_TRUE(kDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest, TooFewDataLanes) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 0,  // Invalid
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_LT(kInvalidDphyInterfaceConfig.data_lane_count, kMinSupportedDphyDataLaneCount);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest, TooManyDataLanes) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 5,  // Invalid
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_GT(kInvalidDphyInterfaceConfig.data_lane_count, kMaxSupportedDphyDataLaneCount);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest, HighSpeedModeClockLaneFrequencyNonPositive) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfigWithNegativeFrequency = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = -1,  // Invalid
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_FALSE(kInvalidDphyInterfaceConfigWithNegativeFrequency.IsValid());

  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfigWithZeroFrequency = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 0,  // Invalid
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_FALSE(kInvalidDphyInterfaceConfigWithZeroFrequency.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest, EscapeModeClockLaneFrequencyNonPositive) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfigWithNegativeFrequency = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = -1,  // Invalid

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_FALSE(kInvalidDphyInterfaceConfigWithNegativeFrequency.IsValid());

  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfigWithZeroFrequency = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 0,  // Invalid

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_FALSE(kInvalidDphyInterfaceConfigWithZeroFrequency.IsValid());
}

TEST(
    InvalidDphyInterfaceConfigTest,
    EscapeModeClockLaneFrequencyDoesNotDivideHighSpeedModeByteClockFrequencyWithPositiveErrorValid) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'004,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_EQ(kInvalidDphyInterfaceConfig.high_speed_mode_data_lane_bytes_per_second(), 100'000'001);
  EXPECT_TRUE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(
    InvalidDphyInterfaceConfigTest,
    EscapeModeClockLaneFrequencyDoesNotDivideHighSpeedModeByteClockFrequencyWithNegativeErrorValid) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 399'999'996,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_EQ(kInvalidDphyInterfaceConfig.high_speed_mode_data_lane_bytes_per_second(), 99'999'999);
  EXPECT_TRUE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(
    InvalidDphyInterfaceConfigTest,
    EscapeModeClockLaneFrequencyDoesNotDivideHighSpeedModeByteClockFrequencyWithTooLargeErrorInvalid) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 450'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_EQ(kInvalidDphyInterfaceConfig.high_speed_mode_data_lane_bytes_per_second(), 112'500'000);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest, HighSpeedByteClockToEscapeClockLaneFrequencyRatioOverlyLarge) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 1'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_EQ(kInvalidDphyInterfaceConfig.high_speed_mode_data_lane_bytes_per_second(), 100'000'000);
  EXPECT_GT(kInvalidDphyInterfaceConfig.high_speed_mode_data_lane_bytes_per_second() /
                kInvalidDphyInterfaceConfig.escape_mode_clock_lane_frequency_hz,
            kMaxHighSpeedByteClockToEscapeClockFrequencyRatio);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest,
     MaxDataLaneHsToLpTransitionDurationLaneByteClockCyclesOverlySmall) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = -1,  // Invalid
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_LT(
      kInvalidDphyInterfaceConfig.max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles,
      kMinPossibleMaxDataLaneHsToLpTransitionDurationLaneByteClockCycles);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest,
     MaxDataLaneHsToLpTransitionDurationLaneByteClockCyclesOverlyLarge) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 1024,  // Invalid
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_GT(
      kInvalidDphyInterfaceConfig.max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles,
      kMaxPossibleMaxDataLaneHsToLpTransitionDurationLaneByteClockCycles);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest,
     MaxDataLaneLpToHsTransitionDurationLaneByteClockCyclesOverlySmall) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = -1,  // Invalid
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_LT(
      kInvalidDphyInterfaceConfig.max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles,
      kMinPossibleMaxDataLaneLpToHsTransitionDurationLaneByteClockCycles);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest,
     MaxDataLaneLpToHsTransitionDurationLaneByteClockCyclesOverlyLarge) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 1024,  // Invalid
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_GT(
      kInvalidDphyInterfaceConfig.max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles,
      kMaxPossibleMaxDataLaneLpToHsTransitionDurationLaneByteClockCycles);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest,
     MaxClockLaneHsToLpTransitionDurationLaneByteClockCyclesOverlySmall) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = -1,  // Invalid
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_LT(kInvalidDphyInterfaceConfig
                .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles,
            kMinPossibleMaxClockLaneHsToLpTransitionDurationLaneByteClockCycles);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest,
     MaxClockLaneHsToLpTransitionDurationLaneByteClockCyclesOverlyLarge) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 1024,  // Invalid
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 40,
  };

  EXPECT_GT(kInvalidDphyInterfaceConfig
                .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles,
            kMaxPossibleMaxClockLaneHsToLpTransitionDurationLaneByteClockCycles);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest,
     MaxClockLaneLpToHsTransitionDurationLaneByteClockCyclesOverlySmall) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = -1,  // Invalid
  };

  EXPECT_LT(kInvalidDphyInterfaceConfig
                .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles,
            kMinPossibleMaxClockLaneLpToHsTransitionDurationLaneByteClockCycles);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

TEST(InvalidDphyInterfaceConfigTest,
     MaxClockLaneLpToHsTransitionDurationLaneByteClockCyclesOverlyLarge) {
  static constexpr DphyInterfaceConfig kInvalidDphyInterfaceConfig = {
      .data_lane_count = 4,
      .clock_lane_mode_automatic_control_enabled = true,

      .high_speed_mode_clock_lane_frequency_hz = 400'000'000,
      .escape_mode_clock_lane_frequency_hz = 10'000'000,

      .max_data_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 10,
      .max_data_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 20,
      .max_clock_lane_hs_to_lp_transition_duration_lane_byte_clock_cycles = 30,
      .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles = 1024,  // Invalid
  };

  EXPECT_GT(kInvalidDphyInterfaceConfig
                .max_clock_lane_lp_to_hs_transition_duration_lane_byte_clock_cycles,
            kMaxPossibleMaxClockLaneLpToHsTransitionDurationLaneByteClockCycles);
  EXPECT_FALSE(kInvalidDphyInterfaceConfig.IsValid());
}

}  // namespace

}  // namespace designware_dsi
