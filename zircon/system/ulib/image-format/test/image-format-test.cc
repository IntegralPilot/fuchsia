// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/image-format/image_format.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zbi-format/graphics.h>

#include <fbl/array.h>
#include <zxtest/zxtest.h>

namespace sysmem_v1 = fuchsia_sysmem;
namespace sysmem_v2 = fuchsia_sysmem2;

TEST(ImageFormat, LinearComparison_V2) {
  PixelFormatAndModifier plain;
  plain.pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;

  PixelFormatAndModifier linear;
  linear.pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;
  linear.pixel_format_modifier = fuchsia_images2::PixelFormatModifier::kLinear;

  PixelFormatAndModifier x_tiled;
  x_tiled.pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;
  x_tiled.pixel_format_modifier = fuchsia_images2::PixelFormatModifier::kIntelI915XTiled;

  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(plain, plain));
  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(linear, linear));

  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(plain, linear));
  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(linear, plain));

  EXPECT_FALSE(ImageFormatIsPixelFormatEqual(linear, x_tiled));
  EXPECT_FALSE(ImageFormatIsPixelFormatEqual(plain, x_tiled));
}

// This is fairly redundant with LinearComparison_V2 given that wire enums are the same underlying
// type as natural enums, but we keep this test in case that changes.
TEST(ImageFormat, LinearComparison_V2_wire) {
  fidl::Arena allocator;
  PixelFormatAndModifier plain;
  plain.pixel_format = fuchsia_images2::wire::PixelFormat::kB8G8R8A8;

  PixelFormatAndModifier linear;
  linear.pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;
  linear.pixel_format_modifier = fuchsia_images2::wire::PixelFormatModifier::kLinear;

  PixelFormatAndModifier x_tiled;
  x_tiled.pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;
  x_tiled.pixel_format_modifier = fuchsia_images2::wire::PixelFormatModifier::kIntelI915XTiled;

  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(plain, plain));
  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(linear, linear));

  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(plain, linear));
  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(linear, plain));

  EXPECT_FALSE(ImageFormatIsPixelFormatEqual(linear, x_tiled));
  EXPECT_FALSE(ImageFormatIsPixelFormatEqual(plain, x_tiled));
}

TEST(ImageFormat, LinearComparison_V1_wire) {
  sysmem_v1::wire::PixelFormat plain = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = false,
  };

  sysmem_v1::wire::PixelFormat linear = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierLinear,
          },
  };

  sysmem_v1::wire::PixelFormat x_tiled = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierIntelI915XTiled,
          },
  };

  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(plain, plain));
  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(linear, linear));

  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(plain, linear));
  EXPECT_TRUE(ImageFormatIsPixelFormatEqual(linear, plain));

  EXPECT_FALSE(ImageFormatIsPixelFormatEqual(linear, x_tiled));
  EXPECT_FALSE(ImageFormatIsPixelFormatEqual(plain, x_tiled));
}

TEST(ImageFormat, LinearRowBytes_V2) {
  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  constraints.min_size() = {12u, 1u};
  constraints.max_size() = {100u, 0xFFFFFFFF};
  constraints.bytes_per_row_divisor().emplace(4u * 8u);
  constraints.max_bytes_per_row().emplace(100000u);

  uint32_t row_bytes;
  EXPECT_TRUE(ImageFormatMinimumRowBytes(constraints, 17, &row_bytes));
  EXPECT_EQ(row_bytes, 4 * 24);

  // too narrow in pixels
  EXPECT_FALSE(ImageFormatMinimumRowBytes(constraints, 11, &row_bytes));
  // too wide in pixels
  EXPECT_FALSE(ImageFormatMinimumRowBytes(constraints, 101, &row_bytes));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, LinearRowBytesChecked_V2) {
  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  constraints.min_size() = {12u, 1u};
  constraints.max_size() = {100u, 0xFFFFFFFF};
  constraints.bytes_per_row_divisor().emplace(4u * 8u);
  constraints.max_bytes_per_row().emplace(100000u);

  auto row_bytes_checked = ImageFormatMinimumRowBytesChecked(constraints, 17);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(row_bytes_checked.ValueOrDie(), 4 * 24);

  // too narrow in pixels
  EXPECT_FALSE(ImageFormatMinimumRowBytesChecked(constraints, 11).IsValid());
  // too wide in pixels
  EXPECT_FALSE(ImageFormatMinimumRowBytesChecked(constraints, 101).IsValid());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, LinearRowBytes_V2_wire) {
  fidl::Arena allocator;
  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  constraints.set_pixel_format_modifier(allocator,
                                        fuchsia_images2::wire::PixelFormatModifier::kLinear);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 1u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 0xFFFFFFFF});
  constraints.set_bytes_per_row_divisor(4u * 8u);
  constraints.set_max_bytes_per_row(100000u);

  uint32_t row_bytes;
  EXPECT_TRUE(ImageFormatMinimumRowBytes(constraints, 17, &row_bytes));
  EXPECT_EQ(row_bytes, 4 * 24);

  // too narrow in pixels
  EXPECT_FALSE(ImageFormatMinimumRowBytes(constraints, 11, &row_bytes));
  // too wide in pixels
  EXPECT_FALSE(ImageFormatMinimumRowBytes(constraints, 101, &row_bytes));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, LinearRowBytesChecked_V2_wire) {
  fidl::Arena allocator;
  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  constraints.set_pixel_format_modifier(allocator,
                                        fuchsia_images2::wire::PixelFormatModifier::kLinear);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 1u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 0xFFFFFFFF});
  constraints.set_bytes_per_row_divisor(4u * 8u);
  constraints.set_max_bytes_per_row(100000u);

  auto row_bytes_checked = ImageFormatMinimumRowBytesChecked(constraints, 17);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(row_bytes_checked.ValueOrDie(), 4 * 24);

  // too narrow in pixels
  EXPECT_FALSE(ImageFormatMinimumRowBytesChecked(constraints, 11).IsValid());
  // too wide in pixels
  EXPECT_FALSE(ImageFormatMinimumRowBytesChecked(constraints, 101).IsValid());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, LinearRowBytes_V1_wire) {
  sysmem_v1::wire::PixelFormat linear = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierLinear,
          },
  };
  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = linear,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 4 * 8,
  };

  uint32_t row_bytes;
  EXPECT_TRUE(ImageFormatMinimumRowBytes(constraints, 17, &row_bytes));
  EXPECT_EQ(row_bytes, 4 * 24);

  EXPECT_FALSE(ImageFormatMinimumRowBytes(constraints, 11, &row_bytes));
  EXPECT_FALSE(ImageFormatMinimumRowBytes(constraints, 101, &row_bytes));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, LinearRowBytesChecked_V1_wire) {
  sysmem_v1::wire::PixelFormat linear = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierLinear,
          },
  };
  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = linear,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 4 * 8,
  };

  auto row_bytes_checked = ImageFormatMinimumRowBytesChecked(constraints, 17);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(row_bytes_checked.ValueOrDie(), 4 * 24);

  EXPECT_FALSE(ImageFormatMinimumRowBytesChecked(constraints, 11).IsValid());
  EXPECT_FALSE(ImageFormatMinimumRowBytesChecked(constraints, 101).IsValid());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, InvalidColorSpace_V1_wire) {
  fuchsia_sysmem::wire::PixelFormat kPixelFormat = {
      .type = fuchsia_sysmem::PixelFormatType::kRgb565,
      .has_format_modifier = true,
      .format_modifier = {
          .value = fuchsia_sysmem::wire::kFormatModifierLinear,
      }};

  sysmem_v1::wire::ColorSpace color_space{sysmem_v1::wire::ColorSpaceType::kInvalid};
  // Shouldn't crash.
  EXPECT_FALSE(ImageFormatIsSupportedColorSpaceForPixelFormat(color_space, kPixelFormat));
}

TEST(ImageFormat, PassThroughColorSpace_V1_wire) {
  fidl::Arena allocator;
  fuchsia_sysmem::wire::PixelFormat linear_bgra = {
      .type = fuchsia_sysmem::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = fuchsia_sysmem::kFormatModifierLinear,
          },
  };

  sysmem_v1::wire::ColorSpace color_space{sysmem_v1::wire::ColorSpaceType::kPassThrough};
  EXPECT_TRUE(ImageFormatIsSupportedColorSpaceForPixelFormat(color_space, linear_bgra));

  fuchsia_sysmem::wire::PixelFormat linear_nv12 = {
      .type = fuchsia_sysmem::wire::PixelFormatType::kNv12,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = fuchsia_sysmem::kFormatModifierLinear,
          },
  };

  EXPECT_TRUE(ImageFormatIsSupportedColorSpaceForPixelFormat(color_space, linear_nv12));
}

TEST(ImageFormat, L8AndY8PermitRgbAndYuv) {
  fuchsia_images2::ColorSpace color_space_rgb = fuchsia_images2::ColorSpace::kSrgb;
  fuchsia_images2::ColorSpace color_space_yuv = fuchsia_images2::ColorSpace::kRec709;

  auto pixel_format_L8 = PixelFormatAndModifier(fuchsia_images2::PixelFormat::kL8,
                                                fuchsia_images2::PixelFormatModifier::kLinear);
  EXPECT_TRUE(ImageFormatIsSupportedColorSpaceForPixelFormat(color_space_rgb, pixel_format_L8));
  EXPECT_TRUE(ImageFormatIsSupportedColorSpaceForPixelFormat(color_space_yuv, pixel_format_L8));

  auto pixel_format_R8 = PixelFormatAndModifier(fuchsia_images2::PixelFormat::kR8,
                                                fuchsia_images2::PixelFormatModifier::kLinear);
  EXPECT_TRUE(ImageFormatIsSupportedColorSpaceForPixelFormat(color_space_rgb, pixel_format_R8));
  EXPECT_TRUE(ImageFormatIsSupportedColorSpaceForPixelFormat(color_space_yuv, pixel_format_R8));
}

