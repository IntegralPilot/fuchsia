// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/backlight-state.h"

#include <fidl/fuchsia.hardware.backlight/cpp/wire.h>

#include <optional>

#include <gtest/gtest.h>

namespace display {

namespace {

constexpr BacklightState kHalfStrength({
    .on = true,
    .brightness_fraction = 0.5,
    .brightness_nits = 400.0,
});

constexpr BacklightState kHalfStrength2({
    .on = true,
    .brightness_fraction = 0.5,
    .brightness_nits = 400.0,
});

constexpr BacklightState kOnZeroStrength({
    .on = true,
    .brightness_fraction = 0.0,
    .brightness_nits = 0.0,
});

TEST(BacklightStateTest, EqualityIsReflexive) {
  EXPECT_EQ(kHalfStrength, kHalfStrength);
  EXPECT_EQ(kHalfStrength2, kHalfStrength2);
  EXPECT_EQ(kOnZeroStrength, kOnZeroStrength);
}

TEST(BacklightStateTest, EqualityIsSymmetric) {
  EXPECT_EQ(kHalfStrength, kHalfStrength2);
  EXPECT_EQ(kHalfStrength2, kHalfStrength);
}

TEST(BacklightStateTest, EqualityForDifferentFractions) {
  static constexpr BacklightState kQuarterStrengthBiggerBacklight({
      .on = true,
      .brightness_fraction = 0.25,
      .brightness_nits = 400.0,
  });
  EXPECT_NE(kHalfStrength, kQuarterStrengthBiggerBacklight);
  EXPECT_NE(kQuarterStrengthBiggerBacklight, kHalfStrength);
}

TEST(BacklightStateTest, EqualityForDifferentNits) {
  static constexpr BacklightState kHalfStrengthSmallerBacklight({
      .on = true,
      .brightness_fraction = 0.5,
      .brightness_nits = 200.0,
  });
  EXPECT_NE(kHalfStrength, kHalfStrengthSmallerBacklight);
  EXPECT_NE(kHalfStrengthSmallerBacklight, kHalfStrength);
}

TEST(BacklightStateTest, EqualityForDifferentAbsoluteSupportLevel) {
  static constexpr BacklightState kHalfStrengthSmallerBacklight({
      .on = true,
      .brightness_fraction = 0.5,
      .brightness_nits = std::nullopt,
  });
  EXPECT_NE(kHalfStrength, kHalfStrengthSmallerBacklight);
  EXPECT_NE(kHalfStrengthSmallerBacklight, kHalfStrength);
}

TEST(BacklightStateTest, EqualityForOnOff) {
  static constexpr BacklightState kOff({
      .on = false,
      .brightness_fraction = 0.0,
      .brightness_nits = 0.0,
  });
  EXPECT_NE(kOnZeroStrength, kOff);
  EXPECT_NE(kOff, kOnZeroStrength);
}

TEST(BacklightStateTest, FromDesignatedInitializerOn) {
  static constexpr BacklightState state({
      .on = true,
      .brightness_fraction = 0.5,
      .brightness_nits = 400.0,
  });
  EXPECT_EQ(true, state.on());
  EXPECT_EQ(0.5, state.brightness_fraction());
  EXPECT_EQ(400.0, state.brightness_nits());
}

TEST(BacklightStateTest, FromDesignatedInitializerOff) {
  static constexpr BacklightState state({
      .on = false,
      .brightness_fraction = 0.0,
      .brightness_nits = 0.0,
  });
  EXPECT_EQ(false, state.on());
  EXPECT_EQ(0.0, state.brightness_fraction());
  EXPECT_EQ(0.0, state.brightness_nits());
}

TEST(BacklightStateTest, FromDesignatedInitializerOnNoAbsoluteInfo) {
  static constexpr BacklightState state({
      .on = true,
      .brightness_fraction = 0.5,
      .brightness_nits = std::nullopt,
  });
  EXPECT_EQ(true, state.on());
  EXPECT_EQ(0.5, state.brightness_fraction());
  EXPECT_EQ(std::nullopt, state.brightness_nits());
}

TEST(BacklightStateTest, FromDesignatedInitializerOffNoAbsoluteInfo) {
  static constexpr BacklightState state({
      .on = false,
      .brightness_fraction = 0.0,
      .brightness_nits = std::nullopt,
  });
  EXPECT_EQ(false, state.on());
  EXPECT_EQ(0.0, state.brightness_fraction());
  EXPECT_EQ(std::nullopt, state.brightness_nits());
}

TEST(BacklightStateTest, FromFidlBacklightStateUseAbsoluteBrightness) {
  static constexpr fuchsia_hardware_backlight::wire::State fidl_state = {
      .backlight_on = true,
      .brightness = 400.0,
  };

  static constexpr BacklightState state =
      BacklightState::From(fidl_state, /*use_absolute_brightness=*/true);
  EXPECT_EQ(true, state.on());
  EXPECT_EQ(0.0, state.brightness_fraction());
  EXPECT_EQ(400.0, state.brightness_nits());
}

TEST(BacklightStateTest, FromFidlBacklightStateUseFractionalBrightness) {
  static constexpr fuchsia_hardware_backlight::wire::State fidl_state = {
      .backlight_on = true,
      .brightness = 0.5,
  };

  static constexpr BacklightState state =
      BacklightState::From(fidl_state, /*use_absolute_brightness=*/false);
  EXPECT_EQ(true, state.on());
  EXPECT_EQ(0.5, state.brightness_fraction());
  EXPECT_EQ(std::nullopt, state.brightness_nits());
}

TEST(BacklightStateTest, ToFidlBacklightStateUseAbsoluteBrightness) {
  static constexpr BacklightState state({
      .on = true,
      .brightness_fraction = 0.5,
      .brightness_nits = 400.0,
  });

  static constexpr fuchsia_hardware_backlight::wire::State fidl_state =
      state.ToFidl(/*use_absolute_brightness=*/true);
  EXPECT_EQ(true, fidl_state.backlight_on);
  EXPECT_EQ(400.0, fidl_state.brightness);
}

TEST(BacklightStateTest, ToFidlBacklightStateUseFractionBrightness) {
  static constexpr BacklightState state({
      .on = true,
      .brightness_fraction = 0.5,
      .brightness_nits = 400.0,
  });

  static constexpr fuchsia_hardware_backlight::wire::State fidl_state =
      state.ToFidl(/*use_absolute_brightness=*/false);
  EXPECT_EQ(true, fidl_state.backlight_on);
  EXPECT_EQ(0.5, fidl_state.brightness);
}

TEST(BacklightStateTest, IsValidFidlOffZeroBrightness) {
  EXPECT_TRUE(BacklightState::IsValid(
      fuchsia_hardware_backlight::wire::State{
          .backlight_on = false,
          .brightness = 0.0,
      },
      /*use_absolute_brightness=*/true));

  EXPECT_TRUE(BacklightState::IsValid(
      fuchsia_hardware_backlight::wire::State{
          .backlight_on = false,
          .brightness = 0.0,
      },
      /*use_absolute_brightness=*/false));
}

TEST(BacklightStateTest, IsValidFidlOffNonzeroBrightness) {
  EXPECT_FALSE(BacklightState::IsValid(
      fuchsia_hardware_backlight::wire::State{
          .backlight_on = false,
          .brightness = 400.0,
      },
      /*use_absolute_brightness=*/true));

  EXPECT_FALSE(BacklightState::IsValid(
      fuchsia_hardware_backlight::wire::State{
          .backlight_on = false,
          .brightness = 0.5,
      },
      /*use_absolute_brightness=*/false));
}

TEST(BacklightStateTest, IsValidFidlNegativeNormalizedBrightness) {
  EXPECT_FALSE(BacklightState::IsValid(
      fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = -0.5,
      },
      /*use_absolute_brightness=*/false));
}

TEST(BacklightStateTest, IsValidFidlHalfNormalizedBrightness) {
  EXPECT_TRUE(BacklightState::IsValid(
      fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = 0.5,
      },
      /*use_absolute_brightness=*/false));
}

TEST(BacklightStateTest, IsValidFidlFullNormalizedBrightness) {
  EXPECT_TRUE(BacklightState::IsValid(
      fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = 1.0,
      },
      /*use_absolute_brightness=*/false));
}

TEST(BacklightStateTest, IsValidFidlNegativeAbsoluteBrightness) {
  EXPECT_FALSE(BacklightState::IsValid(
      fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = -0.5,
      },
      /*use_absolute_brightness=*/true));
}

TEST(BacklightStateTest, IsValidFidlLargeAbsoluteBrightness) {
  EXPECT_TRUE(BacklightState::IsValid(
      fuchsia_hardware_backlight::wire::State{
          .backlight_on = true,
          .brightness = 1000.0,
      },
      /*use_absolute_brightness=*/true));
}

}  // namespace

}  // namespace display
