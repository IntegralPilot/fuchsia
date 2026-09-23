// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/dimensions.h"

#include <fidl/fuchsia.math/cpp/wire.h>

#include <gtest/gtest.h>

namespace display {

namespace {

constexpr Dimensions kSmall({
    .width = 10,
    .height = 20,
});

constexpr Dimensions kSmall2({
    .width = 10,
    .height = 20,
});

constexpr Dimensions kMedium({
    .width = 10000,
    .height = 20000,
});

TEST(DimensionsTest, EqualityIsReflexive) {
  EXPECT_EQ(kSmall, kSmall);
  EXPECT_EQ(kSmall2, kSmall2);
  EXPECT_EQ(kMedium, kMedium);
}

TEST(DimensionsTest, EqualityIsSymmetric) {
  EXPECT_EQ(kSmall, kSmall2);
  EXPECT_EQ(kSmall2, kSmall);
}

TEST(DimensionsTest, EqualityForDifferentWidths) {
  static constexpr Dimensions kSquare20({
      .width = 20,
      .height = 20,
  });
  EXPECT_NE(kSmall, kSquare20);
  EXPECT_NE(kSquare20, kSmall);
}

TEST(DimensionsTest, EqualityForDifferentHeights) {
  static constexpr Dimensions kSquare10({
      .width = 20,
      .height = 20,
  });
  EXPECT_NE(kSmall, kSquare10);
  EXPECT_NE(kSquare10, kSmall);
}

TEST(DimensionsTest, FromDesignatedInitializer) {
  static constexpr Dimensions dimensions({
      .width = 400,
      .height = 500,
  });
  EXPECT_EQ(400, dimensions.width());
  EXPECT_EQ(500, dimensions.height());
}

TEST(DimensionsTest, FromFidlSizeU) {
  static constexpr fuchsia_math::wire::SizeU fidl_size = {
      .width = 400,
      .height = 500,
  };

  static constexpr Dimensions dimensions = Dimensions::From(fidl_size);
  EXPECT_EQ(400, dimensions.width());
  EXPECT_EQ(500, dimensions.height());
}

TEST(DimensionsTest, ToFidlDimensions) {
  static constexpr Dimensions dimensions({
      .width = 400,
      .height = 500,
  });

  static constexpr fuchsia_math::wire::SizeU fidl_size = dimensions.ToFidl();
  EXPECT_EQ(400u, fidl_size.width);
  EXPECT_EQ(500u, fidl_size.height);
}

TEST(DimensionsTest, IsValidFidlSmall) {
  EXPECT_TRUE(Dimensions::IsValid(fuchsia_math::wire::SizeU({.width = 10, .height = 20})));
}

TEST(DimensionsTest, IsValidFidlLargeWidth) {
  EXPECT_FALSE(Dimensions::IsValid(fuchsia_math::wire::SizeU({.width = 1'000'000, .height = 20})));
}

TEST(DimensionsTest, IsValidFidlLargeHeight) {
  EXPECT_FALSE(Dimensions::IsValid(fuchsia_math::wire::SizeU({.width = 10, .height = 1'000'000})));
}

TEST(DimensionsTest, IsValidFidlCanonicalEmpty) {
  EXPECT_TRUE(Dimensions::IsValid(fuchsia_math::wire::SizeU({.width = 0, .height = 0})));
}

TEST(DimensionsTest, IsValidFidlEmptyHorizontalLine) {
  EXPECT_FALSE(Dimensions::IsValid(fuchsia_math::wire::SizeU({.width = 10, .height = 0})));
}

TEST(DimensionsTest, IsValidFidlEmptyVerticalLine) {
  EXPECT_FALSE(Dimensions::IsValid(fuchsia_math::wire::SizeU({.width = 0, .height = 10})));
}

TEST(DimensionsTest, IsEmpty) {
  static constexpr Dimensions kEmpty({.width = 0, .height = 0});
  EXPECT_TRUE(kEmpty.IsEmpty());

  EXPECT_FALSE(kSmall.IsEmpty());
  EXPECT_FALSE(kMedium.IsEmpty());
}

}  // namespace

}  // namespace display