TEST(ImageFormat, ZbiPixelFormatV2) {
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_RGB_565);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kR5G6B5);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_RGB_332);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kR3G3B2);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_RGB_2220);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kR2G2B2X2);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_ARGB_8888);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_RGB_X888);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_MONO_8);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kL8);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_I420);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kI420);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_NV12);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kNv12);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_RGB_888);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kB8G8R8);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_ABGR_8888);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kR8G8B8A8);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_BGR_888_X);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kR8G8B8A8);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_ARGB_2_10_10_10);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kA2R10G10B10);
  }
  {
    fpromise::result<fuchsia_images2::wire::PixelFormat> convert_result =
        ImageFormatConvertZbiToSysmemPixelFormat_v2(ZBI_PIXEL_FORMAT_ABGR_2_10_10_10);
    ASSERT_TRUE(convert_result.is_ok());
    EXPECT_EQ(convert_result.value(), fuchsia_images2::wire::PixelFormat::kA2B10G10R10);
  }
}

TEST(ImageFormat, PlaneByteOffset_V2) {
  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  constraints.min_size() = {12u, 12u};
  constraints.max_size() = {100u, 100u};
  constraints.bytes_per_row_divisor().emplace(4u * 8u);
  constraints.max_bytes_per_row().emplace(100000u);

  auto image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row());

  uint64_t byte_offset;
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 0, &byte_offset));
  EXPECT_EQ(0u, byte_offset);
  EXPECT_FALSE(ImageFormatPlaneByteOffset(image_format, 1, &byte_offset));

  // clone via generated code
  auto constraints2 = constraints;
  constraints2.pixel_format() = fuchsia_images2::PixelFormat::kI420;

  constexpr uint32_t kBytesPerRow = 32;
  image_format_result = ImageConstraintsToFormat(constraints2, 18, 20);
  EXPECT_TRUE(image_format_result.is_ok());
  image_format = image_format_result.take_value();
  EXPECT_EQ(kBytesPerRow, image_format.bytes_per_row());
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 0, &byte_offset));
  EXPECT_EQ(0u, byte_offset);
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 1, &byte_offset));
  EXPECT_EQ(kBytesPerRow * 20, byte_offset);
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 2, &byte_offset));
  EXPECT_EQ(kBytesPerRow * 20 + kBytesPerRow / 2 * 20 / 2, byte_offset);
  EXPECT_FALSE(ImageFormatPlaneByteOffset(image_format, 3, &byte_offset));

  uint32_t row_bytes;
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 0, &row_bytes));
  EXPECT_EQ(kBytesPerRow, row_bytes);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 1, &row_bytes));
  EXPECT_EQ(kBytesPerRow / 2, row_bytes);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 2, &row_bytes));
  EXPECT_EQ(kBytesPerRow / 2, row_bytes);
  EXPECT_FALSE(ImageFormatPlaneRowBytes(image_format, 3, &row_bytes));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, PlaneByteOffsetChecked_V2) {
  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  constraints.min_size() = {12u, 12u};
  constraints.max_size() = {100u, 100u};
  constraints.bytes_per_row_divisor().emplace(4u * 8u);
  constraints.max_bytes_per_row().emplace(100000u);

  auto image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row());

  auto byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 0);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(0u, byte_offset_checked.ValueOrDie());
  EXPECT_FALSE(ImageFormatPlaneByteOffsetChecked(image_format, 1).IsValid());

  // clone via generated code
  auto constraints2 = constraints;
  constraints2.pixel_format() = fuchsia_images2::PixelFormat::kI420;

  constexpr uint32_t kBytesPerRow = 32;
  image_format_result = ImageConstraintsToFormat(constraints2, 18, 20);
  EXPECT_TRUE(image_format_result.is_ok());
  image_format = image_format_result.take_value();
  EXPECT_EQ(kBytesPerRow, image_format.bytes_per_row());

  byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 0);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(0u, byte_offset_checked.ValueOrDie());

  byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 1);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(kBytesPerRow * 20, byte_offset_checked.ValueOrDie());

  byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 2);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(kBytesPerRow * 20 + kBytesPerRow / 2 * 20 / 2, byte_offset_checked.ValueOrDie());

  EXPECT_FALSE(ImageFormatPlaneByteOffsetChecked(image_format, 3).IsValid());

  auto row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 0);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(kBytesPerRow, row_bytes_checked.ValueOrDie());

  row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 1);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(kBytesPerRow / 2, row_bytes_checked.ValueOrDie());

  row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 2);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(kBytesPerRow / 2, row_bytes_checked.ValueOrDie());

  EXPECT_FALSE(ImageFormatPlaneRowBytesChecked(image_format, 3).IsValid());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, PlaneByteOffset_V2_wire) {
  fidl::Arena allocator;
  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  constraints.set_pixel_format_modifier(allocator,
                                        fuchsia_images2::wire::PixelFormatModifier::kLinear);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
  constraints.set_bytes_per_row_divisor(4u * 8u);
  constraints.set_max_bytes_per_row(100000u);

  auto image_format_result = ImageConstraintsToFormat(allocator, constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row());

  uint64_t byte_offset;
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 0, &byte_offset));
  EXPECT_EQ(0u, byte_offset);
  EXPECT_FALSE(ImageFormatPlaneByteOffset(image_format, 1, &byte_offset));

  auto constraints2 = sysmem::V2CloneImageFormatConstraints(allocator, constraints);
  constraints2.pixel_format() = fuchsia_images2::PixelFormat::kI420;

  constexpr uint32_t kBytesPerRow = 32;
  image_format_result = ImageConstraintsToFormat(allocator, constraints2, 18, 20);
  EXPECT_TRUE(image_format_result.is_ok());
  image_format = image_format_result.take_value();
  EXPECT_EQ(kBytesPerRow, image_format.bytes_per_row());
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 0, &byte_offset));
  EXPECT_EQ(0u, byte_offset);
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 1, &byte_offset));
  EXPECT_EQ(kBytesPerRow * 20, byte_offset);
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 2, &byte_offset));
  EXPECT_EQ(kBytesPerRow * 20 + kBytesPerRow / 2 * 20 / 2, byte_offset);
  EXPECT_FALSE(ImageFormatPlaneByteOffset(image_format, 3, &byte_offset));

  uint32_t row_bytes;
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 0, &row_bytes));
  EXPECT_EQ(kBytesPerRow, row_bytes);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 1, &row_bytes));
  EXPECT_EQ(kBytesPerRow / 2, row_bytes);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 2, &row_bytes));
  EXPECT_EQ(kBytesPerRow / 2, row_bytes);
  EXPECT_FALSE(ImageFormatPlaneRowBytes(image_format, 3, &row_bytes));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, PlaneByteOffsetChecked_V2_wire) {
  fidl::Arena allocator;
  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  constraints.set_pixel_format_modifier(allocator,
                                        fuchsia_images2::wire::PixelFormatModifier::kLinear);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
  constraints.set_bytes_per_row_divisor(4u * 8u);
  constraints.set_max_bytes_per_row(100000u);

  auto image_format_result = ImageConstraintsToFormat(allocator, constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row());

  auto byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 0);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(0u, byte_offset_checked.ValueOrDie());
  EXPECT_FALSE(ImageFormatPlaneByteOffsetChecked(image_format, 1).IsValid());

  auto constraints2 = sysmem::V2CloneImageFormatConstraints(allocator, constraints);
  constraints2.pixel_format() = fuchsia_images2::PixelFormat::kI420;

  constexpr uint32_t kBytesPerRow = 32;
  image_format_result = ImageConstraintsToFormat(allocator, constraints2, 18, 20);
  EXPECT_TRUE(image_format_result.is_ok());
  image_format = image_format_result.take_value();
  EXPECT_EQ(kBytesPerRow, image_format.bytes_per_row());

  byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 0);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(0u, byte_offset_checked.ValueOrDie());

  byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 1);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(kBytesPerRow * 20, byte_offset_checked.ValueOrDie());

  byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 2);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(kBytesPerRow * 20 + kBytesPerRow / 2 * 20 / 2, byte_offset_checked.ValueOrDie());

  EXPECT_FALSE(ImageFormatPlaneByteOffsetChecked(image_format, 3).IsValid());

  auto row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 0);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(kBytesPerRow, row_bytes_checked.ValueOrDie());

  row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 1);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(kBytesPerRow / 2, row_bytes_checked.ValueOrDie());

  row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 2);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(kBytesPerRow / 2, row_bytes_checked.ValueOrDie());

  EXPECT_FALSE(ImageFormatPlaneRowBytesChecked(image_format, 3).IsValid());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, PlaneByteOffset_V1_wire) {
  sysmem_v1::wire::PixelFormat linear = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierLinear,
          },
  };
  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = linear,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .min_coded_height = 12,
      .max_coded_height = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 4 * 8,
  };

  auto image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row);

  uint64_t byte_offset;
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 0, &byte_offset));
  EXPECT_EQ(0u, byte_offset);
  EXPECT_FALSE(ImageFormatPlaneByteOffset(image_format, 1, &byte_offset));

  constraints.pixel_format.type = sysmem_v1::wire::PixelFormatType::kI420;

  constexpr uint32_t kBytesPerRow = 32;
  image_format_result = ImageConstraintsToFormat(constraints, 18, 20);
  EXPECT_TRUE(image_format_result.is_ok());
  image_format = image_format_result.take_value();
  EXPECT_EQ(kBytesPerRow, image_format.bytes_per_row);
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 0, &byte_offset));
  EXPECT_EQ(0u, byte_offset);
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 1, &byte_offset));
  EXPECT_EQ(kBytesPerRow * 20, byte_offset);
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 2, &byte_offset));
  EXPECT_EQ(kBytesPerRow * 20 + kBytesPerRow / 2 * 20 / 2, byte_offset);
  EXPECT_FALSE(ImageFormatPlaneByteOffset(image_format, 3, &byte_offset));

  uint32_t row_bytes;
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 0, &row_bytes));
  EXPECT_EQ(kBytesPerRow, row_bytes);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 1, &row_bytes));
  EXPECT_EQ(kBytesPerRow / 2, row_bytes);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 2, &row_bytes));
  EXPECT_EQ(kBytesPerRow / 2, row_bytes);
  EXPECT_FALSE(ImageFormatPlaneRowBytes(image_format, 3, &row_bytes));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, PlaneByteOffsetChecked_V1_wire) {
  sysmem_v1::wire::PixelFormat linear = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierLinear,
          },
  };
  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = linear,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .min_coded_height = 12,
      .max_coded_height = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 4 * 8,
  };

  auto image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row);

  auto byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 0);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(0u, byte_offset_checked.ValueOrDie());
  EXPECT_FALSE(ImageFormatPlaneByteOffsetChecked(image_format, 1).IsValid());

  constraints.pixel_format.type = sysmem_v1::wire::PixelFormatType::kI420;

  constexpr uint32_t kBytesPerRow = 32;
  image_format_result = ImageConstraintsToFormat(constraints, 18, 20);
  EXPECT_TRUE(image_format_result.is_ok());
  image_format = image_format_result.take_value();
  EXPECT_EQ(kBytesPerRow, image_format.bytes_per_row);

  byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 0);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(0u, byte_offset_checked.ValueOrDie());

  byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 1);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(kBytesPerRow * 20, byte_offset_checked.ValueOrDie());

  byte_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 2);
  EXPECT_TRUE(byte_offset_checked.IsValid());
  EXPECT_EQ(kBytesPerRow * 20 + kBytesPerRow / 2 * 20 / 2, byte_offset_checked.ValueOrDie());

  EXPECT_FALSE(ImageFormatPlaneByteOffsetChecked(image_format, 3).IsValid());

  auto row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 0);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(kBytesPerRow, row_bytes_checked.ValueOrDie());

  row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 1);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(kBytesPerRow / 2, row_bytes_checked.ValueOrDie());

  row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 2);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(kBytesPerRow / 2, row_bytes_checked.ValueOrDie());

  EXPECT_FALSE(ImageFormatPlaneRowBytesChecked(image_format, 3).IsValid());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, TransactionEliminationFormats_V2) {
  PixelFormatAndModifier format;
  format.pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;
  format.pixel_format_modifier = fuchsia_images2::PixelFormatModifier::kLinear;

  EXPECT_TRUE(ImageFormatCompatibleWithProtectedMemory(format));

  auto format2 = format;
  format2.pixel_format_modifier = fuchsia_images2::PixelFormatModifier::kArmLinearTe;

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format2));

  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = format2.pixel_format;
  constraints.pixel_format_modifier() = format2.pixel_format_modifier;
  constraints.min_size() = {12u, 12u};
  constraints.max_size() = {100u, 100u};
  constraints.bytes_per_row_divisor().emplace(4u * 8u);
  constraints.max_bytes_per_row().emplace(100000u);

  auto image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row());

  // Check the color plane data.
  uint32_t row_bytes;
  uint64_t plane_offset;
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 0, &plane_offset));
  EXPECT_EQ(0u, plane_offset);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 0, &row_bytes));
  EXPECT_EQ(image_format.bytes_per_row(), row_bytes);

  constexpr uint32_t kTePlane = 3;
  // Check the TE plane data.
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, kTePlane, &plane_offset));
  EXPECT_LE(image_format.bytes_per_row().value() * 17, plane_offset);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, kTePlane, &row_bytes));

  // Row size should be rounded up to 64 bytes.
  EXPECT_EQ(64, row_bytes);
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, TransactionEliminationBufferSize_ArmV12) {
  // Test that for 1280x800, the TE CRC buffer size requires 52 rows of 640 bytes
  // plus the 64-byte header, totaling 33,344 bytes (to satisfy Arm Mali v12+
  // 64x64 region fetch requirements).
  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kArmLinearTe;
  constraints.min_size() = {1280u, 800u};
  constraints.max_size() = {1280u, 800u};
  constraints.bytes_per_row_divisor().emplace(1u);
  constraints.max_bytes_per_row().emplace(100000u);

  auto image_format_result = ImageConstraintsToFormat(constraints, 1280, 800);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();

  constexpr uint32_t kTePlane = 3;
  auto plane_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, kTePlane);
  EXPECT_TRUE(plane_offset_checked.IsValid());
  auto total_size_checked = ImageFormatImageSizeChecked(image_format);
  EXPECT_TRUE(total_size_checked.IsValid());
  EXPECT_EQ(33344u, (total_size_checked - plane_offset_checked).ValueOrDie());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, TransactionEliminationFormatsChecked_V2) {
  PixelFormatAndModifier format;
  format.pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;
  format.pixel_format_modifier = fuchsia_images2::PixelFormatModifier::kLinear;

  EXPECT_TRUE(ImageFormatCompatibleWithProtectedMemory(format));

  auto format2 = format;
  format2.pixel_format_modifier = fuchsia_images2::PixelFormatModifier::kArmLinearTe;

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format2));

  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = format2.pixel_format;
  constraints.pixel_format_modifier() = format2.pixel_format_modifier;
  constraints.min_size() = {12u, 12u};
  constraints.max_size() = {100u, 100u};
  constraints.bytes_per_row_divisor().emplace(4u * 8u);
  constraints.max_bytes_per_row().emplace(100000u);

  auto image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row());

  // Check the color plane data.
  auto plane_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 0);
  EXPECT_TRUE(plane_offset_checked.IsValid());
  EXPECT_EQ(0u, plane_offset_checked.ValueOrDie());
  auto row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 0);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(image_format.bytes_per_row(), row_bytes_checked.ValueOrDie());

  constexpr uint32_t kTePlane = 3;
  // Check the TE plane data.
  plane_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, kTePlane);
  EXPECT_TRUE(plane_offset_checked.IsValid());
  EXPECT_LE(image_format.bytes_per_row().value() * 17, plane_offset_checked.ValueOrDie());
  row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, kTePlane);
  EXPECT_TRUE(row_bytes_checked.IsValid());

  // Row size should be rounded up to 64 bytes.
  EXPECT_EQ(64, row_bytes_checked.ValueOrDie());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, TransactionEliminationFormats_V2_wire) {
  fidl::Arena allocator;
  PixelFormatAndModifier format;
  format.pixel_format = fuchsia_images2::wire::PixelFormat::kB8G8R8A8;
  format.pixel_format_modifier = fuchsia_images2::wire::PixelFormatModifier::kLinear;

  EXPECT_TRUE(ImageFormatCompatibleWithProtectedMemory(format));

  auto format2 = format;
  format2.pixel_format_modifier = fuchsia_images2::wire::PixelFormatModifier::kArmLinearTe;

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format2));

  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(format2.pixel_format);
  constraints.set_pixel_format_modifier(allocator, format2.pixel_format_modifier);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
  constraints.set_bytes_per_row_divisor(4u * 8u);
  constraints.set_max_bytes_per_row(100000u);

  auto image_format_result = ImageConstraintsToFormat(allocator, constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row());

  // Check the color plane data.
  uint32_t row_bytes;
  uint64_t plane_offset;
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 0, &plane_offset));
  EXPECT_EQ(0u, plane_offset);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 0, &row_bytes));
  EXPECT_EQ(image_format.bytes_per_row(), row_bytes);

  constexpr uint32_t kTePlane = 3;
  // Check the TE plane data.
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, kTePlane, &plane_offset));
  EXPECT_LE(image_format.bytes_per_row() * 17, plane_offset);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, kTePlane, &row_bytes));

  // Row size should be rounded up to 64 bytes.
  EXPECT_EQ(64, row_bytes);
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, TransactionEliminationFormatsChecked_V2_wire) {
  fidl::Arena allocator;
  PixelFormatAndModifier format;
  format.pixel_format = fuchsia_images2::wire::PixelFormat::kB8G8R8A8;
  format.pixel_format_modifier = fuchsia_images2::wire::PixelFormatModifier::kLinear;

  EXPECT_TRUE(ImageFormatCompatibleWithProtectedMemory(format));

  auto format2 = format;
  format2.pixel_format_modifier = fuchsia_images2::wire::PixelFormatModifier::kArmLinearTe;

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format2));

  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(format2.pixel_format);
  constraints.set_pixel_format_modifier(allocator, format2.pixel_format_modifier);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
  constraints.set_bytes_per_row_divisor(4u * 8u);
  constraints.set_max_bytes_per_row(100000u);

  auto image_format_result = ImageConstraintsToFormat(allocator, constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row());

  // Check the color plane data.
  auto plane_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 0);
  EXPECT_TRUE(plane_offset_checked.IsValid());
  EXPECT_EQ(0u, plane_offset_checked.ValueOrDie());
  auto row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 0);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(image_format.bytes_per_row(), row_bytes_checked.ValueOrDie());

  constexpr uint32_t kTePlane = 3;
  // Check the TE plane data.
  plane_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, kTePlane);
  EXPECT_TRUE(plane_offset_checked.IsValid());
  EXPECT_LE(image_format.bytes_per_row() * 17, plane_offset_checked.ValueOrDie());
  row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, kTePlane);
  EXPECT_TRUE(row_bytes_checked.IsValid());

  // Row size should be rounded up to 64 bytes.
  EXPECT_EQ(64, row_bytes_checked.ValueOrDie());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, TransactionEliminationFormats_V1_wire) {
  sysmem_v1::wire::PixelFormat format = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierLinear,
          },
  };
  EXPECT_TRUE(ImageFormatCompatibleWithProtectedMemory(format));

  format.format_modifier.value = sysmem_v1::wire::kFormatModifierArmLinearTe;
  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format));

  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = format,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .min_coded_height = 12,
      .max_coded_height = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 4 * 8,
  };

  auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(optional_format);
  auto& image_format = optional_format.value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row);

  // Check the color plane data.
  uint32_t row_bytes;
  uint64_t plane_offset;
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, 0, &plane_offset));
  EXPECT_EQ(0u, plane_offset);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, 0, &row_bytes));
  EXPECT_EQ(image_format.bytes_per_row, row_bytes);

  constexpr uint32_t kTePlane = 3;
  // Check the TE plane data.
  EXPECT_TRUE(ImageFormatPlaneByteOffset(image_format, kTePlane, &plane_offset));
  EXPECT_LE(image_format.bytes_per_row * 17, plane_offset);
  EXPECT_TRUE(ImageFormatPlaneRowBytes(image_format, kTePlane, &row_bytes));

  // Row size should be rounded up to 64 bytes.
  EXPECT_EQ(64, row_bytes);
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, TransactionEliminationFormatsChecked_V1_wire) {
  sysmem_v1::wire::PixelFormat format = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierLinear,
          },
  };
  EXPECT_TRUE(ImageFormatCompatibleWithProtectedMemory(format));

  format.format_modifier.value = sysmem_v1::wire::kFormatModifierArmLinearTe;
  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format));

  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = format,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .min_coded_height = 12,
      .max_coded_height = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 4 * 8,
  };

  auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(optional_format);
  auto& image_format = optional_format.value();
  // The raw size would be 72 without bytes_per_row_divisor of 32.
  EXPECT_EQ(96u, image_format.bytes_per_row);

  // Check the color plane data.
  auto plane_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, 0);
  EXPECT_TRUE(plane_offset_checked.IsValid());
  EXPECT_EQ(0u, plane_offset_checked.ValueOrDie());
  auto row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, 0);
  EXPECT_TRUE(row_bytes_checked.IsValid());
  EXPECT_EQ(image_format.bytes_per_row, row_bytes_checked.ValueOrDie());

  constexpr uint32_t kTePlane = 3;
  // Check the TE plane data.
  plane_offset_checked = ImageFormatPlaneByteOffsetChecked(image_format, kTePlane);
  EXPECT_TRUE(plane_offset_checked.IsValid());
  EXPECT_LE(image_format.bytes_per_row * 17, plane_offset_checked.ValueOrDie());
  row_bytes_checked = ImageFormatPlaneRowBytesChecked(image_format, kTePlane);
  EXPECT_TRUE(row_bytes_checked.IsValid());

  // Row size should be rounded up to 64 bytes.
  EXPECT_EQ(64, row_bytes_checked.ValueOrDie());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, BasicSizes_V2) {
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  fuchsia_images2::ImageFormat image_format_bgra32;
  image_format_bgra32.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  image_format_bgra32.size() = {kWidth, kHeight};
  image_format_bgra32.bytes_per_row().emplace(kStride);
  EXPECT_EQ(kHeight * kStride, ImageFormatImageSize(image_format_bgra32));
  auto pixel_format_and_modifier_bgra32 =
      PixelFormatAndModifierFromImageFormat(image_format_bgra32);
  EXPECT_EQ(1, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_bgra32));
  EXPECT_EQ(1, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_bgra32));
  EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier_bgra32));

  fuchsia_images2::ImageFormat image_format_nv12;
  image_format_nv12.pixel_format() = fuchsia_images2::PixelFormat::kNv12;
  image_format_nv12.size() = {kWidth, kHeight};
  image_format_nv12.bytes_per_row() = kStride;
  EXPECT_EQ(kHeight * kStride * 3 / 2, ImageFormatImageSize(image_format_nv12));
  auto pixel_format_and_modifier_nv12 = PixelFormatAndModifierFromImageFormat(image_format_nv12);
  EXPECT_EQ(2, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_nv12));
  EXPECT_EQ(2, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_nv12));
  EXPECT_EQ(2, ImageFormatSampleAlignment(pixel_format_and_modifier_nv12));

  fuchsia_images2::ImageFormat image_format_p010;
  image_format_p010.pixel_format() = fuchsia_images2::PixelFormat::kP010;
  image_format_p010.size() = {kWidth, kHeight};
  image_format_p010.bytes_per_row() = kStride;
  EXPECT_EQ(kHeight * kStride * 3 / 2, ImageFormatImageSize(image_format_p010));
  auto pixel_format_and_modifier_p010 = PixelFormatAndModifierFromImageFormat(image_format_p010);
  EXPECT_EQ(2, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_p010));
  EXPECT_EQ(2, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_p010));
  EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier_p010));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, BasicSizesChecked_V2) {
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  fuchsia_images2::ImageFormat image_format_bgra32;
  image_format_bgra32.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  image_format_bgra32.size() = {kWidth, kHeight};
  image_format_bgra32.bytes_per_row().emplace(kStride);
  auto size_checked_bgra32 = ImageFormatImageSizeChecked(image_format_bgra32);
  EXPECT_TRUE(size_checked_bgra32.IsValid());
  EXPECT_EQ(kHeight * kStride, size_checked_bgra32.ValueOrDie());
  auto pixel_format_and_modifier_bgra32 =
      PixelFormatAndModifierFromImageFormat(image_format_bgra32);
  EXPECT_EQ(1, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_bgra32));
  EXPECT_EQ(1, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_bgra32));
  EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier_bgra32));

  fuchsia_images2::ImageFormat image_format_nv12;
  image_format_nv12.pixel_format() = fuchsia_images2::PixelFormat::kNv12;
  image_format_nv12.size() = {kWidth, kHeight};
  image_format_nv12.bytes_per_row() = kStride;
  auto size_checked_nv12 = ImageFormatImageSizeChecked(image_format_nv12);
  EXPECT_TRUE(size_checked_nv12.IsValid());
  EXPECT_EQ(kHeight * kStride * 3 / 2, size_checked_nv12.ValueOrDie());
  auto pixel_format_and_modifier_nv12 = PixelFormatAndModifierFromImageFormat(image_format_nv12);
  EXPECT_EQ(2, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_nv12));
  EXPECT_EQ(2, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_nv12));
  EXPECT_EQ(2, ImageFormatSampleAlignment(pixel_format_and_modifier_nv12));

  fuchsia_images2::ImageFormat image_format_p010;
  image_format_p010.pixel_format() = fuchsia_images2::PixelFormat::kP010;
  image_format_p010.size() = {kWidth, kHeight};
  image_format_p010.bytes_per_row() = kStride;
  auto size_checked_p010 = ImageFormatImageSizeChecked(image_format_p010);
  EXPECT_TRUE(size_checked_p010.IsValid());
  EXPECT_EQ(kHeight * kStride * 3 / 2, size_checked_p010.ValueOrDie());
  auto pixel_format_and_modifier_p010 = PixelFormatAndModifierFromImageFormat(image_format_p010);
  EXPECT_EQ(2, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_p010));
  EXPECT_EQ(2, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_p010));
  EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier_p010));
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, BasicSizes_V2_wire) {
  fidl::Arena allocator;
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  fuchsia_images2::wire::ImageFormat image_format_bgra32(allocator);
  image_format_bgra32.set_pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
  image_format_bgra32.set_size(allocator, fuchsia_math::wire::SizeU{kWidth, kHeight});
  image_format_bgra32.set_bytes_per_row(kStride);
  EXPECT_EQ(kHeight * kStride, ImageFormatImageSize(image_format_bgra32));
  auto pixel_format_and_modifier_bgra32 =
      PixelFormatAndModifierFromImageFormat(image_format_bgra32);
  EXPECT_EQ(1, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_bgra32));
  EXPECT_EQ(1, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_bgra32));
  EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier_bgra32));

  fuchsia_images2::wire::ImageFormat image_format_nv12(allocator);
  image_format_nv12.set_pixel_format(fuchsia_images2::PixelFormat::kNv12);
  image_format_nv12.set_size(allocator, fuchsia_math::wire::SizeU{kWidth, kHeight});
  image_format_nv12.set_bytes_per_row(kStride);
  EXPECT_EQ(kHeight * kStride * 3 / 2, ImageFormatImageSize(image_format_nv12));
  auto pixel_format_and_modifier_nv12 = PixelFormatAndModifierFromImageFormat(image_format_nv12);
  EXPECT_EQ(2, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_nv12));
  EXPECT_EQ(2, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_nv12));
  EXPECT_EQ(2, ImageFormatSampleAlignment(pixel_format_and_modifier_nv12));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, BasicSizesChecked_V2_wire) {
  fidl::Arena allocator;
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  fuchsia_images2::wire::ImageFormat image_format_bgra32(allocator);
  image_format_bgra32.set_pixel_format(fuchsia_images2::PixelFormat::kB8G8R8A8);
  image_format_bgra32.set_size(allocator, fuchsia_math::wire::SizeU{kWidth, kHeight});
  image_format_bgra32.set_bytes_per_row(kStride);
  auto size_checked_bgra32 = ImageFormatImageSizeChecked(image_format_bgra32);
  EXPECT_TRUE(size_checked_bgra32.IsValid());
  EXPECT_EQ(kHeight * kStride, size_checked_bgra32.ValueOrDie());
  auto pixel_format_and_modifier_bgra32 =
      PixelFormatAndModifierFromImageFormat(image_format_bgra32);
  EXPECT_EQ(1, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_bgra32));
  EXPECT_EQ(1, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_bgra32));
  EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier_bgra32));

  fuchsia_images2::wire::ImageFormat image_format_nv12(allocator);
  image_format_nv12.set_pixel_format(fuchsia_images2::PixelFormat::kNv12);
  image_format_nv12.set_size(allocator, fuchsia_math::wire::SizeU{kWidth, kHeight});
  image_format_nv12.set_bytes_per_row(kStride);
  auto size_checked_nv12 = ImageFormatImageSizeChecked(image_format_nv12);
  EXPECT_TRUE(size_checked_nv12.IsValid());
  EXPECT_EQ(kHeight * kStride * 3 / 2, size_checked_nv12.ValueOrDie());
  auto pixel_format_and_modifier_nv12 = PixelFormatAndModifierFromImageFormat(image_format_nv12);
  EXPECT_EQ(2, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_nv12));
  EXPECT_EQ(2, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_nv12));
  EXPECT_EQ(2, ImageFormatSampleAlignment(pixel_format_and_modifier_nv12));
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, BasicSizes_V1_wire) {
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = 256;

  sysmem_v1::wire::ImageFormat2 image_format_bgra32 = {
      .pixel_format =
          {
              .type = sysmem_v1::wire::PixelFormatType::kBgra32,
          },
      .coded_width = kWidth,
      .coded_height = kHeight,
      .bytes_per_row = kStride,
  };
  EXPECT_EQ(kHeight * kStride, ImageFormatImageSize(image_format_bgra32));
  EXPECT_EQ(1, ImageFormatCodedWidthMinDivisor(image_format_bgra32.pixel_format));
  EXPECT_EQ(1, ImageFormatCodedHeightMinDivisor(image_format_bgra32.pixel_format));
  EXPECT_EQ(4, ImageFormatSampleAlignment(image_format_bgra32.pixel_format));

  sysmem_v1::wire::ImageFormat2 image_format_nv12 = {
      .pixel_format =
          {
              .type = sysmem_v1::wire::PixelFormatType::kNv12,
          },
      .coded_width = kWidth,
      .coded_height = kHeight,
      .bytes_per_row = kStride,
  };
  EXPECT_EQ(kHeight * kStride * 3 / 2, ImageFormatImageSize(image_format_nv12));
  EXPECT_EQ(2, ImageFormatCodedWidthMinDivisor(image_format_nv12.pixel_format));
  EXPECT_EQ(2, ImageFormatCodedHeightMinDivisor(image_format_nv12.pixel_format));
  EXPECT_EQ(2, ImageFormatSampleAlignment(image_format_nv12.pixel_format));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, BasicSizesChecked_V1_wire) {
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = 256;

  sysmem_v1::wire::ImageFormat2 image_format_bgra32 = {
      .pixel_format =
          {
              .type = sysmem_v1::wire::PixelFormatType::kBgra32,
          },
      .coded_width = kWidth,
      .coded_height = kHeight,
      .bytes_per_row = kStride,
  };
  auto size_checked_bgra32 = ImageFormatImageSizeChecked(image_format_bgra32);
  EXPECT_TRUE(size_checked_bgra32.IsValid());
  EXPECT_EQ(kHeight * kStride, size_checked_bgra32.ValueOrDie());
  EXPECT_EQ(1, ImageFormatCodedWidthMinDivisor(image_format_bgra32.pixel_format));
  EXPECT_EQ(1, ImageFormatCodedHeightMinDivisor(image_format_bgra32.pixel_format));
  EXPECT_EQ(4, ImageFormatSampleAlignment(image_format_bgra32.pixel_format));

  sysmem_v1::wire::ImageFormat2 image_format_nv12 = {
      .pixel_format =
          {
              .type = sysmem_v1::wire::PixelFormatType::kNv12,
          },
      .coded_width = kWidth,
      .coded_height = kHeight,
      .bytes_per_row = kStride,
  };
  auto size_checked_nv12 = ImageFormatImageSizeChecked(image_format_nv12);
  EXPECT_TRUE(size_checked_nv12.IsValid());
  EXPECT_EQ(kHeight * kStride * 3 / 2, size_checked_nv12.ValueOrDie());
  EXPECT_EQ(2, ImageFormatCodedWidthMinDivisor(image_format_nv12.pixel_format));
  EXPECT_EQ(2, ImageFormatCodedHeightMinDivisor(image_format_nv12.pixel_format));
  EXPECT_EQ(2, ImageFormatSampleAlignment(image_format_nv12.pixel_format));
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, AfbcFlagFormats_V2) {
  PixelFormatAndModifier format;
  format.pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;
  format.pixel_format_modifier =
      fuchsia_images2::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTe;

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format));

  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = format.pixel_format;
  constraints.pixel_format_modifier() = format.pixel_format_modifier;
  constraints.min_size() = {12u, 12u};
  constraints.max_size() = {100u, 100u};
  constraints.bytes_per_row_divisor().emplace(4u * 8u);
  constraints.max_bytes_per_row().emplace(100000u);

  auto image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());

  constraints.pixel_format_modifier() =
      fuchsia_images2::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTiledHeader;

  image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  constexpr uint32_t kMinHeaderOffset = 4096u;
  constexpr uint32_t kMinWidth = 128;
  constexpr uint32_t kMinHeight = 128;
  EXPECT_EQ(kMinHeaderOffset + kMinWidth * kMinHeight * 4, ImageFormatImageSize(image_format));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, AfbcFlagFormatsChecked_V2) {
  PixelFormatAndModifier format;
  format.pixel_format = fuchsia_images2::PixelFormat::kB8G8R8A8;
  format.pixel_format_modifier =
      fuchsia_images2::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTe;

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format));

  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = format.pixel_format;
  constraints.pixel_format_modifier() = format.pixel_format_modifier;
  constraints.min_size() = {12u, 12u};
  constraints.max_size() = {100u, 100u};
  constraints.bytes_per_row_divisor().emplace(4u * 8u);
  constraints.max_bytes_per_row().emplace(100000u);

  auto image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());

  constraints.pixel_format_modifier() =
      fuchsia_images2::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTiledHeader;

  image_format_result = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  constexpr uint32_t kMinHeaderOffset = 4096u;
  constexpr uint32_t kMinWidth = 128;
  constexpr uint32_t kMinHeight = 128;
  auto size_checked = ImageFormatImageSizeChecked(image_format);
  EXPECT_TRUE(size_checked.IsValid());
  EXPECT_EQ(kMinHeaderOffset + kMinWidth * kMinHeight * 4, size_checked.ValueOrDie());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, AfbcFlagFormats_V2_wire) {
  fidl::Arena allocator;
  PixelFormatAndModifier format;
  format.pixel_format = fuchsia_images2::wire::PixelFormat::kB8G8R8A8;
  format.pixel_format_modifier =
      fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTe;

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format));

  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(format.pixel_format);
  constraints.set_pixel_format_modifier(allocator, format.pixel_format_modifier);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
  constraints.set_bytes_per_row_divisor(4u * 8u);
  constraints.set_max_bytes_per_row(100000u);

  auto image_format_result = ImageConstraintsToFormat(allocator, constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());

  constraints.set_pixel_format_modifier(
      allocator,
      fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTiledHeader);

  image_format_result = ImageConstraintsToFormat(allocator, constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  constexpr uint32_t kMinHeaderOffset = 4096u;
  constexpr uint32_t kMinWidth = 128;
  constexpr uint32_t kMinHeight = 128;
  EXPECT_EQ(kMinHeaderOffset + kMinWidth * kMinHeight * 4, ImageFormatImageSize(image_format));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, AfbcFlagFormatsChecked_V2_wire) {
  fidl::Arena allocator;
  PixelFormatAndModifier format;
  format.pixel_format = fuchsia_images2::wire::PixelFormat::kB8G8R8A8;
  format.pixel_format_modifier =
      fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTe;

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format));

  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(format.pixel_format);
  constraints.set_pixel_format_modifier(allocator, format.pixel_format_modifier);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
  constraints.set_bytes_per_row_divisor(4u * 8u);
  constraints.set_max_bytes_per_row(100000u);

  auto image_format_result = ImageConstraintsToFormat(allocator, constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());

  constraints.set_pixel_format_modifier(
      allocator,
      fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTiledHeader);

  image_format_result = ImageConstraintsToFormat(allocator, constraints, 18, 17);
  EXPECT_TRUE(image_format_result.is_ok());
  auto image_format = image_format_result.take_value();
  constexpr uint32_t kMinHeaderOffset = 4096u;
  constexpr uint32_t kMinWidth = 128;
  constexpr uint32_t kMinHeight = 128;
  auto size_checked = ImageFormatImageSizeChecked(image_format);
  EXPECT_TRUE(size_checked.IsValid());
  EXPECT_EQ(kMinHeaderOffset + kMinWidth * kMinHeight * 4, size_checked.ValueOrDie());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, AfbcFlagFormats_V1_wire) {
  sysmem_v1::wire::PixelFormat format = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierArmAfbc16X16SplitBlockSparseYuvTe,
          },
  };

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format));

  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = format,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .min_coded_height = 12,
      .max_coded_height = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 4 * 8,
  };

  auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(optional_format);

  sysmem_v1::wire::PixelFormat tiled_format = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierArmAfbc16X16SplitBlockSparseYuvTiledHeader,
          },
  };

  constraints.pixel_format = tiled_format;

  optional_format = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(optional_format);
  auto& image_format = optional_format.value();
  constexpr uint32_t kMinHeaderOffset = 4096u;
  constexpr uint32_t kMinWidth = 128;
  constexpr uint32_t kMinHeight = 128;
  EXPECT_EQ(kMinHeaderOffset + kMinWidth * kMinHeight * 4, ImageFormatImageSize(image_format));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, AfbcFlagFormatsChecked_V1_wire) {
  sysmem_v1::wire::PixelFormat format = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierArmAfbc16X16SplitBlockSparseYuvTe,
          },
  };

  EXPECT_FALSE(ImageFormatCompatibleWithProtectedMemory(format));

  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = format,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .min_coded_height = 12,
      .max_coded_height = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 4 * 8,
  };

  auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(optional_format);

  sysmem_v1::wire::PixelFormat tiled_format = {
      .type = sysmem_v1::wire::PixelFormatType::kBgra32,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierArmAfbc16X16SplitBlockSparseYuvTiledHeader,
          },
  };

  constraints.pixel_format = tiled_format;

  optional_format = ImageConstraintsToFormat(constraints, 18, 17);
  EXPECT_TRUE(optional_format);
  auto& image_format = optional_format.value();
  constexpr uint32_t kMinHeaderOffset = 4096u;
  constexpr uint32_t kMinWidth = 128;
  constexpr uint32_t kMinHeight = 128;
  auto size_checked = ImageFormatImageSizeChecked(image_format);
  EXPECT_TRUE(size_checked.IsValid());
  EXPECT_EQ(kMinHeaderOffset + kMinWidth * kMinHeight * 4, size_checked.ValueOrDie());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, R8G8Formats_V1_wire) {
  sysmem_v1::wire::PixelFormat format = {
      .type = sysmem_v1::wire::PixelFormatType::kR8G8,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierLinear,
          },
  };

  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = format,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .min_coded_height = 12,
      .max_coded_height = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 1,
  };

  {
    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format);
    auto& image_format = optional_format.value();
    EXPECT_EQ(18u * 2, image_format.bytes_per_row);
    EXPECT_EQ(18u * 17u * 2, ImageFormatImageSize(image_format));
  }

  constraints.pixel_format.type = sysmem_v1::wire::PixelFormatType::kR8;

  {
    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format);
    auto& image_format = optional_format.value();
    EXPECT_EQ(18u * 1, image_format.bytes_per_row);
    EXPECT_EQ(18u * 17u * 1, ImageFormatImageSize(image_format));
  }
}

