// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/color.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>

#include <cstdint>
#include <initializer_list>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace display {

namespace {

constexpr Color kRgbaGreyish({
    .format = PixelFormat::kR8G8B8A8,
    .bytes = std::initializer_list<uint8_t>{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0},
});

constexpr Color kRgbaGreyish2({
    .format = PixelFormat::kR8G8B8A8,
    .bytes = std::initializer_list<uint8_t>{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0},
});

constexpr Color kBgraGreyish2({
    .format = PixelFormat::kB8G8R8A8,
    .bytes = std::initializer_list<uint8_t>{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0},
});

TEST(ColorTest, EqualityIsReflexive) {
  EXPECT_EQ(kRgbaGreyish, kRgbaGreyish);
  EXPECT_EQ(kRgbaGreyish2, kRgbaGreyish2);
  EXPECT_EQ(kBgraGreyish2, kBgraGreyish2);
}

TEST(ColorTest, EqualityIsSymmetric) {
  EXPECT_EQ(kRgbaGreyish, kRgbaGreyish2);
  EXPECT_EQ(kRgbaGreyish2, kRgbaGreyish);
}

TEST(ColorTest, EqualityForDifferentContents) {
  static constexpr Color kRgbaFuchsia({
      .format = PixelFormat::kR8G8B8A8,
      .bytes = std::initializer_list<uint8_t>{0xff, 0x00, 0xff, 0xff, 0, 0, 0, 0},
  });

  EXPECT_NE(kRgbaGreyish, kRgbaFuchsia);
  EXPECT_NE(kRgbaFuchsia, kRgbaGreyish);
}

TEST(ColorTest, EqualityForDifferentFormats) {
  static constexpr Color kBgraGreyish({
      .format = PixelFormat::kB8G8R8A8,
      .bytes = std::initializer_list<uint8_t>{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0},
  });
  EXPECT_NE(kRgbaGreyish, kBgraGreyish);
  EXPECT_NE(kBgraGreyish, kRgbaGreyish);
}

TEST(ColorTest, FromDesignatedInitializer) {
  static constexpr Color color({
      .format = PixelFormat::kR8G8B8A8,
      .bytes = std::initializer_list<uint8_t>{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0},
  });
  EXPECT_EQ(PixelFormat::kR8G8B8A8, color.format());
  EXPECT_THAT(color.bytes(), testing::ElementsAreArray(std::initializer_list<uint8_t>{
                                 0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}));
}

TEST(ColorTest, FromFidlColor) {
  static constexpr fuchsia_hardware_display_types::wire::Color fidl_color = {
      .format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
      .bytes = {0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0},
  };

  static constexpr Color color = Color::From(fidl_color);
  EXPECT_EQ(PixelFormat::kR8G8B8A8, color.format());
  EXPECT_THAT(color.bytes(), testing::ElementsAreArray(std::initializer_list<uint8_t>{
                                 0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}));
}

TEST(ColorTest, ToFidlColor) {
  static constexpr Color color({
      .format = PixelFormat::kR8G8B8A8,
      .bytes = std::initializer_list<uint8_t>{0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0},
  });

  static constexpr fuchsia_hardware_display_types::wire::Color fidl_color = color.ToFidl();
  EXPECT_EQ(fuchsia_images2::wire::PixelFormat::kR8G8B8A8, fidl_color.format);
  EXPECT_THAT(fidl_color.bytes, testing::ElementsAreArray(std::initializer_list<uint8_t>{
                                    0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0}));
}

TEST(ColorTest, IsValidFidlRgbaGreyish) {
  EXPECT_TRUE(Color::IsValid(fuchsia_hardware_display_types::wire::Color{
      .format = fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
      .bytes = {0x41, 0x42, 0x43, 0x44, 0, 0, 0, 0},
  }));
}

TEST(ColorTest, SupportsFormatRgbaFormats) {
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kB8G8R8A8));
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kR8G8B8A8));
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kB8G8R8A8));
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kA2B10G10R10));
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kA2R10G10B10));
}

TEST(ColorTest, SupportsFormatRgbFormats) {
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kB8G8R8));
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kR5G6B5));
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kR3G3B2));
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kR8G8B8));
}

TEST(ColorTest, SupportsFormatImplicitColorChannels) {
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kL8));
  EXPECT_TRUE(Color::SupportsFormat(PixelFormat::kR8));
}

TEST(ColorTest, SupportsFormatMultiPlanar) {
  EXPECT_FALSE(Color::SupportsFormat(PixelFormat::kI420));
  EXPECT_FALSE(Color::SupportsFormat(PixelFormat::kNv12));
  EXPECT_FALSE(Color::SupportsFormat(PixelFormat::kYuy2));
  EXPECT_FALSE(Color::SupportsFormat(PixelFormat::kYv12));
  EXPECT_FALSE(Color::SupportsFormat(PixelFormat::kP010));
}

}  // namespace

}  // namespace display
