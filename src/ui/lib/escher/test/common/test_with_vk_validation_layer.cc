// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/test/common/test_with_vk_validation_layer.h"

#include "src/ui/lib/escher/test/common/gtest_escher.h"
#include "src/ui/lib/escher/test/common/gtest_vulkan.h"
#include "src/ui/lib/escher/test/common/vk/vk_debug_utils_message_callback_registry.h"
#include "src/ui/lib/escher/vk/vulkan_instance.h"

namespace escher::test {

TestWithVkValidationLayer::TestWithVkValidationLayer(
    std::vector<VulkanInstance::DebugUtilsMessengerCallback> optional_callbacks)
    : vk_debug_utils_message_collector_() {
  fxl::RefPtr<VulkanInstance> instance = nullptr;
  if (!VK_TESTS_SUPPRESSED()) {
    instance = EscherEnvironment::GetGlobalTestEnvironment()->GetVulkanInstance();
  }
  VulkanInstance::DebugUtilsMessengerCallback callback(
      impl::VkDebugUtilsMessageCollector::HandleDebugUtilsMessage,
      &vk_debug_utils_message_collector_);
  vk_debug_utils_message_callback_registry_ =
      std::make_unique<impl::VkDebugUtilsMessengerCallbackRegistry>(
          std::move(instance), std::move(callback), std::move(optional_callbacks));
}

void TestWithVkValidationLayer::SetUp() {
  vk_debug_utils_message_callback_registry().RegisterDebugUtilsMessengerCallbacks();
}

void TestWithVkValidationLayer::TearDown() {
  EXPECT_NO_VULKAN_VALIDATION_ERRORS();
  EXPECT_NO_VULKAN_VALIDATION_WARNINGS();
  if (vk_debug_utils_message_collector().PrintDebugUtilsMessagesWithFlags(
          vk::DebugUtilsMessageSeverityFlagBitsEXT::eWarning,
          vk::DebugUtilsMessageTypeFlagBitsEXT::ePerformance, __FILE__, __LINE__)) {
    FX_LOGS(WARNING) << "Performance warning occurred in test, see above for details.";
  }
  vk_debug_utils_message_callback_registry().DeregisterDebugUtilsMessengerCallbacks();
}

}  // namespace escher::test
