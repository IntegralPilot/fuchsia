// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/boot-shim/devicetree-framebuffer.h>
#include <lib/boot-shim/testing/devicetree-test-fixture.h>
#include <zxtest/zxtest.h>

namespace {
TEST(DevicetreeFramebuffer, PaddedStrideConvertedToPixels) {
  auto dt = boot_shim::testing::LoadDtb("boot_framebuffer.dtb");
  ASSERT_TRUE(dt.is_ok());
  boot_shim::DevicetreeBootShim<boot_shim::DevicetreeFramebufferItem> shim("test", dt->fdt());
  unsigned observed = 0;
  shim.set_mmio_observer([&](const auto& range) {
    EXPECT_EQ(range.address, 0x103e1bf4000u);
    EXPECT_EQ(range.size, 512u * 60u);
    ++observed;
  });
  ASSERT_TRUE(shim.Init());
  auto info = shim.Get<boot_shim::DevicetreeFramebufferItem>().payload();
  ASSERT_TRUE(info);
  EXPECT_EQ(info->base, 0x103e1bf4000u);
  EXPECT_EQ(info->width, 120u);
  EXPECT_EQ(info->height, 60u);
  EXPECT_EQ(info->stride, 128u);
  EXPECT_EQ(info->format, ZBI_PIXEL_FORMAT_ARGB_2_10_10_10);
  EXPECT_EQ(observed, 1u);
}
TEST(DevicetreeFramebuffer, MissingDisabledAndMalformedStaySerialOnly) {
  for (const char* file : {"empty.dtb", "boot_framebuffer_disabled.dtb",
                           "boot_framebuffer_bad_stride.dtb", "boot_framebuffer_bad_format.dtb"}) {
    auto dt = boot_shim::testing::LoadDtb(file);
    ASSERT_TRUE(dt.is_ok());
    boot_shim::DevicetreeBootShim<boot_shim::DevicetreeFramebufferItem> shim("test", dt->fdt());
    shim.set_mmio_observer([](const auto&) { FAIL("Invalid framebuffer observed"); });
    ASSERT_TRUE(shim.Init());
    EXPECT_FALSE(shim.Get<boot_shim::DevicetreeFramebufferItem>().payload());
  }
}
}  // namespace
