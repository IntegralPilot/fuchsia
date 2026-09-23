// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/framebuffer-display/boot-framebuffer.h"
#include <zxtest/zxtest.h>

namespace framebuffer_display {
namespace {
TEST(BootFramebuffer, PaddedLayoutAndRangeBounds) {
  zbi_swfb_t info{.base = 0x10000000000, .width = 3, .height = 2,
                 .stride = 4, .format = ZBI_PIXEL_FORMAT_ARGB_2_10_10_10};
  ASSERT_TRUE(BootFramebufferSize(info));
  EXPECT_EQ(*BootFramebufferSize(info), 32u);
  info.stride = 2;
  EXPECT_FALSE(BootFramebufferSize(info));
  info.stride = 4;
  info.height = 0;
  EXPECT_FALSE(BootFramebufferSize(info));
  info.height = 2;
  info.base = UINT64_MAX - 16;
  EXPECT_FALSE(BootFramebufferSize(info));
  info.base = 0x10000000000;
  info.height = INT32_MAX;
  EXPECT_FALSE(BootFramebufferSize(info));
  info.height = 2;
  info.format = ZBI_PIXEL_FORMAT_NV12;
  EXPECT_FALSE(BootFramebufferSize(info));
}
TEST(BootFramebuffer, OpaquePackedPrimaryColours) {
  constexpr auto format = ZBI_PIXEL_FORMAT_ARGB_2_10_10_10;
  EXPECT_EQ(PackBgra8888(0, format), 0xc0000000u);
  EXPECT_EQ(PackBgra8888(0xffffff, format), 0xffffffffu);
  EXPECT_EQ(PackBgra8888(0xff0000, format), 0xfff00000u);
  EXPECT_EQ(PackBgra8888(0x00ff00, format), 0xc00ffc00u);
  EXPECT_EQ(PackBgra8888(0x0000ff, format), 0xc00003ffu);
  EXPECT_EQ(PackBgra8888(0xff0000, ZBI_PIXEL_FORMAT_ABGR_2_10_10_10), 0xc00003ffu);
  EXPECT_EQ(PackBgra8888(0x00123456, ZBI_PIXEL_FORMAT_ARGB_8888), 0xff123456u);
  EXPECT_EQ(PackBgra8888(0x00123456, ZBI_PIXEL_FORMAT_ABGR_8888), 0xff563412u);
}
TEST(BootFramebuffer, TenBitRampIsMonotonicAndPreservesEightBits) {
  uint32_t previous = 0;
  for (uint32_t c = 0; c < 256; ++c) {
    const uint32_t pixel = PackBgra8888(c, ZBI_PIXEL_FORMAT_ARGB_2_10_10_10);
    const uint32_t blue = pixel & 1023;
    EXPECT_EQ(blue >> 2, c);
    EXPECT_GE(blue, previous);
    previous = blue;
  }
}
}  // namespace
}  // namespace framebuffer_display
