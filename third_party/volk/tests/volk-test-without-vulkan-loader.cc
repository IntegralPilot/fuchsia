// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <volk.h>

#include <gtest/gtest.h>

TEST(VolkTestWithoutVulkanLoader, InitializeFailure) {
  VkResult result = volkInitialize();
  EXPECT_NE(result, VK_SUCCESS);
}
