// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>

#include <array>
#include <limits>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace display {

namespace {

const ColorConversion kIdentity({
    .preoffsets = {0.0f, 0.0f, 0.0f},
    .coefficients =
        {
            std::to_array({1.0f, 0.0f, 0.0f}),
            std::to_array({0.0f, 1.0f, 0.0f}),
            std::to_array({0.0f, 0.0f, 1.0f}),
        },
    .postoffsets = {0.0f, 0.0f, 0.0f},
});

const ColorConversion kIdentity2({
    .preoffsets = {0.0f, 0.0f, 0.0f},
    .coefficients =
        {
            std::to_array({1.0f, 0.0f, 0.0f}),
            std::to_array({0.0f, 1.0f, 0.0f}),
            std::to_array({0.0f, 0.0f, 1.0f}),
        },
    .postoffsets = {0.0f, 0.0f, 0.0f},
});

const ColorConversion kAnotherConfig({
    .preoffsets = {0.1f, 0.2f, 0.3f},
    .coefficients =
        {
            std::to_array({1.0f, 2.0f, 3.0f}),
            std::to_array({4.0f, 5.0f, 6.0f}),
            std::to_array({7.0f, 8.0f, 9.0f}),
        },
    .postoffsets = {0.4f, 0.5f, 0.6f},
});

TEST(ColorConversionTest, EqualityIsReflexive) {
  EXPECT_EQ(kIdentity, kIdentity);
  EXPECT_EQ(kIdentity2, kIdentity2);
  EXPECT_EQ(kAnotherConfig, kAnotherConfig);
}

TEST(ColorConversionTest, EqualityIsSymmetric) {
  EXPECT_EQ(kIdentity, kIdentity2);
  EXPECT_EQ(kIdentity2, kIdentity);
}

TEST(ColorConversionTest, EqualityForDifferentValues) {
  EXPECT_NE(kIdentity, kAnotherConfig);
  EXPECT_NE(kAnotherConfig, kIdentity);
}

TEST(ColorConversionTest, FromDesignatedInitializer) {
  const ColorConversion kConfig({
      .preoffsets = {0.1f, 0.2f, 0.3f},
      .coefficients =
          {
              std::to_array({1.0f, 2.0f, 3.0f}),
              std::to_array({4.0f, 5.0f, 6.0f}),
              std::to_array({7.0f, 8.0f, 9.0f}),
          },
      .postoffsets = {0.4f, 0.5f, 0.6f},
  });

  EXPECT_THAT(kConfig.preoffsets(), testing::ElementsAre(0.1f, 0.2f, 0.3f));
  EXPECT_THAT(kConfig.coefficients()[0], testing::ElementsAreArray(std::array{1.0f, 2.0f, 3.0f}));
  EXPECT_THAT(kConfig.coefficients()[1], testing::ElementsAreArray(std::array{4.0f, 5.0f, 6.0f}));
  EXPECT_THAT(kConfig.coefficients()[2], testing::ElementsAreArray(std::array{7.0f, 8.0f, 9.0f}));
  EXPECT_THAT(kConfig.postoffsets(), testing::ElementsAreArray(std::array{0.4f, 0.5f, 0.6f}));
}

TEST(ColorConversionTest, FromFidl) {
  static constexpr fuchsia_hardware_display_engine::wire::ColorConversion fidl_config = {
      .preoffsets = {0.1f, 0.2f, 0.3f},
      .coefficients =
          {
              fidl::Array<float, 3>{1.0f, 2.0f, 3.0f},
              fidl::Array<float, 3>{4.0f, 5.0f, 6.0f},
              fidl::Array<float, 3>{7.0f, 8.0f, 9.0f},
          },
      .postoffsets = {0.4f, 0.5f, 0.6f},
  };
  const ColorConversion config(fidl_config);
  EXPECT_EQ(kAnotherConfig, config);
}

TEST(ColorConversionTest, ToFidl) {
  const fuchsia_hardware_display_engine::wire::ColorConversion fidl_config =
      kAnotherConfig.ToFidl();

  EXPECT_THAT(fidl_config.preoffsets, testing::ElementsAreArray(std::array{0.1f, 0.2f, 0.3f}));
  EXPECT_THAT(fidl_config.coefficients[0], testing::ElementsAreArray(std::array{1.0f, 2.0f, 3.0f}));
  EXPECT_THAT(fidl_config.coefficients[1], testing::ElementsAreArray(std::array{4.0f, 5.0f, 6.0f}));
  EXPECT_THAT(fidl_config.coefficients[2], testing::ElementsAreArray(std::array{7.0f, 8.0f, 9.0f}));
  EXPECT_THAT(fidl_config.postoffsets, testing::ElementsAreArray(std::array{0.4f, 0.5f, 0.6f}));
}

TEST(ColorConversionTest, FidlRoundTrip) {
  const ColorConversion config(kAnotherConfig.ToFidl());
  EXPECT_EQ(kAnotherConfig, config);
}

TEST(ColorConversionTest, IsValidFidlForValidFidl) {
  EXPECT_TRUE(ColorConversion::IsValid(kIdentity.ToFidl()));
  EXPECT_TRUE(ColorConversion::IsValid(kAnotherConfig.ToFidl()));
}

TEST(ColorConversionTest, IsValidFidlForFidlWithInfinity) {
  fuchsia_hardware_display_engine::wire::ColorConversion non_finite_config = kIdentity.ToFidl();
  non_finite_config.preoffsets[0] = std::numeric_limits<float>::infinity();
  EXPECT_FALSE(ColorConversion::IsValid(non_finite_config));
}

TEST(ColorConversionTest, IsValidFidlForFidlWithNaN) {
  fuchsia_hardware_display_engine::wire::ColorConversion non_finite_config = kIdentity.ToFidl();
  non_finite_config.coefficients[1][1] = std::numeric_limits<float>::quiet_NaN();
  EXPECT_FALSE(ColorConversion::IsValid(non_finite_config));
}

TEST(ColorConversionTest, IsValidFidlForFidlWithNegativeInfinity) {
  fuchsia_hardware_display_engine::wire::ColorConversion non_finite_config = kIdentity.ToFidl();
  non_finite_config.postoffsets[2] = -std::numeric_limits<float>::infinity();
  EXPECT_FALSE(ColorConversion::IsValid(non_finite_config));
}

}  // namespace

}  // namespace display
