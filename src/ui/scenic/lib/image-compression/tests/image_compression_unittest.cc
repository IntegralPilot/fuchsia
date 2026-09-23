// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/image-compression/image_compression.h"

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.ui.compression.internal/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>
#include <png.h>

#include <iostream>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/ui/testing/util/screenshot_helper.h"

using image_compression::ImageCompression;
using ui_testing::Screenshot;

namespace {

// Needed for |png_set_read_fn| so that libpng can read from a zx::vmo in a stream-like fashion.
struct libpng_vmo {
  zx::vmo* vmo;
  size_t offset;
};

constexpr size_t kBytesPerPixel = 4;

}  // namespace

class ImageCompressionTest : public gtest::TestLoopFixture {
 protected:
  void SetUp() override {
    image_compression_ = std::make_unique<ImageCompression>();
    auto [client_end, server_end] =
        fidl::Endpoints<fuchsia_ui_compression_internal::ImageCompressor>::Create();
    image_compression_->Connect(std::move(server_end), dispatcher());
    client_.Bind(std::move(client_end), dispatcher());

    // Handle 4k tests.
    if (is_4k_) {
      size_.width(3840);
      size_.height(2160);
      bytes_to_write_ = size_.width() * size_.height() * kBytesPerPixel;
    }

    // Define in_vmo.
    zx_status_t status = zx::vmo::create(bytes_to_write_, 0, &in_vmo_);
    EXPECT_EQ(status, ZX_OK);
    EXPECT_EQ(in_vmo_.set_prop_content_size(bytes_to_write_), ZX_OK);

    // Define out_vmo.
    status =
        zx::vmo::create(bytes_to_write_ + zx_system_get_page_size(), ZX_VMO_RESIZABLE, &out_vmo_);
    EXPECT_EQ(status, ZX_OK);

    // Duplicate in_ and out_ VMO.
    EXPECT_EQ(in_vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &in_vmo_copy_), ZX_OK);
    EXPECT_EQ(out_vmo_.duplicate(ZX_RIGHT_SAME_RIGHTS, &out_vmo_copy_), ZX_OK);
  }

  fidl::Client<fuchsia_ui_compression_internal::ImageCompressor> client_;
  std::unique_ptr<ImageCompression> image_compression_;

  // By default, assume 1080p BGRA sized buffers.
  fuchsia_math::SizeU size_{{.width = 1920, .height = 1080}};
  size_t bytes_to_write_ = size_.width() * size_.height() * kBytesPerPixel;

  zx::vmo in_vmo_;
  zx::vmo out_vmo_;

  // Copies of VMO to pass into function.
  zx::vmo in_vmo_copy_;
  zx::vmo out_vmo_copy_;

  bool is_4k_ = false;
};

class ParameterizedImageCompressionTest : public ImageCompressionTest,
                                          public ::testing::WithParamInterface<bool> {
 protected:
  void SetUp() override {
    is_4k_ = GetParam();
    ImageCompressionTest::SetUp();
  }
};

INSTANTIATE_TEST_SUITE_P(UseFlatland, ParameterizedImageCompressionTest, ::testing::Bool());