TEST(ImageFormat, R8G8Formats_V2) {
  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8G8;
  constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  constraints.min_size() = {12u, 12u};
  constraints.max_size() = {100u, 100u};
  constraints.bytes_per_row_divisor().emplace(1u);
  constraints.max_bytes_per_row().emplace(100000u);

  {
    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    EXPECT_EQ(18u * 2, image_format.bytes_per_row());
    EXPECT_EQ(18u * 17u * 2, ImageFormatImageSize(image_format));
  }

  constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8;

  {
    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    EXPECT_EQ(18u * 1, image_format.bytes_per_row());
    EXPECT_EQ(18u * 17u * 1, ImageFormatImageSize(image_format));
  }
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, R8G8FormatsChecked_V2) {
  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8G8;
  constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  constraints.min_size() = {12u, 12u};
  constraints.max_size() = {100u, 100u};
  constraints.bytes_per_row_divisor().emplace(1u);
  constraints.max_bytes_per_row().emplace(100000u);

  {
    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    auto bytes_per_row_checked = ImageFormatMinimumRowBytesChecked(constraints, 18);
    EXPECT_TRUE(bytes_per_row_checked.IsValid());
    EXPECT_EQ(18u * 2, bytes_per_row_checked.ValueOrDie());
    auto size_checked = ImageFormatImageSizeChecked(image_format);
    EXPECT_TRUE(size_checked.IsValid());
    EXPECT_EQ(18u * 17u * 2, size_checked.ValueOrDie());
  }

  constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8;

  {
    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    auto bytes_per_row_checked = ImageFormatMinimumRowBytesChecked(constraints, 18);
    EXPECT_TRUE(bytes_per_row_checked.IsValid());
    EXPECT_EQ(18u * 1, bytes_per_row_checked.ValueOrDie());
    auto size_checked = ImageFormatImageSizeChecked(image_format);
    EXPECT_TRUE(size_checked.IsValid());
    EXPECT_EQ(18u * 17u * 1, size_checked.ValueOrDie());
  }
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, R8G8Formats_V2_wire) {
  fidl::Arena allocator;
  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(fuchsia_images2::wire::PixelFormat::kR8G8);
  constraints.set_pixel_format_modifier(allocator,
                                        fuchsia_images2::wire::PixelFormatModifier::kLinear);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
  constraints.set_bytes_per_row_divisor(1u);
  constraints.set_max_bytes_per_row(100000u);

  {
    auto optional_format = ImageConstraintsToFormat(allocator, constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    EXPECT_EQ(18u * 2, image_format.bytes_per_row());
    EXPECT_EQ(18u * 17u * 2, ImageFormatImageSize(image_format));
  }

  constraints.set_pixel_format(fuchsia_images2::wire::PixelFormat::kR8);

  {
    auto optional_format = ImageConstraintsToFormat(allocator, constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    EXPECT_EQ(18u * 1, image_format.bytes_per_row());
    EXPECT_EQ(18u * 17u * 1, ImageFormatImageSize(image_format));
  }
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, R8G8FormatsChecked_V2_wire) {
  fidl::Arena allocator;
  sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
  constraints.set_pixel_format(fuchsia_images2::wire::PixelFormat::kR8G8);
  constraints.set_pixel_format_modifier(allocator,
                                        fuchsia_images2::wire::PixelFormatModifier::kLinear);
  constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
  constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
  constraints.set_bytes_per_row_divisor(1u);
  constraints.set_max_bytes_per_row(100000u);

  {
    auto optional_format = ImageConstraintsToFormat(allocator, constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    auto bytes_per_row_checked = ImageFormatMinimumRowBytesChecked(constraints, 18);
    EXPECT_TRUE(bytes_per_row_checked.IsValid());
    EXPECT_EQ(18u * 2, bytes_per_row_checked.ValueOrDie());
    auto size_checked = ImageFormatImageSizeChecked(image_format);
    EXPECT_TRUE(size_checked.IsValid());
    EXPECT_EQ(18u * 17u * 2, size_checked.ValueOrDie());
  }

  constraints.set_pixel_format(fuchsia_images2::wire::PixelFormat::kR8);

  {
    auto optional_format = ImageConstraintsToFormat(allocator, constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    auto bytes_per_row_checked = ImageFormatMinimumRowBytesChecked(constraints, 18);
    EXPECT_TRUE(bytes_per_row_checked.IsValid());
    EXPECT_EQ(18u * 1, bytes_per_row_checked.ValueOrDie());
    auto size_checked = ImageFormatImageSizeChecked(image_format);
    EXPECT_TRUE(size_checked.IsValid());
    EXPECT_EQ(18u * 17u * 1, size_checked.ValueOrDie());
  }
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, R8G8FormatsChecked_V1_wire) {
  sysmem_v1::wire::PixelFormat format = {
      .type = sysmem_v1::wire::PixelFormatType::kR8G8,
      .has_format_modifier = true,
      .format_modifier =
          {
              .value = sysmem_v1::wire::kFormatModifierLinear,
          },
  };

  sysmem_v1::wire::ImageFormatConstraints constraints = {
      .pixel_format = format,
      .min_coded_width = 12,
      .max_coded_width = 100,
      .min_coded_height = 12,
      .max_coded_height = 100,
      .max_bytes_per_row = 100000,
      .bytes_per_row_divisor = 1,
  };

  {
    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format);
    auto& image_format = optional_format.value();
    auto bytes_per_row_checked = ImageFormatMinimumRowBytesChecked(constraints, 18);
    EXPECT_TRUE(bytes_per_row_checked.IsValid());
    EXPECT_EQ(18u * 2, bytes_per_row_checked.ValueOrDie());
    auto size_checked = ImageFormatImageSizeChecked(image_format);
    EXPECT_TRUE(size_checked.IsValid());
    EXPECT_EQ(18u * 17u * 2, size_checked.ValueOrDie());
  }

  constraints.pixel_format.type = sysmem_v1::wire::PixelFormatType::kR8;

  {
    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format);
    auto& image_format = optional_format.value();
    auto bytes_per_row_checked = ImageFormatMinimumRowBytesChecked(constraints, 18);
    EXPECT_TRUE(bytes_per_row_checked.IsValid());
    EXPECT_EQ(18u * 1, bytes_per_row_checked.ValueOrDie());
    auto size_checked = ImageFormatImageSizeChecked(image_format);
    EXPECT_TRUE(size_checked.IsValid());
    EXPECT_EQ(18u * 17u * 1, size_checked.ValueOrDie());
  }
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, A2R10G10B10_Formats_V1_wire) {
  for (const auto& pixel_format_type : {sysmem_v1::wire::PixelFormatType::kA2R10G10B10,
                                        sysmem_v1::wire::PixelFormatType::kA2B10G10R10}) {
    sysmem_v1::wire::PixelFormat format = {
        .type = pixel_format_type,
        .has_format_modifier = true,
        .format_modifier =
            {
                .value = sysmem_v1::wire::kFormatModifierLinear,
            },
    };

    sysmem_v1::wire::ImageFormatConstraints constraints = {
        .pixel_format = format,
        .min_coded_width = 12,
        .max_coded_width = 100,
        .min_coded_height = 12,
        .max_coded_height = 100,
        .max_bytes_per_row = 100000,
        .bytes_per_row_divisor = 1,
    };

    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format);
    auto& image_format = optional_format.value();
    EXPECT_EQ(18u * 4, image_format.bytes_per_row);
    EXPECT_EQ(18u * 17u * 4, ImageFormatImageSize(image_format));
    EXPECT_EQ(1, ImageFormatCodedWidthMinDivisor(image_format.pixel_format));
    EXPECT_EQ(1, ImageFormatCodedHeightMinDivisor(image_format.pixel_format));
    EXPECT_EQ(4, ImageFormatSampleAlignment(image_format.pixel_format));
  }
}

TEST(ImageFormat, A2R10G10B10_Formats_V2) {
  for (const auto& pixel_format :
       {fuchsia_images2::PixelFormat::kA2R10G10B10, fuchsia_images2::PixelFormat::kA2B10G10R10}) {
    sysmem_v2::ImageFormatConstraints constraints;
    constraints.pixel_format() = pixel_format;
    constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
    constraints.min_size() = {12u, 12u};
    constraints.max_size() = {100u, 100u};
    constraints.bytes_per_row_divisor().emplace(1u);
    constraints.max_bytes_per_row().emplace(100000u);

    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    EXPECT_EQ(18u * 4, image_format.bytes_per_row());
    EXPECT_EQ(18u * 17u * 4, ImageFormatImageSize(image_format));
    auto pixel_format_and_modifier = PixelFormatAndModifierFromImageFormat(image_format);
    EXPECT_EQ(1u, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier));
    EXPECT_EQ(1u, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier));
    EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier));
  }
}

