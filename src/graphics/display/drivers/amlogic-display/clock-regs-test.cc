// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/clock-regs.h"

#include <algorithm>
#include <bit>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/drivers/amlogic-display/fixed-point-util.h"

namespace amlogic_display {

namespace {

TEST(VideoClock2Divider, GetSetDivider2) {
  // TODO(https://fxbug.dev/42085985): Use meaningful variable names for registers.
  auto reg = VideoClock2Divider::Get().FromValue(0);
  reg.SetDivider2(1);
  EXPECT_EQ(reg.Divider2(), 1);
  EXPECT_EQ(reg.divider2_minus_one(), 0u);
  reg.SetDivider2(2);
  EXPECT_EQ(reg.Divider2(), 2);
  EXPECT_EQ(reg.divider2_minus_one(), 1u);
  reg.SetDivider2(256);
  EXPECT_EQ(reg.Divider2(), 256);
  EXPECT_EQ(reg.divider2_minus_one(), 255u);
}

TEST(VideoClock1Divider, GetSetDivider1) {
  auto reg = VideoClock1Divider::Get().FromValue(0);
  reg.SetDivider1(1);
  EXPECT_EQ(reg.Divider1(), 1);
  EXPECT_EQ(reg.divider1_minus_one(), 0u);
  reg.SetDivider1(2);
  EXPECT_EQ(reg.Divider1(), 2);
  EXPECT_EQ(reg.divider1_minus_one(), 1u);
  reg.SetDivider1(256);
  EXPECT_EQ(reg.Divider1(), 256);
  EXPECT_EQ(reg.divider1_minus_one(), 255u);
}

TEST(VideoClock1Divider, GetSetDivider0) {
  auto reg = VideoClock1Divider::Get().FromValue(0);
  reg.SetDivider0(1);
  EXPECT_EQ(reg.Divider0(), 1);
  EXPECT_EQ(reg.divider0_minus_one(), 0u);
  reg.SetDivider0(2);
  EXPECT_EQ(reg.Divider0(), 2);
  EXPECT_EQ(reg.divider0_minus_one(), 1u);
  reg.SetDivider0(256);
  EXPECT_EQ(reg.Divider0(), 256);
  EXPECT_EQ(reg.divider0_minus_one(), 255u);
}

TEST(HdmiClockControl, GetSetHdmiSystemClockDivider) {
  auto reg = HdmiClockControl::Get().FromValue(0);
  reg.SetHdmiTxSystemClockDivider(1);
  EXPECT_EQ(reg.HdmiTxSystemClockDivider(), 1);
  EXPECT_EQ(reg.hdmi_tx_system_clock_divider_minus_one(), 0u);
  reg.SetHdmiTxSystemClockDivider(2);
  EXPECT_EQ(reg.HdmiTxSystemClockDivider(), 2);
  EXPECT_EQ(reg.hdmi_tx_system_clock_divider_minus_one(), 1u);
  reg.SetHdmiTxSystemClockDivider(128);
  EXPECT_EQ(reg.HdmiTxSystemClockDivider(), 128);
  EXPECT_EQ(reg.hdmi_tx_system_clock_divider_minus_one(), 127u);
}

TEST(VpuClockCControl, GetSetBranch1MuxDivider) {
  auto reg = VpuClockCControl::Get().FromValue(0);
  reg.SetBranch1MuxDivider(1);
  EXPECT_EQ(reg.Branch1MuxDivider(), 1);
  EXPECT_EQ(reg.branch1_mux_divider_minus_one(), 0u);
  reg.SetBranch1MuxDivider(2);
  EXPECT_EQ(reg.Branch1MuxDivider(), 2);
  EXPECT_EQ(reg.branch1_mux_divider_minus_one(), 1u);
  reg.SetBranch1MuxDivider(128);
  EXPECT_EQ(reg.Branch1MuxDivider(), 128);
  EXPECT_EQ(reg.branch1_mux_divider_minus_one(), 127u);
}

TEST(VpuClockCControl, GetSetBranch0MuxDivider) {
  auto reg = VpuClockCControl::Get().FromValue(0);
  reg.SetBranch0MuxDivider(1);
  EXPECT_EQ(reg.Branch0MuxDivider(), 1);
  EXPECT_EQ(reg.branch0_mux_divider_minus_one(), 0u);
  reg.SetBranch0MuxDivider(2);
  EXPECT_EQ(reg.Branch0MuxDivider(), 2);
  EXPECT_EQ(reg.branch0_mux_divider_minus_one(), 1u);
  reg.SetBranch0MuxDivider(128);
  EXPECT_EQ(reg.Branch0MuxDivider(), 128);
  EXPECT_EQ(reg.branch0_mux_divider_minus_one(), 127u);
}

TEST(VpuClockControl, GetSetBranch1MuxDivider) {
  auto reg = VpuClockControl::Get().FromValue(0);
  reg.SetBranch1MuxDivider(1);
  EXPECT_EQ(reg.Branch1MuxDivider(), 1);
  EXPECT_EQ(reg.branch1_mux_divider_minus_one(), 0u);
  reg.SetBranch1MuxDivider(2);
  EXPECT_EQ(reg.Branch1MuxDivider(), 2);
  EXPECT_EQ(reg.branch1_mux_divider_minus_one(), 1u);
  reg.SetBranch1MuxDivider(128);
  EXPECT_EQ(reg.Branch1MuxDivider(), 128);
  EXPECT_EQ(reg.branch1_mux_divider_minus_one(), 127u);
}

TEST(VpuClockControl, GetSetBranch0MuxDivider) {
  auto reg = VpuClockControl::Get().FromValue(0);
  reg.SetBranch0MuxDivider(1);
  EXPECT_EQ(reg.Branch0MuxDivider(), 1);
  EXPECT_EQ(reg.branch0_mux_divider_minus_one(), 0u);
  reg.SetBranch0MuxDivider(2);
  EXPECT_EQ(reg.Branch0MuxDivider(), 2);
  EXPECT_EQ(reg.branch0_mux_divider_minus_one(), 1u);
  reg.SetBranch0MuxDivider(128);
  EXPECT_EQ(reg.Branch0MuxDivider(), 128);
  EXPECT_EQ(reg.branch0_mux_divider_minus_one(), 127u);
}

TEST(VpuClockBControl, GetSetDivider1) {
  auto reg = VpuClockBControl::Get().FromValue(0);
  reg.SetDivider1(1);
  EXPECT_EQ(reg.Divider1(), 1);
  EXPECT_EQ(reg.divider1_minus_one(), 0u);
  reg.SetDivider1(2);
  EXPECT_EQ(reg.Divider1(), 2);
  EXPECT_EQ(reg.divider1_minus_one(), 1u);
  reg.SetDivider1(16);
  EXPECT_EQ(reg.Divider1(), 16);
  EXPECT_EQ(reg.divider1_minus_one(), 15u);
}

TEST(VpuClockBControl, GetSetDivider2) {
  auto reg = VpuClockBControl::Get().FromValue(0);
  reg.SetDivider2(1);
  EXPECT_EQ(reg.Divider2(), 1);
  EXPECT_EQ(reg.divider2_minus_one(), 0u);
  reg.SetDivider2(2);
  EXPECT_EQ(reg.Divider2(), 2);
  EXPECT_EQ(reg.divider2_minus_one(), 1u);
  reg.SetDivider2(256);
  EXPECT_EQ(reg.Divider2(), 256);
  EXPECT_EQ(reg.divider2_minus_one(), 255u);
}

TEST(VideoAdvancedPeripheralBusClockControl, GetSetBranch1MuxDivider) {
  auto reg = VideoAdvancedPeripheralBusClockControl::Get().FromValue(0);
  reg.SetBranch1MuxDivider(1);
  EXPECT_EQ(reg.Branch1MuxDivider(), 1);
  EXPECT_EQ(reg.branch1_mux_divider_minus_one(), 0u);
  reg.SetBranch1MuxDivider(2);
  EXPECT_EQ(reg.Branch1MuxDivider(), 2);
  EXPECT_EQ(reg.branch1_mux_divider_minus_one(), 1u);
  reg.SetBranch1MuxDivider(128);
  EXPECT_EQ(reg.Branch1MuxDivider(), 128);
  EXPECT_EQ(reg.branch1_mux_divider_minus_one(), 127u);
}

TEST(VideoAdvancedPeripheralBusClockControl, GetSetBranch0MuxDivider) {
  auto reg = VideoAdvancedPeripheralBusClockControl::Get().FromValue(0);
  reg.SetBranch0MuxDivider(1);
  EXPECT_EQ(reg.Branch0MuxDivider(), 1);
  EXPECT_EQ(reg.branch0_mux_divider_minus_one(), 0u);
  reg.SetBranch0MuxDivider(2);
  EXPECT_EQ(reg.Branch0MuxDivider(), 2);
  EXPECT_EQ(reg.branch0_mux_divider_minus_one(), 1u);
  reg.SetBranch0MuxDivider(128);
  EXPECT_EQ(reg.Branch0MuxDivider(), 128);
  EXPECT_EQ(reg.branch0_mux_divider_minus_one(), 127u);
}

TEST(VideoInputMeasureClockControl, GetSetDsiDivider) {
  auto reg = VideoInputMeasureClockControl::Get().FromValue(0);
  reg.SetDsiMeasureClockDivider(1);
  EXPECT_EQ(reg.DsiMeasureClockDivider(), 1);
  EXPECT_EQ(reg.dsi_measure_clock_divider_minus_one(), 0u);
  reg.SetDsiMeasureClockDivider(2);
  EXPECT_EQ(reg.DsiMeasureClockDivider(), 2);
  EXPECT_EQ(reg.dsi_measure_clock_divider_minus_one(), 1u);
  reg.SetDsiMeasureClockDivider(128);
  EXPECT_EQ(reg.DsiMeasureClockDivider(), 128);
  EXPECT_EQ(reg.dsi_measure_clock_divider_minus_one(), 127u);
}

TEST(VideoInputMeasureClockControl, GetSetVideoInputDivider) {
  auto reg = VideoInputMeasureClockControl::Get().FromValue(0);
  reg.SetVideoInputMeasureClockDivider(1);
  EXPECT_EQ(reg.VideoInputMeasureClockDivider(), 1);
  EXPECT_EQ(reg.video_input_measure_clock_divider_minus_one(), 0u);
  reg.SetVideoInputMeasureClockDivider(2);
  EXPECT_EQ(reg.VideoInputMeasureClockDivider(), 2);
  EXPECT_EQ(reg.video_input_measure_clock_divider_minus_one(), 1u);
  reg.SetVideoInputMeasureClockDivider(128);
  EXPECT_EQ(reg.VideoInputMeasureClockDivider(), 128);
  EXPECT_EQ(reg.video_input_measure_clock_divider_minus_one(), 127u);
}

TEST(MipiDsiPhyClockControl, GetSetDivider) {
  auto reg = MipiDsiPhyClockControl::Get().FromValue(0);
  reg.SetDivider(1);
  EXPECT_EQ(reg.Divider(), 1);
  EXPECT_EQ(reg.divider_minus_one(), 0u);
  reg.SetDivider(2);
  EXPECT_EQ(reg.Divider(), 2);
  EXPECT_EQ(reg.divider_minus_one(), 1u);
  reg.SetDivider(128);
  EXPECT_EQ(reg.Divider(), 128);
  EXPECT_EQ(reg.divider_minus_one(), 127u);
}

TEST(HdmiClockTreeControl, PatternSize) {
  auto hdmi_clock_tree_control = HdmiClockTreeControl::Get().FromValue(0);
  hdmi_clock_tree_control.set_reg_value(0).set_bypass_pattern_generators(true);
  EXPECT_EQ(hdmi_clock_tree_control.PatternSize(), 0);

  hdmi_clock_tree_control.set_reg_value(0)
      .set_bypass_pattern_generators(false)
      .set_pattern_generator_state(0b11111'00000'10101)
      .set_pattern_generator_mode_selection(
          HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern);
  EXPECT_EQ(hdmi_clock_tree_control.PatternSize(), 12);

  hdmi_clock_tree_control.set_reg_value(0)
      .set_bypass_pattern_generators(false)
      .set_pattern_generator_state(0b11111'00000'10101)
      .set_pattern_generator_mode_selection(
          HdmiClockTreePatternGeneratorModeSource::kRepeated14BitPattern);
  EXPECT_EQ(hdmi_clock_tree_control.PatternSize(), 14);

  hdmi_clock_tree_control.set_reg_value(0)
      .set_bypass_pattern_generators(false)
      .set_pattern_generator_state(0b11111'00000'10101)
      .set_pattern_generator_mode_selection(
          HdmiClockTreePatternGeneratorModeSource::kRepeated15BitPattern);
  EXPECT_EQ(hdmi_clock_tree_control.PatternSize(), 15);

  hdmi_clock_tree_control.set_reg_value(0)
      .set_bypass_pattern_generators(false)
      .set_pattern_generator_mode_selection(
          HdmiClockTreePatternGeneratorModeSource::kFixed25BitPattern);
  EXPECT_EQ(hdmi_clock_tree_control.PatternSize(), 25);
}

TEST(HdmiClockTreeControl, Pattern) {
  auto hdmi_clock_tree_control = HdmiClockTreeControl::Get().FromValue(0);
  hdmi_clock_tree_control.set_reg_value(0).set_bypass_pattern_generators(true);
  EXPECT_EQ(hdmi_clock_tree_control.Pattern(), 0u);

  hdmi_clock_tree_control.set_reg_value(0)
      .set_bypass_pattern_generators(false)
      .set_pattern_generator_state(0b11111'00000'10101u)
      .set_pattern_generator_mode_selection(
          HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern);
  EXPECT_EQ(hdmi_clock_tree_control.Pattern(), 0b11'00000'10101u);

  hdmi_clock_tree_control.set_reg_value(0)
      .set_bypass_pattern_generators(false)
      .set_pattern_generator_state(0b11111'00000'10101u)
      .set_pattern_generator_mode_selection(
          HdmiClockTreePatternGeneratorModeSource::kRepeated14BitPattern);
  EXPECT_EQ(hdmi_clock_tree_control.Pattern(), 0b1111'00000'10101u);

  hdmi_clock_tree_control.set_reg_value(0)
      .set_bypass_pattern_generators(false)
      .set_pattern_generator_state(0b11111'00000'10101u)
      .set_pattern_generator_mode_selection(
          HdmiClockTreePatternGeneratorModeSource::kRepeated15BitPattern);
  EXPECT_EQ(hdmi_clock_tree_control.Pattern(), 0b11111'00000'10101u);

  hdmi_clock_tree_control.set_reg_value(0)
      .set_bypass_pattern_generators(false)
      .set_pattern_generator_state(0b11111'00000'10101u)
      .set_pattern_generator_mode_selection(
          HdmiClockTreePatternGeneratorModeSource::kFixed25BitPattern);
  EXPECT_EQ(hdmi_clock_tree_control.Pattern(), 0b111'000'111'000'111'000'1111'000u);
}

TEST(HdmiClockTreeControlSetFrequencyDivider, BypassPatternGeneratorIffDividerRatioIsOne) {
  for (const uint32_t divider_ratio_u28_4 :
       HdmiClockTreeControl::kSupportedFrequencyDividerRatios) {
    const double divider_ratio = U28_4ToDouble(divider_ratio_u28_4);
    SCOPED_TRACE(testing::Message() << "Divider ratio: " << divider_ratio);

    auto hdmi_clock_tree_control = HdmiClockTreeControl::Get().FromValue(0);
    hdmi_clock_tree_control.SetFrequencyDividerRatio(divider_ratio_u28_4);
    EXPECT_EQ(hdmi_clock_tree_control.bypass_pattern_generators(), divider_ratio == 1.0);
  }
}

TEST(HdmiClockTreeControlSetFrequencyDivider, LeastSignificantBitIsAlwaysZero) {
  for (const uint32_t divider_ratio_u28_4 :
       HdmiClockTreeControl::kSupportedFrequencyDividerRatios) {
    const double divider_ratio = U28_4ToDouble(divider_ratio_u28_4);
    SCOPED_TRACE(testing::Message() << "Divider ratio: " << divider_ratio);

    auto hdmi_clock_tree_control = HdmiClockTreeControl::Get().FromValue(0);
    hdmi_clock_tree_control.SetFrequencyDividerRatio(divider_ratio_u28_4);
    EXPECT_EQ(hdmi_clock_tree_control.Pattern() & 0x1, 0u);
  }
}

// The bit position of the most significant one, where 0 stands for the least
// significant bit.
//
// Returns -1 if `pattern` doesn't contains any one (i.e. pattern is 0).
int BitOfMostSignificantOne(uint32_t pattern) {
  // This also works for pattern being 0, where countl_zero(0) equals to 32.
  return 31 - std::countl_zero(pattern);
}

TEST(BitOfMostSignificantOne, Correctness) {
  EXPECT_EQ(BitOfMostSignificantOne(0b0000), -1);
  EXPECT_EQ(BitOfMostSignificantOne(0b0001), 0);
  EXPECT_EQ(BitOfMostSignificantOne(0b0010), 1);
  EXPECT_EQ(BitOfMostSignificantOne(0b0100), 2);
  EXPECT_EQ(BitOfMostSignificantOne(0b1000), 3);
  EXPECT_EQ(BitOfMostSignificantOne(0b1111), 3);
  EXPECT_EQ(BitOfMostSignificantOne(0x0001'0000), 16);
  EXPECT_EQ(BitOfMostSignificantOne(0x8000'0000), 31);
}

TEST(HdmiClockTreeControlSetFrequencyDivider, MostSignificantBitIsAtBitPatternSizeMinusOne) {
  for (const uint32_t divider_ratio_u28_4 :
       HdmiClockTreeControl::kSupportedFrequencyDividerRatios) {
    const double divider_ratio = U28_4ToDouble(divider_ratio_u28_4);
    SCOPED_TRACE(testing::Message() << "Divider ratio: " << divider_ratio);

    auto hdmi_clock_tree_control = HdmiClockTreeControl::Get().FromValue(0);
    hdmi_clock_tree_control.SetFrequencyDividerRatio(divider_ratio_u28_4);
    EXPECT_EQ(BitOfMostSignificantOne(hdmi_clock_tree_control.Pattern()),
              hdmi_clock_tree_control.PatternSize() - 1);
  }
}

// Number of 1 (more significant bit, or MSB) to 0 (less significant bit, or
// LSB) transitions in a 32-bit unsigned integer.
//
// For example, the pattern 0b1110011110000 contains 2 transitions from 0 to 1:
// 111 -> 00 and 1111 -> 0000.
//
// The MSB to LSB direction may differ from how the hardware behaves, but is
// sufficient for checking the properties of a clock signal. Actually, it
// always equals to the number of 0 (LSB) to 1 (MSB) transitions.
int NumberOfOneToZeroTransitions(uint32_t pattern) {
  int num_transitions = 0;
  uint32_t previous_bit = pattern & 0x1;
  while (pattern != 0) {
    uint32_t current_bit = pattern & 0x1;
    if (previous_bit == 0 && current_bit == 1) {
      num_transitions++;
    }
    previous_bit = current_bit;
    pattern >>= 1;
  }
  return num_transitions;
}

TEST(NumberOfOneToZeroTransitions, Correctness) {
  EXPECT_EQ(NumberOfOneToZeroTransitions(0b0), 0);
  EXPECT_EQ(NumberOfOneToZeroTransitions(0b1), 0);
  EXPECT_EQ(NumberOfOneToZeroTransitions(0b10), 1);
  EXPECT_EQ(NumberOfOneToZeroTransitions(0b1111110000), 1);
  EXPECT_EQ(NumberOfOneToZeroTransitions(0b111000'1110000), 2);
  EXPECT_EQ(NumberOfOneToZeroTransitions(0b10'10'10'10'10), 5);

  // 0xa == 0b1010. This represents a bit pattern of alternating 1s and 0s.
  EXPECT_EQ(NumberOfOneToZeroTransitions(0xaaaa'aaaa), 16);
}

TEST(HdmiClockTreeControlSetFrequencyDivider, DivisionRatio) {
  for (const uint32_t divider_ratio_u28_4 :
       HdmiClockTreeControl::kSupportedFrequencyDividerRatios) {
    const double expected_divider_ratio = U28_4ToDouble(divider_ratio_u28_4);
    SCOPED_TRACE(testing::Message() << "Divider ratio: " << expected_divider_ratio);

    if (expected_divider_ratio == 1.0) {
      continue;
    }

    auto hdmi_clock_tree_control = HdmiClockTreeControl::Get().FromValue(0);
    hdmi_clock_tree_control.SetFrequencyDividerRatio(divider_ratio_u28_4);
    const double divider_ratio = static_cast<double>(hdmi_clock_tree_control.PatternSize()) /
                                 NumberOfOneToZeroTransitions(hdmi_clock_tree_control.Pattern());

    // For all division ratios in kSupportedFrequencyDividerRatios, the
    // fractional part of division ratio only contains two's powers, so the
    // calculated result must be precise.
    EXPECT_EQ(divider_ratio, expected_divider_ratio);
  }
}

// Splits a 32-bit binary `pattern` to bit segments of continuous zeros or ones
// (excluding the left-padding zeros), and returns a vector of each segment's
// length, from the most significant segments to the least significant segments.
//
// For example, for binary pattern
//   0b0000'11'0000'111'0000000'111111'0000'11 ,
// this function returns
//   {       2,   4,  3,      7,     6,   4, 2}.
std::vector<int> GetContinuousBitSegmentLengths(uint32_t pattern) {
  std::vector<int> segment_lengths;

  int current_segment_number = static_cast<int>(pattern & 0x1);
  int current_segment_length = 0;
  while (pattern != 0) {
    if ((pattern & 0x1) != current_segment_number) {
      segment_lengths.push_back(current_segment_length);
      current_segment_number = static_cast<int>(pattern & 0x1);
      current_segment_length = 1;
    } else {
      current_segment_length++;
    }
    pattern >>= 1;
  }
  if (current_segment_length != 0) {
    segment_lengths.push_back(current_segment_length);
  }

  // `segment_lengths` stores lengths of segments from the least to the most
  // significant bits. We need to return the reversed vector.
  return std::vector<int>(segment_lengths.rbegin(), segment_lengths.rend());
}

TEST(GetContinuousBitSegmentLengths, Correctness) {
  EXPECT_THAT(GetContinuousBitSegmentLengths(0b0), testing::ElementsAre());
  EXPECT_THAT(GetContinuousBitSegmentLengths(0b1), testing::ElementsAre(1));
  EXPECT_THAT(GetContinuousBitSegmentLengths(0b11), testing::ElementsAre(2));
  EXPECT_THAT(GetContinuousBitSegmentLengths(0xffff'ffff), testing::ElementsAre(32));

  // 0x5 == 0b0101. 0x5555'5555 has 31 segments (the most significant bit is a
  // one on bit 30) where each segment is a single bit of 0 or 1.
  EXPECT_THAT(GetContinuousBitSegmentLengths(0x5555'5555),
              testing::ElementsAreArray(std::vector<int>(31, 1)));
  // 0xa == 0b1010. 0x5555'5555 has 32 segments (the most significant bit is a
  // one on bit 31) where each segment is a single bit of 0 or 1.
  EXPECT_THAT(GetContinuousBitSegmentLengths(0xaaaa'aaaa),
              testing::ElementsAreArray(std::vector<int>(32, 1)));

  // 0xf'00'fff'00 == 0b1111'00000000'111111111111'00000000
  EXPECT_THAT(GetContinuousBitSegmentLengths(0xf'00'fff'00), testing::ElementsAre(4, 8, 12, 8));

  EXPECT_THAT(GetContinuousBitSegmentLengths(0b0000'11'0000'111'0000000'111111'0000'11),
              testing::ElementsAre(2, 4, 3, 7, 6, 4, 2));
}

// The maximum length difference (maximum length - minimum length) of
// continuous {0, 1} segments in `pattern` (excluding the left-padding zeros).
//
// For example, for all the {0, 1} segments in pattern
// 0b111'0000'1111111'0000'11:
//
// The maximum segment length is 7 (0b1111111) and the minimum segment length
// is 2 (0b11). Thus the maximum length difference is 7 - 2 = 5.
//
// Returns 0 if there's only zero or one continuous {0, 1} segment.
int MaxLengthDifferenceOfContinuousBitSegments(uint32_t pattern) {
  std::vector<int> segment_lengths = GetContinuousBitSegmentLengths(pattern);
  if (segment_lengths.size() == 0 || segment_lengths.size() == 1) {
    return 0;
  }

  auto max_iter = std::max_element(segment_lengths.begin(), segment_lengths.end());
  auto min_iter = std::min_element(segment_lengths.begin(), segment_lengths.end());
  ZX_DEBUG_ASSERT(max_iter != segment_lengths.end());
  ZX_DEBUG_ASSERT(min_iter != segment_lengths.end());
  return *max_iter - *min_iter;
}

TEST(MaxLengthDifferenceOfContinuousBitSegments, Correctness) {
  EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(0b0), 0);
  EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(0b1), 0);
  EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(0xffff'ffff), 0);

  EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(0b1'0), 0);
  EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(0b1111'00), 2);
  EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(0b111'000'111'0000), 1);
  EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(0b1'000000000'111111111), 8);
  EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(0b111'0000'1111111'0000'11), 5);
}

TEST(HdmiClockTreeControlSetFrequencyDivider, MaxLengthDifferenceBetweenContiguousZeroAndOneRuns) {
  auto hdmi_clock_tree_control = HdmiClockTreeControl::Get().FromValue(0);

  // For perfect (50/50) duty cycle, the length difference between contiguous
  // {0, 1} runs will be 0.
  static constexpr double kDividerRatiosWithPerfectDutyCycle[] = {
      2.0, 4.0, 6.0, 12.0, 14.0,
  };
  for (const double divider_ratio : kDividerRatiosWithPerfectDutyCycle) {
    SCOPED_TRACE(testing::Message() << "Divider ratio: " << divider_ratio);
    hdmi_clock_tree_control.set_reg_value(0).SetFrequencyDividerRatio(ToU28_4(divider_ratio));
    EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(hdmi_clock_tree_control.Pattern()), 0);
  }

  // For non-perfect duty cycle, the length difference between contiguous
  // {0, 1} runs will be 1.
  static constexpr double kDividerRatiosWithNonPerfectDutyCycle[] = {
      2.5, 3.0, 3.5, 3.75, 5.0, 6.25, 7.0, 7.5, 15.0,
  };
  for (const double divider_ratio : kDividerRatiosWithNonPerfectDutyCycle) {
    SCOPED_TRACE(testing::Message() << "Divider ratio: " << divider_ratio);
    hdmi_clock_tree_control.set_reg_value(0).SetFrequencyDividerRatio(ToU28_4(divider_ratio));
    EXPECT_EQ(MaxLengthDifferenceOfContinuousBitSegments(hdmi_clock_tree_control.Pattern()), 1);
  }
}

}  // namespace

}  // namespace amlogic_display
