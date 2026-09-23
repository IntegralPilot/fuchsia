// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/pixel-grid-size2d.h"

#include <gtest/gtest.h>

namespace amlogic_display {

namespace {

TEST(PixelGridSize2D, EqualityIsReflexive) {
  constexpr PixelGridSize2D kSizeA = {
      .width = 800,
      .height = 600,
  };

  EXPECT_EQ(kSizeA, kSizeA);
}

TEST(PixelGridSize2D, EqualityIsSymmetric) {
  constexpr PixelGridSize2D kSizeA = {
      .width = 800,
      .height = 600,
  };

  constexpr PixelGridSize2D kSizeB = {
      .width = 800,
      .height = 600,
  };

  EXPECT_EQ(kSizeA, kSizeB);
  EXPECT_EQ(kSizeB, kSizeA);
}

TEST(PixelGridSize2D, EqualityIsTransitive) {
  constexpr PixelGridSize2D kSizeA = {
      .width = 800,
      .height = 600,
  };

  constexpr PixelGridSize2D kSizeB = {
      .width = 800,
      .height = 600,
  };

  constexpr PixelGridSize2D kSizeC = {
      .width = 800,
      .height = 600,
  };

  EXPECT_EQ(kSizeA, kSizeB);
  EXPECT_EQ(kSizeB, kSizeC);
  EXPECT_EQ(kSizeA, kSizeC);
}

TEST(PixelGridSize2D, NonEquality) {
  constexpr PixelGridSize2D kSize = {
      .width = 800,
      .height = 600,
  };

  constexpr PixelGridSize2D kSizeHeightDifferent = {
      .width = 800,
      .height = 700,
  };

  constexpr PixelGridSize2D kSizeWidthDifferent = {
      .width = 400,
      .height = 600,
  };

  constexpr PixelGridSize2D kSizeWidthHeightDifferent = {
      .width = 400,
      .height = 500,
  };

  EXPECT_NE(kSize, kSizeHeightDifferent);
  EXPECT_NE(kSize, kSizeWidthDifferent);
  EXPECT_NE(kSize, kSizeWidthHeightDifferent);
}

TEST(PixelGridSize2D, ValidSize) {
  constexpr PixelGridSize2D kValidSize = {
      .width = 8191,
      .height = 4095,
  };
  EXPECT_TRUE(kValidSize.IsValid());

  constexpr PixelGridSize2D kValidSize2 = {
      .width = 1280,
      .height = 720,
  };
  EXPECT_TRUE(kValidSize2.IsValid());

  constexpr PixelGridSize2D kUhd4KSize = {
      .width = 3840,
      .height = 2160,
  };
  EXPECT_TRUE(kUhd4KSize.IsValid());

  constexpr PixelGridSize2D kDci4KSize = {
      .width = 4096,
      .height = 2160,
  };
  EXPECT_TRUE(kDci4KSize.IsValid());
}

TEST(PixelGridSize2D, InvalidConstant) { EXPECT_FALSE(kInvalidPixelGridSize2D.IsValid()); }

TEST(PixelGridSize2D, SizeWithZeroInvalid) {
  constexpr PixelGridSize2D kSizeWidthZero = {
      .width = 0,
      .height = 600,
  };
  EXPECT_FALSE(kSizeWidthZero.IsValid());

  constexpr PixelGridSize2D kSizeHeightZero = {
      .width = 800,
      .height = 0,
  };
  EXPECT_FALSE(kSizeHeightZero.IsValid());
}

TEST(PixelGridSize2D, SizeWithNegativeValueIsInvalid) {
  constexpr PixelGridSize2D kSizeWithNegativeWidth = {
      .width = -1,
      .height = 800,
  };
  EXPECT_FALSE(kSizeWithNegativeWidth.IsValid());

  constexpr PixelGridSize2D kSizeWithNegativeHeight = {
      .width = 600,
      .height = -1,
  };
  EXPECT_FALSE(kSizeWithNegativeHeight.IsValid());
}

TEST(PixelGridSize2D, SizeGreaterThanMaximumIsInvalid) {
  constexpr PixelGridSize2D kSizeWithOverlyLargeWidth = {
      .width = 8192,
      .height = 800,
  };
  EXPECT_FALSE(kSizeWithOverlyLargeWidth.IsValid());

  constexpr PixelGridSize2D kSizeWithOverlyLargeHeight = {
      .width = 600,
      .height = 4096,
  };
  EXPECT_FALSE(kSizeWithOverlyLargeHeight.IsValid());
}

}  // namespace

}  // namespace amlogic_display