TEST(ImageFormat, A2R10G10B10_Formats_V2_wire) {
  for (const auto& pixel_format : {fuchsia_images2::wire::PixelFormat::kA2R10G10B10,
                                   fuchsia_images2::wire::PixelFormat::kA2B10G10R10}) {
    fidl::Arena allocator;
    sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
    constraints.set_pixel_format(pixel_format);
    constraints.set_pixel_format_modifier(allocator,
                                          fuchsia_images2::wire::PixelFormatModifier::kLinear);
    constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
    constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
    constraints.set_bytes_per_row_divisor(1u);
    constraints.set_max_bytes_per_row(100000u);

    auto optional_format = ImageConstraintsToFormat(allocator, constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    EXPECT_EQ(18u * 4, image_format.bytes_per_row());
    EXPECT_EQ(18u * 17u * 4, ImageFormatImageSize(image_format));
    auto pixel_format_and_modifier = PixelFormatAndModifierFromImageFormat(image_format);
    EXPECT_EQ(1u, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier));
    EXPECT_EQ(1u, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier));
    EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier));
  }
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, A2R10G10B10_FormatsChecked_V2) {
  for (const auto& pixel_format :
       {fuchsia_images2::PixelFormat::kA2R10G10B10, fuchsia_images2::PixelFormat::kA2B10G10R10}) {
    sysmem_v2::ImageFormatConstraints constraints;
    constraints.pixel_format() = pixel_format;
    constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
    constraints.min_size() = {12u, 12u};
    constraints.max_size() = {100u, 100u};
    constraints.bytes_per_row_divisor().emplace(1u);
    constraints.max_bytes_per_row().emplace(100000u);

    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    auto bytes_per_row_checked = ImageFormatMinimumRowBytesChecked(constraints, 18);
    EXPECT_TRUE(bytes_per_row_checked.IsValid());
    EXPECT_EQ(18u * 4, bytes_per_row_checked.ValueOrDie());
    auto size_checked = ImageFormatImageSizeChecked(image_format);
    EXPECT_TRUE(size_checked.IsValid());
    EXPECT_EQ(18u * 17u * 4, size_checked.ValueOrDie());
    auto pixel_format_and_modifier = PixelFormatAndModifierFromImageFormat(image_format);
    EXPECT_EQ(1u, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier));
    EXPECT_EQ(1u, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier));
    EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier));
  }
}

