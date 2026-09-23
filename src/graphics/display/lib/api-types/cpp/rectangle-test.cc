// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/rectangle.h"

#include <fidl/fuchsia.math/cpp/wire.h>

#include <gtest/gtest.h>

namespace display {

namespace {

constexpr Rectangle kLayerDestination({
    .x = 100,
    .y = 200,
    .width = 400,
    .height = 500,
});

constexpr Rectangle kLayerDestination2({
    .x = 100,
    .y = 200,
    .width = 400,
    .height = 500,
});

constexpr Rectangle kDisplayArea({
    .x = 0,
    .y = 0,
    .width = 800,
    .height = 600,
});

TEST(RectangleTest, EqualityIsReflexive) {
  EXPECT_EQ(kLayerDestination, kLayerDestination);
  EXPECT_EQ(kLayerDestination2, kLayerDestination2);
  EXPECT_EQ(kDisplayArea, kDisplayArea);
}

TEST(RectangleTest, EqualityIsSymmetric) {
  EXPECT_EQ(kLayerDestination, kLayerDestination2);
  EXPECT_EQ(kLayerDestination2, kLayerDestination);
}

TEST(RectangleTest, EqualityForDifferentWidths) {
  static constexpr Rectangle kSmallSquareDisplayArea({
      .x = 0,
      .y = 0,
      .width = 600,
      .height = 600,
  });
  EXPECT_NE(kDisplayArea, kSmallSquareDisplayArea);
  EXPECT_NE(kSmallSquareDisplayArea, kDisplayArea);
}

TEST(RectangleTest, EqualityForDifferentHeights) {
  static constexpr Rectangle kLargeSquareDisplayArea({
      .x = 0,
      .y = 0,
      .width = 800,
      .height = 800,
  });
  EXPECT_NE(kDisplayArea, kLargeSquareDisplayArea);
  EXPECT_NE(kLargeSquareDisplayArea, kDisplayArea);
}

TEST(RectangleTest, EqualityForDifferentXOffsets) {
  static constexpr Rectangle kXOffsetDisplayArea({
      .x = 100,
      .y = 0,
      .width = 800,
      .height = 600,
  });
  EXPECT_NE(kDisplayArea, kXOffsetDisplayArea);
  EXPECT_NE(kXOffsetDisplayArea, kDisplayArea);
}

TEST(RectangleTest, EqualityForDifferentYOffsets) {
  static constexpr Rectangle kYOffsetDisplayArea({
      .x = 0,
      .y = 100,
      .width = 800,
      .height = 600,
  });
  EXPECT_NE(kDisplayArea, kYOffsetDisplayArea);
  EXPECT_NE(kYOffsetDisplayArea, kDisplayArea);
}

TEST(RectangleTest, FromDesignatedInitializer) {
  static constexpr Rectangle rectangle({
      .x = 100,
      .y = 200,
      .width = 400,
      .height = 500,
  });
  EXPECT_EQ(100, rectangle.x());
  EXPECT_EQ(200, rectangle.y());
  EXPECT_EQ(Dimensions({.width = 400, .height = 500}), rectangle.dimensions());
  EXPECT_EQ(400, rectangle.width());
  EXPECT_EQ(500, rectangle.height());
}

TEST(RectangleTest, FromFidlRectU) {
  static constexpr fuchsia_math::wire::RectU fidl_rectangle = {
      .x = 100,
      .y = 200,
      .width = 400,
      .height = 500,
  };

  static constexpr Rectangle rectangle = Rectangle::From(fidl_rectangle);
  EXPECT_EQ(100, rectangle.x());
  EXPECT_EQ(200, rectangle.y());
  EXPECT_EQ(Dimensions({.width = 400, .height = 500}), rectangle.dimensions());
  EXPECT_EQ(400, rectangle.width());
  EXPECT_EQ(500, rectangle.height());
}

TEST(RectangleTest, ToFidlRectangle) {
  static constexpr Rectangle rectangle({
      .x = 100,
      .y = 200,
      .width = 400,
      .height = 500,
  });

  static constexpr fuchsia_math::wire::RectU fidl_rectangle = rectangle.ToFidl();
  EXPECT_EQ(100u, fidl_rectangle.x);
  EXPECT_EQ(200u, fidl_rectangle.y);
  EXPECT_EQ(400u, fidl_rectangle.width);
  EXPECT_EQ(500u, fidl_rectangle.height);
}

TEST(RectangleTest, IsValidFidlLayerDestination) {
  EXPECT_TRUE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = 100,
      .y = 200,
      .width = 400,
      .height = 500,
  }));
}

TEST(RectangleTest, IsValidFidlLargeXOffset) {
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = 1'000'000,
      .y = 200,
      .width = 400,
      .height = 500,
  }));
}

TEST(RectangleTest, IsValidFidlLargeYOffset) {
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = 100,
      .y = 1'000'000,
      .width = 400,
      .height = 500,
  }));
}

TEST(RectangleTest, IsValidFidlLargeWidth) {
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = 100,
      .y = 200,
      .width = 1'000'000,
      .height = 500,
  }));
}

TEST(RectangleTest, IsValidFidlLargeHeight) {
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = 100,
      .y = 200,
      .width = 400,
      .height = 1'000'000,
  }));
}

}  // namespace

}  // namespace display
