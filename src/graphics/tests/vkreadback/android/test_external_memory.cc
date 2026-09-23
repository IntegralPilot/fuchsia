// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <android/hardware_buffer.h>
#include <gtest/gtest.h>

#include "vkreadback.h"

TEST(VkReadbackExternal, Android) {
  AHardwareBuffer* ahb = nullptr;
  AHardwareBuffer_Desc ahb_desc = {
      .width = VkReadbackTest::kWidth,
      .height = VkReadbackTest::kHeight,
      .layers = 1,
      .format = AHARDWAREBUFFER_FORMAT_R8G8B8A8_UNORM,
      .usage = AHARDWAREBUFFER_USAGE_GPU_COLOR_OUTPUT | AHARDWAREBUFFER_USAGE_CPU_READ_RARELY,
      .stride = 0,  // ignored for allocate
      .rfu0 = 0,    // reserved
      .rfu1 = 0,    // reserved
  };

  ASSERT_EQ(0, AHardwareBuffer_allocate(&ahb_desc, &ahb));

  VkReadbackTest test(ahb);

  ASSERT_TRUE(test.Initialize(VK_API_VERSION_1_1));
  ASSERT_TRUE(test.Exec());
  ASSERT_TRUE(test.Readback());

  AHardwareBuffer_release(ahb);
}