TEST(ImageFormat, A2R10G10B10_FormatsChecked_V2_wire) {
  for (const auto& pixel_format : {fuchsia_images2::wire::PixelFormat::kA2R10G10B10,
                                   fuchsia_images2::wire::PixelFormat::kA2B10G10R10}) {
    fidl::Arena allocator;
    sysmem_v2::wire::ImageFormatConstraints constraints(allocator);
    constraints.set_pixel_format(pixel_format);
    constraints.set_pixel_format_modifier(allocator,
                                          fuchsia_images2::wire::PixelFormatModifier::kLinear);
    constraints.set_min_size(allocator, fuchsia_math::wire::SizeU{12u, 12u});
    constraints.set_max_size(allocator, fuchsia_math::wire::SizeU{100u, 100u});
    constraints.set_bytes_per_row_divisor(1u);
    constraints.set_max_bytes_per_row(100000u);

    auto optional_format = ImageConstraintsToFormat(allocator, constraints, 18, 17);
    EXPECT_TRUE(optional_format.is_ok());
    auto& image_format = optional_format.value();
    auto bytes_per_row_checked = ImageFormatMinimumRowBytesChecked(constraints, 18);
    EXPECT_TRUE(bytes_per_row_checked.IsValid());
    EXPECT_EQ(18u * 4, bytes_per_row_checked.ValueOrDie());
    auto size_checked = ImageFormatImageSizeChecked(image_format);
    EXPECT_TRUE(size_checked.IsValid());
    EXPECT_EQ(18u * 17u * 4, size_checked.ValueOrDie());
    auto pixel_format_and_modifier = PixelFormatAndModifierFromImageFormat(image_format);
    EXPECT_EQ(1u, ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier));
    EXPECT_EQ(1u, ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier));
    EXPECT_EQ(4, ImageFormatSampleAlignment(pixel_format_and_modifier));
  }
}

