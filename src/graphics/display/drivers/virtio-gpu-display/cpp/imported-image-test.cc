// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/imported-image.h"

#include <zircon/syscalls.h>

#include <cstdint>

#include <gtest/gtest.h>

namespace virtio_display {

namespace {

class ImportedImageTest : public ::testing::Test {
 public:
  ImportedImageTest() : page_size_(zx_system_get_page_size()) {}
  ~ImportedImageTest() override = default;

 protected:
  const uint32_t page_size_;
};

TEST_F(ImportedImageTest, RoundUpPageSizeNoRounding) {
  EXPECT_EQ(page_size_, ImportedImage::RoundedUpImageSize(page_size_));
  EXPECT_EQ(2 * page_size_, ImportedImage::RoundedUpImageSize(2 * page_size_));
  EXPECT_EQ(ZX_MAX_PAGE_SIZE, ImportedImage::RoundedUpImageSize(ZX_MAX_PAGE_SIZE));
}

TEST_F(ImportedImageTest, RoundUpPageSizeOffByOne) {
  EXPECT_EQ(page_size_, ImportedImage::RoundedUpImageSize(1));
  EXPECT_EQ(2 * page_size_, ImportedImage::RoundedUpImageSize(page_size_ + 1));
  EXPECT_EQ(ZX_MAX_PAGE_SIZE + page_size_, ImportedImage::RoundedUpImageSize(ZX_MAX_PAGE_SIZE + 1));
}

TEST_F(ImportedImageTest, RoundUpPageSizeOffByAlmostOnePage) {
  EXPECT_EQ(page_size_, ImportedImage::RoundedUpImageSize(page_size_ - 1));
  EXPECT_EQ(2 * page_size_, ImportedImage::RoundedUpImageSize((2 * page_size_) - 1));
  EXPECT_EQ(ZX_MAX_PAGE_SIZE, ImportedImage::RoundedUpImageSize(ZX_MAX_PAGE_SIZE - 1));
}

}  // namespace

}  // namespace virtio_display
