// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/display_mode.h"

#include <gtest/gtest.h>

namespace types {

namespace {

constexpr DisplayMode kDisplayMode1({.active_area = Extent2({.width = 1024, .height = 768}),
                                     .refresh_rate_millihertz = 60000,
                                     .mode_flags = 0});
constexpr DisplayMode kDisplayMode2({.active_area = Extent2({.width = 1024, .height = 768}),
                                     .refresh_rate_millihertz = 60000,
                                     .mode_flags = 0});
constexpr DisplayMode kDisplayMode3({.active_area = Extent2({.width = 1920, .height = 1080}),
                                     .refresh_rate_millihertz = 60000,
                                     .mode_flags = 0});

TEST(DisplayModeTest, Equality) {
  // Reflexive property.
  EXPECT_EQ(kDisplayMode1, kDisplayMode1);

  // Symmetric property.
  EXPECT_EQ(kDisplayMode1, kDisplayMode2);
  EXPECT_EQ(kDisplayMode2, kDisplayMode1);

  // Transitive property.
  EXPECT_NE(kDisplayMode1, kDisplayMode3);
  EXPECT_NE(kDisplayMode2, kDisplayMode3);

  // Test all fields for inequality.
  EXPECT_NE(kDisplayMode1, DisplayMode({.active_area = Extent2({.width = 1, .height = 768}),
                                        .refresh_rate_millihertz = 60000,
                                        .mode_flags = 0}));
  EXPECT_NE(kDisplayMode1, DisplayMode({.active_area = Extent2({.width = 1024, .height = 1}),
                                        .refresh_rate_millihertz = 60000,
                                        .mode_flags = 0}));
  EXPECT_NE(kDisplayMode1, DisplayMode({.active_area = Extent2({.width = 1024, .height = 768}),
                                        .refresh_rate_millihertz = 1,
                                        .mode_flags = 0}));
}

TEST(DisplayModeTest, IsValid) {
  // Basic valid case.
  EXPECT_TRUE(DisplayMode::IsValid({.active_area = Extent2({.width = 1, .height = 1}),
                                    .refresh_rate_millihertz = 1,
                                    .mode_flags = 0}));

  // Invalid active_area.
  EXPECT_FALSE(DisplayMode::IsValid({.active_area = Extent2({.width = 0, .height = 1}),
                                     .refresh_rate_millihertz = 1,
                                     .mode_flags = 0}));
  EXPECT_FALSE(DisplayMode::IsValid({.active_area = Extent2({.width = 1, .height = 0}),
                                     .refresh_rate_millihertz = 1,
                                     .mode_flags = 0}));

  // Invalid refresh_rate_millihertz.
  EXPECT_FALSE(DisplayMode::IsValid({.active_area = Extent2({.width = 1, .height = 1}),
                                     .refresh_rate_millihertz = 0,
                                     .mode_flags = 0}));

  // Invalid mode_flags.
  EXPECT_FALSE(DisplayMode::IsValid({.active_area = Extent2({.width = 1, .height = 1}),
                                     .refresh_rate_millihertz = 1,
                                     .mode_flags = 1}));
}

TEST(DisplayModeTest, FromFidl) {
  // Basic conversion from a fuchsia_hardware_display_types::wire::Mode.
  const fuchsia_hardware_display_types::wire::Mode fidl_mode = {
      .active_area = {.width = 1024, .height = 768},
      .refresh_rate_millihertz = 60000,
      .flags = fuchsia_hardware_display_types::wire::ModeFlags()};
  EXPECT_EQ(DisplayMode::From(fidl_mode), kDisplayMode1);
}

TEST(DisplayModeTest, ToWire) {
  // Basic conversion to a fuchsia_hardware_display_types::wire::Mode.
  const fuchsia_hardware_display_types::wire::Mode wire_mode = kDisplayMode1.ToWire();
  EXPECT_EQ(wire_mode.active_area.width, 1024u);
  EXPECT_EQ(wire_mode.active_area.height, 768u);
  EXPECT_EQ(wire_mode.refresh_rate_millihertz, 60000u);
  EXPECT_EQ(wire_mode.flags, fuchsia_hardware_display_types::wire::ModeFlags());
}

TEST(DisplayModeTest, Accessors) {
  // Test all the data accessors.
  EXPECT_EQ(kDisplayMode1.active_area().width(), 1024);
  EXPECT_EQ(kDisplayMode1.active_area().height(), 768);
  EXPECT_EQ(kDisplayMode1.refresh_rate_millihertz(), 60000u);
  EXPECT_EQ(kDisplayMode1.mode_flags(), 0u);
}

TEST(DisplayModeTest, Hash) {
  const std::hash<DisplayMode> hasher;
  EXPECT_EQ(hasher(kDisplayMode1), hasher(kDisplayMode2));
  EXPECT_NE(hasher(kDisplayMode1), hasher(kDisplayMode3));

  // Changing each field should result in a different hash.
  EXPECT_NE(hasher(kDisplayMode1),
            hasher(DisplayMode({.active_area = Extent2({.width = 1, .height = 768}),
                                .refresh_rate_millihertz = 60000,
                                .mode_flags = 0})));
  EXPECT_NE(hasher(kDisplayMode1),
            hasher(DisplayMode({.active_area = Extent2({.width = 1024, .height = 1}),
                                .refresh_rate_millihertz = 60000,
                                .mode_flags = 0})));
  EXPECT_NE(hasher(kDisplayMode1),
            hasher(DisplayMode({.active_area = Extent2({.width = 1024, .height = 768}),
                                .refresh_rate_millihertz = 1,
                                .mode_flags = 0})));
}

}  // namespace
}  // namespace types
