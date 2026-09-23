// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/clock-regs.h"

#include <zircon/assert.h>

#include <algorithm>

#include "src/graphics/display/drivers/amlogic-display/fixed-point-util.h"

namespace amlogic_display {

int HdmiClockTreeControl::PatternSize() const {
  if (bypass_pattern_generators()) {
    return 0;
  }
  switch (pattern_generator_mode_selection()) {
    case HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern:
      return 12;
    case HdmiClockTreePatternGeneratorModeSource::kRepeated14BitPattern:
      return 14;
    case HdmiClockTreePatternGeneratorModeSource::kRepeated15BitPattern:
      return 15;
    case HdmiClockTreePatternGeneratorModeSource::kFixed25BitPattern:
      return 25;
  }
}

uint32_t HdmiClockTreeControl::Pattern() const {
  if (bypass_pattern_generators()) {
    return 0;
  }

  switch (pattern_generator_mode_selection()) {
    case HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern:
    case HdmiClockTreePatternGeneratorModeSource::kRepeated14BitPattern:
    case HdmiClockTreePatternGeneratorModeSource::kRepeated15BitPattern: {
      uint32_t mask = (1 << PatternSize()) - 1;
      return pattern_generator_state() & mask;
    }
    case HdmiClockTreePatternGeneratorModeSource::kFixed25BitPattern:
      return 0b111'000'111'000'111'000'1111'000;
  }
}

HdmiClockTreeControl& HdmiClockTreeControl::SetFrequencyDividerRatio(uint32_t divider_ratio_u28_4) {
  ZX_DEBUG_ASSERT(std::find(kSupportedFrequencyDividerRatios.begin(),
                            kSupportedFrequencyDividerRatios.end(),
                            divider_ratio_u28_4) != kSupportedFrequencyDividerRatios.end());
  // All register field values are from Amlogic-provided code.
  switch (divider_ratio_u28_4) {
    case ToU28_4(1.0):
      return set_bypass_pattern_generators(true)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern)
          .set_pattern_generator_state(0b111'111'111'111'111);
    case ToU28_4(2.0):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern)
          .set_pattern_generator_state(0b10'10'10'10'10'10);
    case ToU28_4(2.5):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated15BitPattern)
          .set_pattern_generator_state(0b10'100'10'100'10'100);
    case ToU28_4(3.0):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern)
          .set_pattern_generator_state(0b110'110'110'110);
    case ToU28_4(3.5):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated14BitPattern)
          .set_pattern_generator_state(0b110'110'1100'1100);
    case ToU28_4(3.75):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated15BitPattern)
          .set_pattern_generator_state(0b1100'1100'1100'110);
    case ToU28_4(4.0):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern)
          .set_pattern_generator_state(0b1100'1100'1100);
    case ToU28_4(5.0):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated15BitPattern)
          .set_pattern_generator_state(0b11100'11100'11100);
    case ToU28_4(6.0):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern)
          .set_pattern_generator_state(0b111000'111000);
    case ToU28_4(6.25):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kFixed25BitPattern)
          .set_pattern_generator_state(0);
    case ToU28_4(7.0):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated14BitPattern)
          .set_pattern_generator_state(0b1111000'1111000);
    case ToU28_4(7.5):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated15BitPattern)
          .set_pattern_generator_state(0b1111000'11110000);
    case ToU28_4(12.0):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated12BitPattern)
          .set_pattern_generator_state(0b111111000000);
    case ToU28_4(14.0):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated14BitPattern)
          .set_pattern_generator_state(0b11111110000000);
    case ToU28_4(15.0):
      return set_bypass_pattern_generators(false)
          .set_pattern_generator_mode_selection(
              HdmiClockTreePatternGeneratorModeSource::kRepeated15BitPattern)
          .set_pattern_generator_state(0b111111110000000);
      break;
  }
  ZX_ASSERT_MSG(false, "Invalid predefined division ratio: 0x%x (%.2f)", divider_ratio_u28_4,
                U28_4ToDouble(divider_ratio_u28_4));
}

}  // namespace amlogic_display
