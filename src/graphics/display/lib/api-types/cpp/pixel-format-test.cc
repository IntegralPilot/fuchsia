// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <lib/image-format/image_format.h>

#include <gtest/gtest.h>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

namespace {

constexpr PixelFormat kB8G8R8A8Alias(fuchsia_images2::wire::PixelFormat::kB8G8R8A8);

TEST(PixelFormatTest, EqualityIsReflexive) {
  EXPECT_EQ(PixelFormat::kB8G8R8A8, PixelFormat::kB8G8R8A8);
  EXPECT_EQ(kB8G8R8A8Alias, kB8G8R8A8Alias);
  EXPECT_EQ(PixelFormat::kB8G8R8, PixelFormat::kB8G8R8);
}

TEST(PixelFormatTest, EqualityIsSymmetric) {
  EXPECT_EQ(PixelFormat::kB8G8R8A8, kB8G8R8A8Alias);
  EXPECT_EQ(kB8G8R8A8Alias, PixelFormat::kB8G8R8A8);
}

TEST(PixelFormatTest, EqualityForDifferentValues) {
  EXPECT_NE(PixelFormat::kB8G8R8A8, PixelFormat::kB8G8R8);
  EXPECT_NE(PixelFormat::kB8G8R8, PixelFormat::kB8G8R8A8);
  EXPECT_NE(kB8G8R8A8Alias, PixelFormat::kB8G8R8);
  EXPECT_NE(PixelFormat::kB8G8R8, kB8G8R8A8Alias);
}

TEST(PixelFormatTest, ToFidlPixelFormat) {
  static constexpr fuchsia_images2::wire::PixelFormat fidl_pixel_format =
      PixelFormat::kB8G8R8A8.ToFidl();
  EXPECT_EQ(fuchsia_images2::wire::PixelFormat::kB8G8R8A8, fidl_pixel_format);
}

TEST(PixelFormatTest, ToPixelFormatWithFidlValue) {
  static constexpr PixelFormat pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  EXPECT_EQ(PixelFormat::kB8G8R8A8, pixel_format);
}

TEST(PixelFormatTest, ValueForLogging) {
  EXPECT_EQ(static_cast<uint32_t>(fuchsia_images2::wire::PixelFormat::kB8G8R8A8),
            PixelFormat::kB8G8R8A8.ValueForLogging());
}

TEST(PixelFormatTest, FidlConversionRoundtrip) {
  EXPECT_EQ(PixelFormat::kB8G8R8A8, PixelFormat(PixelFormat::kB8G8R8A8.ToFidl()));
  EXPECT_EQ(PixelFormat::kB8G8R8, PixelFormat(PixelFormat::kB8G8R8.ToFidl()));
}

TEST(PixelFormatTest, IsSupportedRgba) {
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kB8G8R8A8));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kR8G8B8A8));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kB8G8R8A8));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kA2B10G10R10));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kA2R10G10B10));
}

TEST(PixelFormatTest, IsSupportedRgb) {
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kB8G8R8));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kR5G6B5));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kR3G3B2));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kR8G8B8));
}

TEST(PixelFormatTest, IsSupportedImplicitColorChannels) {
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kL8));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kR8));
}

TEST(PixelFormatTest, IsSupportedDeprecatedUnspecifiedAlpha) {
  EXPECT_FALSE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kB8G8R8X8));
  EXPECT_FALSE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kR8G8B8X8));
  EXPECT_FALSE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kR2G2B2X2));
}

TEST(PixelFormatTest, IsSupportedMultiPlanarWithVulkanEquivalents) {
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kI420));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kNv12));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kYuy2));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kYv12));
  EXPECT_TRUE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kP010));
}

TEST(PixelFormatTest, IsSupportedMultiPlanarWithoutVulkanEquivalents) {
  EXPECT_FALSE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kM420));
}

TEST(PixelFormatTest, IsSupportedInvalid) {
  EXPECT_FALSE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kInvalid));
  EXPECT_FALSE(PixelFormat::IsSupported(fuchsia_images2::wire::PixelFormat::kMjpeg));
}

TEST(PixelFormatTest, PlaneCountRgba) {
  EXPECT_EQ(1, PixelFormat::kB8G8R8A8.PlaneCount());
  EXPECT_EQ(1, PixelFormat::kR8G8B8A8.PlaneCount());
  EXPECT_EQ(1, PixelFormat::kB8G8R8A8.PlaneCount());
  EXPECT_EQ(1, PixelFormat::kA2B10G10R10.PlaneCount());
  EXPECT_EQ(1, PixelFormat::kA2R10G10B10.PlaneCount());
}