TEST_P(ParameterizedImageCompressionTest, ValidImage) {
  size_t num_pixels = size_.width() * size_.height();

  std::vector<uint8_t> pixels;
  for (size_t i = 0; i < num_pixels; ++i) {
    uint8_t inc = static_cast<uint8_t>(i);
    pixels.push_back(15 + inc);
    pixels.push_back(112 + inc);
    pixels.push_back(122 + inc);
    pixels.push_back(251 + inc);
  }

  EXPECT_EQ(in_vmo_.write(pixels.data(), 0, bytes_to_write_), ZX_OK);

  fuchsia_ui_compression_internal::ImageCompressorEncodePngRequest request;
  request.raw_vmo(std::move(in_vmo_copy_));
  request.image_dimensions(size_);
  request.png_vmo(std::move(out_vmo_copy_));

  bool flag = false;
  client_->EncodePng(std::move(request))
      .ThenExactlyOnce(
          [&flag](
              fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>& result) {
            EXPECT_TRUE(result.is_ok());
            flag = true;
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(flag);

  // Compare the original Screenshot to the converted Screenshot.
  const auto& original = Screenshot(in_vmo_, size_.width(), size_.height(), 0);
  const auto& converted = Screenshot(out_vmo_);
  EXPECT_GE(original.ComputeSimilarity(converted), 100.f);

  // Expect some compression to have occurred.
  size_t after_size;
  out_vmo_.get_size(&after_size);
  EXPECT_LT(after_size, bytes_to_write_ + zx_system_get_page_size());
}

TEST_F(ImageCompressionTest, MissingArgs) {
  // Only include in vmo.
  fuchsia_ui_compression_internal::ImageCompressorEncodePngRequest request;
  request.raw_vmo(std::move(in_vmo_copy_));

  bool flag = false;
  client_->EncodePng(std::move(request))
      .ThenExactlyOnce(
          [&flag](
              fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>& result) {
            ASSERT_TRUE(result.is_error());
            ASSERT_TRUE(result.error_value().is_domain_error());
            EXPECT_EQ(result.error_value().domain_error(),
                      fuchsia_ui_compression_internal::ImageCompressionError::kMissingArgs);
            flag = true;
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(flag);

  // Only include size.
  request = {};
  request.image_dimensions(size_);
  flag = false;

  client_->EncodePng(std::move(request))
      .ThenExactlyOnce(
          [&flag](
              fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>& result) {
            ASSERT_TRUE(result.error_value().is_domain_error());
            EXPECT_EQ(result.error_value().domain_error(),
                      fuchsia_ui_compression_internal::ImageCompressionError::kMissingArgs);
            flag = true;
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(flag);

  // Only include out vmo.
  request = {};
  request.png_vmo(std::move(out_vmo_copy_));
  flag = false;

  client_->EncodePng(std::move(request))
      .ThenExactlyOnce(
          [&flag](
              fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>& result) {
            ASSERT_TRUE(result.error_value().is_domain_error());
            EXPECT_EQ(result.error_value().domain_error(),
                      fuchsia_ui_compression_internal::ImageCompressionError::kMissingArgs);
            flag = true;
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(flag);
}

TEST_F(ImageCompressionTest, EmptyInVmo) {
  // Define empty in_vmo.
  zx::vmo in_vmo;
  zx_status_t status = zx::vmo::create(0, 0, &in_vmo);
  EXPECT_EQ(status, ZX_OK);

  fuchsia_ui_compression_internal::ImageCompressorEncodePngRequest request;
  request.raw_vmo(std::move(in_vmo));
  request.image_dimensions(size_);
  request.png_vmo(std::move(out_vmo_copy_));

  bool flag = false;
  client_->EncodePng(std::move(request))
      .ThenExactlyOnce(
          [&flag](
              fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>& result) {
            ASSERT_TRUE(result.is_error());
            ASSERT_TRUE(result.error_value().is_domain_error());
            EXPECT_EQ(result.error_value().domain_error(),
                      fuchsia_ui_compression_internal::ImageCompressionError::kInvalidArgs);
            flag = true;
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(flag);
}

TEST_F(ImageCompressionTest, OutVmoTooSmall) {
  // Define small out_vmo, by 1 page size.
  zx::vmo small_out_vmo;
  zx_status_t status = zx::vmo::create(bytes_to_write_, 0, &small_out_vmo);
  EXPECT_EQ(status, ZX_OK);

  fuchsia_ui_compression_internal::ImageCompressorEncodePngRequest request;
  request.raw_vmo(std::move(in_vmo_copy_));
  request.image_dimensions(size_);
  request.png_vmo(std::move(small_out_vmo));

  bool flag = false;
  client_->EncodePng(std::move(request))
      .ThenExactlyOnce(
          [&flag](
              fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>& result) {
            ASSERT_TRUE(result.is_error());
            ASSERT_TRUE(result.error_value().is_domain_error());
            EXPECT_EQ(result.error_value().domain_error(),
                      fuchsia_ui_compression_internal::ImageCompressionError::kInvalidArgs);
            flag = true;
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(flag);
}

// Try to compress a BGRA image with a stated width and height that is larger than the VMO size.
TEST_F(ImageCompressionTest, VmoSizeIncompatibleWithWidthAndHeight) {
  // Define small in_vmo, by 1 page size.
  zx::vmo small_in_vmo;
  zx_status_t status =
      zx::vmo::create(bytes_to_write_ - zx_system_get_page_size(), 0, &small_in_vmo);
  EXPECT_EQ(status, ZX_OK);

  fuchsia_ui_compression_internal::ImageCompressorEncodePngRequest request;
  request.raw_vmo(std::move(small_in_vmo));
  request.image_dimensions(size_);
  request.png_vmo(std::move(out_vmo_copy_));

  bool flag = false;
  client_->EncodePng(std::move(request))
      .ThenExactlyOnce(
          [&flag](
              fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>& result) {
            ASSERT_TRUE(result.is_error());
            ASSERT_TRUE(result.error_value().is_domain_error());
            EXPECT_EQ(result.error_value().domain_error(),
                      fuchsia_ui_compression_internal::ImageCompressionError::kInvalidArgs);
            flag = true;
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(flag);
}

// Test that EncodePng succeeds when in_vmo is larger than the raw image size (e.g. sysmem padding).
TEST_F(ImageCompressionTest, InVmoLargerThanRawImage) {
  // Define large in_vmo with extra sysmem padding.
  zx::vmo padded_in_vmo, padded_in_vmo_copy;
  zx_status_t status =
      zx::vmo::create(bytes_to_write_ + 2 * zx_system_get_page_size(), 0, &padded_in_vmo);
  EXPECT_EQ(status, ZX_OK);
  EXPECT_EQ(padded_in_vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &padded_in_vmo_copy), ZX_OK);

  fuchsia_ui_compression_internal::ImageCompressorEncodePngRequest request;
  request.raw_vmo(std::move(padded_in_vmo_copy));
  request.image_dimensions(size_);
  request.png_vmo(std::move(out_vmo_copy_));

  bool flag = false;
  client_->EncodePng(std::move(request))
      .ThenExactlyOnce(
          [&flag](
              fidl::Result<fuchsia_ui_compression_internal::ImageCompressor::EncodePng>& result) {
            EXPECT_TRUE(result.is_ok());
            flag = true;
          });

  RunLoopUntilIdle();
  EXPECT_TRUE(flag);
}
