// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"

namespace display {

namespace {

constexpr ModeAndId kMode1Vga60Fps({
    .id = ModeId(1),
    .mode = Mode({.active_width = 640, .active_height = 480, .refresh_rate_millihertz = 60'000}),
});

constexpr ModeAndId kMode1Vga60Fps2({
    .id = ModeId(1),
    .mode = Mode({.active_width = 640, .active_height = 480, .refresh_rate_millihertz = 60'000}),
});

constexpr ModeAndId kMode2Qvga30Fps({
    .id = ModeId(2),
    .mode = Mode({.active_width = 240, .active_height = 320, .refresh_rate_millihertz = 30'000}),
});

TEST(ModeAndIdTest, EqualityIsReflexive) {
  EXPECT_EQ(kMode1Vga60Fps, kMode1Vga60Fps);
  EXPECT_EQ(kMode1Vga60Fps2, kMode1Vga60Fps2);
  EXPECT_EQ(kMode2Qvga30Fps, kMode2Qvga30Fps);
}

TEST(ModeAndIdTest, EqualityIsSymmetric) {
  EXPECT_EQ(kMode1Vga60Fps, kMode1Vga60Fps2);
  EXPECT_EQ(kMode1Vga60Fps2, kMode1Vga60Fps);
}

TEST(ModeAndIdTest, EqualityForDifferentIds) {
  static constexpr ModeAndId kMode2Vga60Fps({
      .id = ModeId(2),
      .mode = Mode({.active_width = 640, .active_height = 480, .refresh_rate_millihertz = 60'000}),
  });
  EXPECT_NE(kMode1Vga60Fps, kMode2Vga60Fps);
  EXPECT_NE(kMode2Vga60Fps, kMode1Vga60Fps);
}

TEST(ModeAndIdTest, EqualityForDifferentWidths) {
  static constexpr ModeAndId kMode1SmallSquare60Fps({
      .id = ModeId(1),
      .mode = Mode({.active_width = 480, .active_height = 480, .refresh_rate_millihertz = 60'000}),
  });
  EXPECT_NE(kMode1Vga60Fps, kMode1SmallSquare60Fps);
  EXPECT_NE(kMode1SmallSquare60Fps, kMode1Vga60Fps);
}

TEST(ModeAndIdTest, EqualityForDifferentHeights) {
  static constexpr ModeAndId kMode1LargeSquare60Fps({
      .id = ModeId(1),
      .mode = Mode({.active_width = 640, .active_height = 640, .refresh_rate_millihertz = 60'000}),
  });
  EXPECT_NE(kMode1Vga60Fps, kMode1LargeSquare60Fps);
  EXPECT_NE(kMode1LargeSquare60Fps, kMode1Vga60Fps);
}

TEST(ModeAndIdTest, EqualityForDifferentRefreshRates) {
  static constexpr ModeAndId kMode1Vga30Fps({
      .id = ModeId(1),
      .mode = Mode({.active_width = 640, .active_height = 480, .refresh_rate_millihertz = 30'000}),
  });
  EXPECT_NE(kMode1Vga60Fps, kMode1Vga30Fps);
  EXPECT_NE(kMode1Vga30Fps, kMode1Vga60Fps);
}

TEST(ModeAndIdTest, FromDesignatedInitializer) {
  static constexpr ModeAndId mode_and_id({
      .id = ModeId(42),
      .mode = Mode({.active_width = 640, .active_height = 480, .refresh_rate_millihertz = 60'000}),
  });

  EXPECT_EQ(ModeId(42), mode_and_id.id());
  EXPECT_EQ(640, mode_and_id.mode().active_area().width());
  EXPECT_EQ(480, mode_and_id.mode().active_area().height());
  EXPECT_EQ(60'000, mode_and_id.mode().refresh_rate_millihertz());

  static constexpr Mode kVga60Fps({
      .active_width = 640,
      .active_height = 480,
      .refresh_rate_millihertz = 60'000,
  });
  EXPECT_EQ(kVga60Fps, mode_and_id.mode());
}

}  // namespace

}  // namespace display
