// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/graphics/tests/common/vulkan_context.h"

std::string DisabledTestPattern() { return ""; }

namespace {

// We use a parameter here to run our tests multiple times.
class VkConnect : public ::testing::TestWithParam<int> {};

TEST_P(VkConnect, Connect) {
  std::unique_ptr ctx = VulkanContext::Builder()
                            .set_queue_flags(vk::QueueFlagBits::eCompute)
                            .set_validation_layers_enabled(false)
                            .Unique();
  ASSERT_TRUE(ctx != nullptr);
}

INSTANTIATE_TEST_SUITE_P(ConnectTests, VkConnect, testing::Range(0, 100));

}  // namespace