TEST(ImageFormat, A2R10G10B10_FormatsChecked_V1_wire) {
  for (const auto& pixel_format_type : {sysmem_v1::wire::PixelFormatType::kA2R10G10B10,
                                        sysmem_v1::wire::PixelFormatType::kA2B10G10R10}) {
    sysmem_v1::wire::PixelFormat format = {
        .type = pixel_format_type,
        .has_format_modifier = true,
        .format_modifier =
            {
                .value = sysmem_v1::wire::kFormatModifierLinear,
            },
    };

    sysmem_v1::wire::ImageFormatConstraints constraints = {
        .pixel_format = format,
        .min_coded_width = 12,
        .max_coded_width = 100,
        .min_coded_height = 12,
        .max_coded_height = 100,
        .max_bytes_per_row = 100000,
        .bytes_per_row_divisor = 1,
    };

    auto optional_format = ImageConstraintsToFormat(constraints, 18, 17);
    EXPECT_TRUE(optional_format);
    auto& image_format = optional_format.value();
    auto bytes_per_row_checked = ImageFormatMinimumRowBytesChecked(constraints, 18);
    EXPECT_TRUE(bytes_per_row_checked.IsValid());
    EXPECT_EQ(18u * 4, bytes_per_row_checked.ValueOrDie());
    auto size_checked = ImageFormatImageSizeChecked(image_format);
    EXPECT_TRUE(size_checked.IsValid());
    EXPECT_EQ(18u * 17u * 4, size_checked.ValueOrDie());
    EXPECT_EQ(1, ImageFormatCodedWidthMinDivisor(image_format.pixel_format));
    EXPECT_EQ(1, ImageFormatCodedHeightMinDivisor(image_format.pixel_format));
    EXPECT_EQ(4, ImageFormatSampleAlignment(image_format.pixel_format));
  }
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, GoldfishOptimal_V2) {
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  fuchsia_images2::ImageFormat linear_image_format_bgra32;
  linear_image_format_bgra32.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  linear_image_format_bgra32.size() = {kWidth, kHeight};
  linear_image_format_bgra32.bytes_per_row().emplace(kStride);

  fuchsia_images2::ImageFormat goldfish_optimal_image_format_bgra32;
  goldfish_optimal_image_format_bgra32.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  goldfish_optimal_image_format_bgra32.pixel_format_modifier() =
      fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal;
  goldfish_optimal_image_format_bgra32.size() = {kWidth, kHeight};
  goldfish_optimal_image_format_bgra32.bytes_per_row().emplace(kStride);
  EXPECT_EQ(ImageFormatImageSize(linear_image_format_bgra32),
            ImageFormatImageSize(goldfish_optimal_image_format_bgra32));
  auto pixel_format_and_modifier_linear_bgra32 =
      PixelFormatAndModifierFromImageFormat(linear_image_format_bgra32);
  auto pixel_format_and_modifier_goldfish_bgra32 =
      PixelFormatAndModifierFromImageFormat(goldfish_optimal_image_format_bgra32);
  EXPECT_EQ(ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_linear_bgra32),
            ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_goldfish_bgra32));
  EXPECT_EQ(ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_linear_bgra32),
            ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_goldfish_bgra32));
  EXPECT_EQ(ImageFormatSampleAlignment(pixel_format_and_modifier_linear_bgra32),
            ImageFormatSampleAlignment(pixel_format_and_modifier_goldfish_bgra32));
}

