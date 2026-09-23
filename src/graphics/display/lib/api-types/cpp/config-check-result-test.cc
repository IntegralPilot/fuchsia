// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>

#include <gtest/gtest.h>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

namespace {

constexpr ConfigCheckResult kTooManyDisplays2(
    fuchsia_hardware_display_types::wire::ConfigResult::kTooManyDisplays);

TEST(ConfigCheckResultTest, EqualityIsReflexive) {
  EXPECT_EQ(ConfigCheckResult::kTooManyDisplays, ConfigCheckResult::kTooManyDisplays);
  EXPECT_EQ(kTooManyDisplays2, kTooManyDisplays2);
  EXPECT_EQ(ConfigCheckResult::kInvalidConfig, ConfigCheckResult::kInvalidConfig);
}

TEST(ConfigCheckResultTest, EqualityIsSymmetric) {
  EXPECT_EQ(ConfigCheckResult::kTooManyDisplays, kTooManyDisplays2);
  EXPECT_EQ(kTooManyDisplays2, ConfigCheckResult::kTooManyDisplays);
}

TEST(ConfigCheckResultTest, EqualityForDifferentValues) {
  EXPECT_NE(ConfigCheckResult::kTooManyDisplays, ConfigCheckResult::kInvalidConfig);
  EXPECT_NE(ConfigCheckResult::kInvalidConfig, ConfigCheckResult::kTooManyDisplays);
  EXPECT_NE(kTooManyDisplays2, ConfigCheckResult::kInvalidConfig);
  EXPECT_NE(ConfigCheckResult::kInvalidConfig, kTooManyDisplays2);
}

TEST(ConfigCheckResultTest, ToFidlConfigCheckResult) {
  static constexpr fuchsia_hardware_display_types::wire::ConfigResult fidl_transformation =
      ConfigCheckResult::kTooManyDisplays.ToFidl();
  EXPECT_EQ(fuchsia_hardware_display_types::wire::ConfigResult::kTooManyDisplays,
            fidl_transformation);
}

TEST(ConfigCheckResultTest, ToConfigCheckResultWithFidlValue) {
  static constexpr ConfigCheckResult transformation(
      fuchsia_hardware_display_types::wire::ConfigResult::kTooManyDisplays);
  EXPECT_EQ(ConfigCheckResult::kTooManyDisplays, transformation);
}

TEST(ConfigCheckResultTest, ValueForLogging) {
  EXPECT_EQ(
      static_cast<uint32_t>(fuchsia_hardware_display_types::wire::ConfigResult::kTooManyDisplays),
      ConfigCheckResult::kTooManyDisplays.ValueForLogging());
}

TEST(ConfigCheckResultTest, FidlConversionRoundtrip) {
  EXPECT_EQ(ConfigCheckResult::kTooManyDisplays,
            ConfigCheckResult(ConfigCheckResult::kTooManyDisplays.ToFidl()));
  EXPECT_EQ(ConfigCheckResult::kInvalidConfig,
            ConfigCheckResult(ConfigCheckResult::kInvalidConfig.ToFidl()));
}

TEST(ConfigCheckResult, ToString) {
  EXPECT_EQ(ConfigCheckResult::kOk.ToString(), "Ok");
  EXPECT_EQ(ConfigCheckResult::kEmptyConfig.ToString(), "EmptyConfig");
  EXPECT_EQ(ConfigCheckResult::kInvalidConfig.ToString(), "InvalidConfig");
  EXPECT_EQ(ConfigCheckResult::kUnsupportedConfig.ToString(), "UnsupportedConfig");
  EXPECT_EQ(ConfigCheckResult::kTooManyDisplays.ToString(), "TooManyDisplays");
  EXPECT_EQ(ConfigCheckResult::kUnsupportedDisplayModes.ToString(), "UnsupportedDisplayModes");
}

#if __cplusplus >= 202002L
TEST(ConfigCheckResult, Format) {
  EXPECT_EQ(std::format("{}", ConfigCheckResult::kOk), "Ok");
  EXPECT_EQ(std::format("{}", ConfigCheckResult::kEmptyConfig), "EmptyConfig");
  EXPECT_EQ(std::format("{:>8}", ConfigCheckResult::kOk), "      Ok");
}
#endif  // __cplusplus >= 202002L

}  // namespace

}  // namespace display
