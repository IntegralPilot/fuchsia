// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"

namespace display {

namespace {

constexpr ImageBufferUsage kDisplayUsage({
    .tiling_type = ImageTilingType::kLinear,
});
constexpr ImageBufferUsage kDisplayUsage2({
    .tiling_type = ImageTilingType::kLinear,
});
constexpr ImageBufferUsage kCaptureUsage({
    .tiling_type = ImageTilingType::kCapture,
});

TEST(ImageBufferUsageTest, EqualityIsReflexive) {
  EXPECT_EQ(kDisplayUsage, kDisplayUsage);
  EXPECT_EQ(kDisplayUsage2, kDisplayUsage2);
  EXPECT_EQ(kCaptureUsage, kCaptureUsage);
}

TEST(ImageBufferUsageTest, EqualityIsSymmetric) {
  EXPECT_EQ(kDisplayUsage, kDisplayUsage2);
  EXPECT_EQ(kDisplayUsage2, kDisplayUsage);
}

TEST(ImageBufferUsageTest, EqualityForDifferentTilingTypes) {
  EXPECT_NE(kDisplayUsage, kCaptureUsage);
  EXPECT_NE(kCaptureUsage, kDisplayUsage);
}

TEST(ImageBufferUsageTest, FromFidlImageBufferUsage) {
  static constexpr fuchsia_hardware_display_types::wire::ImageBufferUsage fidl_image_buffer_usage =
      {
          .tiling_type = fuchsia_hardware_display_types::wire::kImageTilingTypeCapture,
      };

  static constexpr ImageBufferUsage image_buffer_usage(fidl_image_buffer_usage);
  EXPECT_EQ(ImageTilingType::kCapture, image_buffer_usage.tiling_type());
}

TEST(ImageBufferUsageTest, ToFidlImageBufferUsage) {
  static constexpr fuchsia_hardware_display_types::wire::ImageBufferUsage fidl_image_buffer_usage =
      kCaptureUsage.ToFidl();
  EXPECT_EQ(fuchsia_hardware_display_types::wire::kImageTilingTypeCapture,
            fidl_image_buffer_usage.tiling_type);
}

TEST(ImageBufferUsageTest, FidlDisplayIdConversionRoundtrip) {
  EXPECT_EQ(kDisplayUsage, ImageBufferUsage(kDisplayUsage.ToFidl()));
  EXPECT_EQ(kCaptureUsage, ImageBufferUsage(kCaptureUsage.ToFidl()));
}

}  // namespace

}  // namespace display