TEST(ImageFormat, GoldfishOptimal_V2_wire) {
  fidl::Arena allocator;
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  fuchsia_images2::wire::ImageFormat linear_image_format_bgra32(allocator);
  linear_image_format_bgra32.set_pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  linear_image_format_bgra32.set_size(allocator, fuchsia_math::wire::SizeU{kWidth, kHeight});
  linear_image_format_bgra32.set_bytes_per_row(kStride);

  fuchsia_images2::wire::ImageFormat goldfish_optimal_image_format_bgra32(allocator);
  goldfish_optimal_image_format_bgra32.set_pixel_format(
      fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  goldfish_optimal_image_format_bgra32.set_pixel_format_modifier(
      allocator, fuchsia_images2::wire::PixelFormatModifier::kGoogleGoldfishOptimal);
  goldfish_optimal_image_format_bgra32.set_size(allocator,
                                                fuchsia_math::wire::SizeU{kWidth, kHeight});
  goldfish_optimal_image_format_bgra32.set_bytes_per_row(kStride);
  EXPECT_EQ(ImageFormatImageSize(linear_image_format_bgra32),
            ImageFormatImageSize(goldfish_optimal_image_format_bgra32));
  auto linear_pixel_format_and_modifier =
      PixelFormatAndModifierFromImageFormat(linear_image_format_bgra32);
  auto goldfish_optimal_pixel_format_and_modifier =
      PixelFormatAndModifierFromImageFormat(goldfish_optimal_image_format_bgra32);
  EXPECT_EQ(ImageFormatSurfaceWidthMinDivisor(linear_pixel_format_and_modifier),
            ImageFormatSurfaceWidthMinDivisor(goldfish_optimal_pixel_format_and_modifier));
  EXPECT_EQ(ImageFormatSurfaceHeightMinDivisor(linear_pixel_format_and_modifier),
            ImageFormatSurfaceHeightMinDivisor(goldfish_optimal_pixel_format_and_modifier));
  EXPECT_EQ(ImageFormatSampleAlignment(linear_pixel_format_and_modifier),
            ImageFormatSampleAlignment(goldfish_optimal_pixel_format_and_modifier));
}

TEST(ImageFormat, GoldfishOptimal_V1_wire) {
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  sysmem_v1::wire::ImageFormat2 linear_image_format_bgra32 = {
      .pixel_format =
          {
              .type = sysmem_v1::wire::PixelFormatType::kBgra32,
              .has_format_modifier = true,
              .format_modifier = {.value = sysmem_v1::wire::kFormatModifierLinear},
          },
      .coded_width = kWidth,
      .coded_height = kHeight,
      .bytes_per_row = kStride,
  };

  sysmem_v1::wire::ImageFormat2 goldfish_optimal_image_format_bgra32 = {
      .pixel_format =
          {
              .type = sysmem_v1::wire::PixelFormatType::kBgra32,
              .has_format_modifier = true,
              .format_modifier = {.value = sysmem_v1::wire::kFormatModifierGoogleGoldfishOptimal},
          },
      .coded_width = kWidth,
      .coded_height = kHeight,
      .bytes_per_row = kStride,
  };

  EXPECT_EQ(ImageFormatImageSize(linear_image_format_bgra32),
            ImageFormatImageSize(goldfish_optimal_image_format_bgra32));
  EXPECT_EQ(ImageFormatCodedWidthMinDivisor(linear_image_format_bgra32.pixel_format),
            ImageFormatCodedWidthMinDivisor(goldfish_optimal_image_format_bgra32.pixel_format));
  EXPECT_EQ(ImageFormatCodedHeightMinDivisor(linear_image_format_bgra32.pixel_format),
            ImageFormatCodedHeightMinDivisor(goldfish_optimal_image_format_bgra32.pixel_format));
  EXPECT_EQ(ImageFormatSampleAlignment(linear_image_format_bgra32.pixel_format),
            ImageFormatSampleAlignment(goldfish_optimal_image_format_bgra32.pixel_format));
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, GoldfishOptimalChecked_V2) {
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  fuchsia_images2::ImageFormat linear_image_format_bgra32;
  linear_image_format_bgra32.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  linear_image_format_bgra32.size() = {kWidth, kHeight};
  linear_image_format_bgra32.bytes_per_row().emplace(kStride);

  fuchsia_images2::ImageFormat goldfish_optimal_image_format_bgra32;
  goldfish_optimal_image_format_bgra32.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
  goldfish_optimal_image_format_bgra32.pixel_format_modifier() =
      fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal;
  goldfish_optimal_image_format_bgra32.size() = {kWidth, kHeight};
  goldfish_optimal_image_format_bgra32.bytes_per_row().emplace(kStride);

  auto linear_size = ImageFormatImageSizeChecked(linear_image_format_bgra32);
  EXPECT_TRUE(linear_size.IsValid());
  auto goldfish_size = ImageFormatImageSizeChecked(goldfish_optimal_image_format_bgra32);
  EXPECT_TRUE(goldfish_size.IsValid());
  EXPECT_EQ(linear_size.ValueOrDie(), goldfish_size.ValueOrDie());

  auto pixel_format_and_modifier_linear_bgra32 =
      PixelFormatAndModifierFromImageFormat(linear_image_format_bgra32);
  auto pixel_format_and_modifier_goldfish_bgra32 =
      PixelFormatAndModifierFromImageFormat(goldfish_optimal_image_format_bgra32);
  EXPECT_EQ(ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_linear_bgra32),
            ImageFormatSurfaceWidthMinDivisor(pixel_format_and_modifier_goldfish_bgra32));
  EXPECT_EQ(ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_linear_bgra32),
            ImageFormatSurfaceHeightMinDivisor(pixel_format_and_modifier_goldfish_bgra32));
  EXPECT_EQ(ImageFormatSampleAlignment(pixel_format_and_modifier_linear_bgra32),
            ImageFormatSampleAlignment(pixel_format_and_modifier_goldfish_bgra32));
}