TEST(PixelFormatTest, PlaneCountRgb) {
  EXPECT_EQ(1, PixelFormat::kB8G8R8.PlaneCount());
  EXPECT_EQ(1, PixelFormat::kR5G6B5.PlaneCount());
  EXPECT_EQ(1, PixelFormat::kR3G3B2.PlaneCount());
  EXPECT_EQ(1, PixelFormat::kR8G8B8.PlaneCount());
}

TEST(PixelFormatTest, PlaneCountImplicitColorChannels) {
  EXPECT_EQ(1, PixelFormat::kL8.PlaneCount());
  EXPECT_EQ(1, PixelFormat::kR8.PlaneCount());
}

TEST(PixelFormatTest, PlaneCountMultiPlanarWithVulkanEquivalents) {
  EXPECT_EQ(3, PixelFormat::kI420.PlaneCount());
  EXPECT_EQ(2, PixelFormat::kNv12.PlaneCount());
  EXPECT_EQ(2, PixelFormat::kYuy2.PlaneCount());
  EXPECT_EQ(3, PixelFormat::kYv12.PlaneCount());
  EXPECT_EQ(2, PixelFormat::kP010.PlaneCount());
}

TEST(PixelFormatTest, EncodingSizeMatchesImageFormat) {
  static constexpr std::array kSupportedSinglePlaneFormats = {
      PixelFormat::kR8G8B8A8, PixelFormat::kB8G8R8A8,    PixelFormat::kB8G8R8,
      PixelFormat::kR5G6B5,   PixelFormat::kR3G3B2,      PixelFormat::kL8,
      PixelFormat::kR8,       PixelFormat::kA2R10G10B10, PixelFormat::kA2B10G10R10,
      PixelFormat::kR8G8B8,
  };

  for (PixelFormat pixel_format : kSupportedSinglePlaneFormats) {
    SCOPED_TRACE(::testing::Message() << "Format code: " << pixel_format.ValueForLogging());
    ASSERT_EQ(1, pixel_format.PlaneCount());
    const PixelFormatAndModifier sysmem_format(pixel_format.ToFidl(),
                                               fuchsia_images2::wire::PixelFormatModifier::kLinear);
    EXPECT_EQ(ImageFormatBitsPerPixel(sysmem_format),
              static_cast<uint32_t>(pixel_format.EncodingSize() * 8));
  }
}

TEST(PixelFormat, ToString) {
  EXPECT_EQ(PixelFormat::kR8G8B8A8.ToString(), "R8G8B8A8");
  EXPECT_EQ(PixelFormat::kB8G8R8A8.ToString(), "B8G8R8A8");
  EXPECT_EQ(PixelFormat::kI420.ToString(), "I420");
  EXPECT_EQ(PixelFormat::kNv12.ToString(), "NV12");
  EXPECT_EQ(PixelFormat::kYuy2.ToString(), "YUY2");
  EXPECT_EQ(PixelFormat::kYv12.ToString(), "YV12");
  EXPECT_EQ(PixelFormat::kB8G8R8.ToString(), "B8G8R8");
  EXPECT_EQ(PixelFormat::kR5G6B5.ToString(), "R5G6B5");
  EXPECT_EQ(PixelFormat::kR3G3B2.ToString(), "R3G3B2");
  EXPECT_EQ(PixelFormat::kL8.ToString(), "L8");
  EXPECT_EQ(PixelFormat::kR8.ToString(), "R8");
  EXPECT_EQ(PixelFormat::kA2R10G10B10.ToString(), "A2R10G10B10");
  EXPECT_EQ(PixelFormat::kA2B10G10R10.ToString(), "A2B10G10R10");
  EXPECT_EQ(PixelFormat::kP010.ToString(), "P010");
  EXPECT_EQ(PixelFormat::kR8G8B8.ToString(), "R8G8B8");
}

#if __cplusplus >= 202002L
TEST(PixelFormat, Format) {
  EXPECT_EQ(std::format("{}", PixelFormat::kR8G8B8A8), "R8G8B8A8");
  EXPECT_EQ(std::format("{}", PixelFormat::kI420), "I420");
  EXPECT_EQ(std::format("{:>8}", PixelFormat::kI420), "    I420");
}
#endif  // __cplusplus >= 202002L

}  // namespace

}  // namespace display
