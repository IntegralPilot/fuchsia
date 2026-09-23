// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/power-mode.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>

#include <gtest/gtest.h>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

namespace {

constexpr PowerMode kOn2(fuchsia_hardware_display_types::wire::PowerMode::kOn);

TEST(PowerModeTest, EqualityIsReflexive) {
  EXPECT_EQ(PowerMode::kOn, PowerMode::kOn);
  EXPECT_EQ(kOn2, kOn2);
  EXPECT_EQ(PowerMode::kOff, PowerMode::kOff);
}

TEST(PowerModeTest, EqualityIsSymmetric) {
  EXPECT_EQ(PowerMode::kOn, kOn2);
  EXPECT_EQ(kOn2, PowerMode::kOn);
}

TEST(PowerModeTest, EqualityForDifferentValues) {
  EXPECT_NE(PowerMode::kOn, PowerMode::kOff);
  EXPECT_NE(PowerMode::kOff, PowerMode::kOn);
  EXPECT_NE(kOn2, PowerMode::kOff);
  EXPECT_NE(PowerMode::kOff, kOn2);
}

TEST(PowerModeTest, ToFidlPowerMode) {
  static constexpr fuchsia_hardware_display_types::wire::PowerMode fidl_power_mode =
      PowerMode::kOn.ToFidl();
  EXPECT_EQ(fuchsia_hardware_display_types::wire::PowerMode::kOn, fidl_power_mode);
}

TEST(PowerModeTest, ToPowerModeWithFidlValue) {
  static constexpr PowerMode power_mode(fuchsia_hardware_display_types::wire::PowerMode::kOn);
  EXPECT_EQ(PowerMode::kOn, power_mode);
}

TEST(PowerModeTest, ValueForLogging) {
  EXPECT_EQ(static_cast<uint32_t>(fuchsia_hardware_display_types::wire::PowerMode::kOn),
            PowerMode::kOn.ValueForLogging());
}

TEST(PowerModeTest, FidlConversionRoundtrip) {
  EXPECT_EQ(PowerMode::kOff, PowerMode(PowerMode::kOff.ToFidl()));
  EXPECT_EQ(PowerMode::kOn, PowerMode(PowerMode::kOn.ToFidl()));
  EXPECT_EQ(PowerMode::kDoze, PowerMode(PowerMode::kDoze.ToFidl()));
  EXPECT_EQ(PowerMode::kDozeSuspend, PowerMode(PowerMode::kDozeSuspend.ToFidl()));
}

TEST(PowerMode, ToString) {
  EXPECT_EQ(PowerMode::kOff.ToString(), "Off");
  EXPECT_EQ(PowerMode::kOn.ToString(), "On");
  EXPECT_EQ(PowerMode::kDoze.ToString(), "Doze");
  EXPECT_EQ(PowerMode::kDozeSuspend.ToString(), "DozeSuspend");
}

#if __cplusplus >= 202002L
TEST(PowerMode, Format) {
  EXPECT_EQ(std::format("{}", PowerMode::kOff), "Off");
  EXPECT_EQ(std::format("{}", PowerMode::kOn), "On");
  EXPECT_EQ(std::format("{:>10}", PowerMode::kOff), "       Off");
}
#endif  // __cplusplus >= 202002L

}  // namespace

}  // namespace display
