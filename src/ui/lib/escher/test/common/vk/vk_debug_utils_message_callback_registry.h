// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_TEST_COMMON_VK_VK_DEBUG_UTILS_MESSAGE_CALLBACK_REGISTRY_H_
#define SRC_UI_LIB_ESCHER_TEST_COMMON_VK_VK_DEBUG_UTILS_MESSAGE_CALLBACK_REGISTRY_H_

#include <lib/syslog/cpp/macros.h>

#include <utility>
#include <vector>

#include "src/ui/lib/escher/test/common/test_with_vk_validation_layer_macros.h"
#include "src/ui/lib/escher/vk/vulkan_instance.h"

namespace escher::test::impl {

// Registry and storage of Vulkan validation callback functions
// used in |escher::test::TestWithVkValidationLayer|.
//
// A test fixture can have an instance of |VkDebugUtilsMessengerCallbackRegistry| as its member to
// register validation debug report callbacks; they need to set up callback functions in their
// initializer, and call |RegisterDebugUtilsMessengerCallbacks()| and
// |DeregisterDebugUtilsMessengerCallbacks()| functions explicitly in their own |SetUp()| and
// |TearDown()| functions.
//
class VkDebugUtilsMessengerCallbackRegistry {
 public:
  VkDebugUtilsMessengerCallbackRegistry(
      VulkanInstancePtr instance,
      std::optional<VulkanInstance::DebugUtilsMessengerCallback> main_callback,
      std::vector<VulkanInstance::DebugUtilsMessengerCallback> optional_callbacks)
      : instance_(std::move(instance)),
        main_callback_(std::move(main_callback)),
        optional_callbacks_(std::move(optional_callbacks)) {}

  VulkanInstancePtr instance() const { return instance_; }

  void SetMainDebugUtilsMessengerCallback(VulkanInstance::DebugUtilsMessengerCallback callback) {
    FX_CHECK(!main_callback_handle_);
    main_callback_ = std::make_optional(std::move(callback));
  }
  void SetOptionalDebugUtilsMessengerCallbacks(
      std::vector<VulkanInstance::DebugUtilsMessengerCallback> callbacks) {
    FX_CHECK(optional_callback_handles_.empty());
    optional_callbacks_ = std::move(callbacks);
  }

  void RegisterDebugUtilsMessengerCallbacks();
  void DeregisterDebugUtilsMessengerCallbacks();

 private:
  VulkanInstancePtr instance_;
  std::optional<VulkanInstance::DebugUtilsMessengerCallback> main_callback_ = std::nullopt;
  std::optional<VulkanInstance::DebugUtilsMessengerCallbackHandle> main_callback_handle_ =
      std::nullopt;

  std::vector<VulkanInstance::DebugUtilsMessengerCallback> optional_callbacks_ = {};
  std::vector<VulkanInstance::DebugUtilsMessengerCallbackHandle> optional_callback_handles_ = {};
};

}  // namespace escher::test::impl

#endif  // SRC_UI_LIB_ESCHER_TEST_COMMON_VK_VK_DEBUG_UTILS_MESSAGE_CALLBACK_REGISTRY_H_
