// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "src/graphics/examples/vkproto/common/device.h"
#include "src/graphics/examples/vkproto/common/instance.h"
#include "src/graphics/examples/vkproto/common/physical_device.h"

// Initialize Vulkan along with implicit teardown.
TEST(VkProto, VulkanInitOnly) {
  // INSTANCE
  const bool kEnableValidation = true;
  vkp::Instance vkp_instance(vkp::Instance::Builder()
                                 .set_validation_layers_enabled(kEnableValidation)
                                 .set_swapchain_enabled(false)
                                 .Build());
  ASSERT_TRUE(vkp_instance.initialized()) << "Instance Initialization Failed.\n";

  // PHYSICAL DEVICE
  vkp::PhysicalDevice vkp_physical_device(vkp_instance.shared());
  vkp_physical_device.set_swapchain_enabled(false);
  ASSERT_TRUE(vkp_physical_device.Init()) << "Physical device initialization failed\n";

  // LOGICAL DEVICE
  vkp::Device vkp_device(vkp_physical_device.get());
  vkp_device.set_swapchain_enabled(false);
  EXPECT_TRUE(vkp_device.Init()) << "Logical device initialization failed\n";
}