TEST(ImageFormat, GoldfishOptimalChecked_V2_wire) {
  fidl::Arena allocator;
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  fuchsia_images2::wire::ImageFormat linear_image_format_bgra32(allocator);
  linear_image_format_bgra32.set_pixel_format(fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  linear_image_format_bgra32.set_size(allocator, fuchsia_math::wire::SizeU{kWidth, kHeight});
  linear_image_format_bgra32.set_bytes_per_row(kStride);

  fuchsia_images2::wire::ImageFormat goldfish_optimal_image_format_bgra32(allocator);
  goldfish_optimal_image_format_bgra32.set_pixel_format(
      fuchsia_images2::wire::PixelFormat::kB8G8R8A8);
  goldfish_optimal_image_format_bgra32.set_pixel_format_modifier(
      allocator, fuchsia_images2::wire::PixelFormatModifier::kGoogleGoldfishOptimal);
  goldfish_optimal_image_format_bgra32.set_size(allocator,
                                                fuchsia_math::wire::SizeU{kWidth, kHeight});
  goldfish_optimal_image_format_bgra32.set_bytes_per_row(kStride);

  auto linear_size = ImageFormatImageSizeChecked(linear_image_format_bgra32);
  EXPECT_TRUE(linear_size.IsValid());
  auto goldfish_size = ImageFormatImageSizeChecked(goldfish_optimal_image_format_bgra32);
  EXPECT_TRUE(goldfish_size.IsValid());
  EXPECT_EQ(linear_size.ValueOrDie(), goldfish_size.ValueOrDie());

  auto linear_pixel_format_and_modifier =
      PixelFormatAndModifierFromImageFormat(linear_image_format_bgra32);
  auto goldfish_optimal_pixel_format_and_modifier =
      PixelFormatAndModifierFromImageFormat(goldfish_optimal_image_format_bgra32);
  EXPECT_EQ(ImageFormatSurfaceWidthMinDivisor(linear_pixel_format_and_modifier),
            ImageFormatSurfaceWidthMinDivisor(goldfish_optimal_pixel_format_and_modifier));
  EXPECT_EQ(ImageFormatSurfaceHeightMinDivisor(linear_pixel_format_and_modifier),
            ImageFormatSurfaceHeightMinDivisor(goldfish_optimal_pixel_format_and_modifier));
  EXPECT_EQ(ImageFormatSampleAlignment(linear_pixel_format_and_modifier),
            ImageFormatSampleAlignment(goldfish_optimal_pixel_format_and_modifier));
}

TEST(ImageFormat, GoldfishOptimalChecked_V1_wire) {
  constexpr uint32_t kWidth = 64;
  constexpr uint32_t kHeight = 128;
  constexpr uint32_t kStride = kWidth * 6;

  sysmem_v1::wire::ImageFormat2 linear_image_format_bgra32 = {
      .pixel_format =
          {
              .type = sysmem_v1::wire::PixelFormatType::kBgra32,
              .has_format_modifier = true,
              .format_modifier = {.value = sysmem_v1::wire::kFormatModifierLinear},
          },
      .coded_width = kWidth,
      .coded_height = kHeight,
      .bytes_per_row = kStride,
  };

  sysmem_v1::wire::ImageFormat2 goldfish_optimal_image_format_bgra32 = {
      .pixel_format =
          {
              .type = sysmem_v1::wire::PixelFormatType::kBgra32,
              .has_format_modifier = true,
              .format_modifier = {.value = sysmem_v1::wire::kFormatModifierGoogleGoldfishOptimal},
          },
      .coded_width = kWidth,
      .coded_height = kHeight,
      .bytes_per_row = kStride,
  };

  auto linear_size = ImageFormatImageSizeChecked(linear_image_format_bgra32);
  EXPECT_TRUE(linear_size.IsValid());
  auto goldfish_size = ImageFormatImageSizeChecked(goldfish_optimal_image_format_bgra32);
  EXPECT_TRUE(goldfish_size.IsValid());
  EXPECT_EQ(linear_size.ValueOrDie(), goldfish_size.ValueOrDie());

  EXPECT_EQ(ImageFormatCodedWidthMinDivisor(linear_image_format_bgra32.pixel_format),
            ImageFormatCodedWidthMinDivisor(goldfish_optimal_image_format_bgra32.pixel_format));
  EXPECT_EQ(ImageFormatCodedHeightMinDivisor(linear_image_format_bgra32.pixel_format),
            ImageFormatCodedHeightMinDivisor(goldfish_optimal_image_format_bgra32.pixel_format));
  EXPECT_EQ(ImageFormatSampleAlignment(linear_image_format_bgra32.pixel_format),
            ImageFormatSampleAlignment(goldfish_optimal_image_format_bgra32.pixel_format));
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)

TEST(ImageFormat, CorrectModifiers) {
  EXPECT_EQ(sysmem_v1::kFormatModifierArmAfbc16X16YuvTiledHeader,
            sysmem_v1::kFormatModifierArmAfbc16X16YuvTiledHeader);
  EXPECT_EQ(sysmem_v1::kFormatModifierArmAfbc16X16YuvTiledHeader,
            sysmem_v1::kFormatModifierArmAfbc16X16 | sysmem_v1::kFormatModifierArmYuvBit |
                sysmem_v1::kFormatModifierArmTiledHeaderBit);
  // V2 changes the value to be less likely to collide.
  EXPECT_NE(sysmem_v1::kFormatModifierGoogleGoldfishOptimal,
            fidl::ToUnderlying(fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal));
}

TEST(ImageFormat, CorrectModifiers_wire) {
  EXPECT_EQ(sysmem_v1::wire::kFormatModifierArmAfbc16X16YuvTiledHeader,
            sysmem_v1::wire::kFormatModifierArmAfbc16X16YuvTiledHeader);
  EXPECT_EQ(sysmem_v1::wire::kFormatModifierArmAfbc16X16YuvTiledHeader,
            sysmem_v1::wire::kFormatModifierArmAfbc16X16 |
                sysmem_v1::wire::kFormatModifierArmYuvBit |
                sysmem_v1::wire::kFormatModifierArmTiledHeaderBit);
  // V2 changes the value to be less likely to collide.
  EXPECT_NE(sysmem_v1::wire::kFormatModifierGoogleGoldfishOptimal,
            fidl::ToUnderlying(fuchsia_images2::wire::PixelFormatModifier::kGoogleGoldfishOptimal));
}

TEST(ImageFormat, RoundUpWidthForCallers) {
  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = fuchsia_images2::PixelFormat::kNv12;
  constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  constraints.min_size() = {12u, 1u};
  constraints.max_size() = {100u, 0xFFFFFFFF};

  // Later we specify a width that isn't already aligned to size_alignment.width.
  constraints.size_alignment() = {8, 1};

  // Ensure that ImageFormatMinimumRowBytes rounds up for the caller.
  uint32_t minimum_row_bytes;
  bool result = ImageFormatMinimumRowBytes(constraints, 17, &minimum_row_bytes);
  ASSERT_TRUE(result);
  // Lowest value that's 17 or larger and divisible by 8.
  EXPECT_EQ(24, minimum_row_bytes);
}

#if FUCHSIA_API_LEVEL_AT_LEAST(32)
TEST(ImageFormat, RoundUpWidthForCallersChecked) {
  sysmem_v2::ImageFormatConstraints constraints;
  constraints.pixel_format() = fuchsia_images2::PixelFormat::kNv12;
  constraints.pixel_format_modifier() = fuchsia_images2::PixelFormatModifier::kLinear;
  constraints.min_size() = {12u, 1u};
  constraints.max_size() = {100u, 0xFFFFFFFF};

  // Later we specify a width that isn't already aligned to size_alignment.width.
  constraints.size_alignment() = {8, 1};

  // Ensure that ImageFormatMinimumRowBytesChecked rounds up for the caller.
  auto minimum_row_bytes = ImageFormatMinimumRowBytesChecked(constraints, 17);
  ASSERT_TRUE(minimum_row_bytes.IsValid());
  // Lowest value that's 17 or larger and divisible by 8.
  EXPECT_EQ(24u, minimum_row_bytes.ValueOrDie());
}
#endif  // FUCHSIA_API_LEVEL_AT_LEAST(32)
