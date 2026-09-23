// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_LIB_ESCHER_VK_VULKAN_INSTANCE_H_
#define SRC_UI_LIB_ESCHER_VK_VULKAN_INSTANCE_H_

#include <list>
#include <set>
#include <string>

#include "src/lib/fxl/memory/ref_counted.h"

#include <vulkan/vulkan.hpp>

namespace escher {

class VulkanInstance;
using VulkanInstancePtr = fxl::RefPtr<VulkanInstance>;

using VkDebugUtilsMessengerCallbackFn = std::function<VkBool32(
    VkDebugUtilsMessageSeverityFlagBitsEXT message_severity,
    VkDebugUtilsMessageTypeFlagsEXT message_types,
    const VkDebugUtilsMessengerCallbackDataEXT* callback_data, void* user_data)>;

// Convenient wrapper for creating and managing the lifecycle of a VkInstance
// that is suitable for use by Escher.
class VulkanInstance : public fxl::RefCountedThreadSafe<VulkanInstance> {
 public:
  // Parameters used to construct a new Vulkan Instance.
  struct Params {
    std::set<std::string> layer_names;
    std::set<std::string> extension_names;
    bool requires_surface = true;
    // These callbacks cannot be removed and must live at least as long as the VulkanInstance. They
    // can catch errors with instance initialization.
    std::list<VkDebugUtilsMessengerCallbackFn> initial_debug_utils_message_callbacks;
  };

  // Contains dynamically-obtained addresses of instance-specific functions.
  struct ProcAddrs {
    ProcAddrs(vk::Instance instance, bool requires_surface);
    ProcAddrs() = default;

    constexpr static int getVkHeaderVersion() { return VK_HEADER_VERSION; }

    PFN_vkCreateDebugUtilsMessengerEXT vkCreateDebugUtilsMessengerEXT = nullptr;
    PFN_vkDestroyDebugUtilsMessengerEXT vkDestroyDebugUtilsMessengerEXT = nullptr;
    PFN_vkGetPhysicalDeviceSurfaceSupportKHR vkGetPhysicalDeviceSurfaceSupportKHR = nullptr;
  };

  // Contains debug report callback function and the user data pointer the
  // callback function binds to.
  struct DebugUtilsMessengerCallback {
    VkDebugUtilsMessengerCallbackFn function = nullptr;
    void* user_data = nullptr;

    DebugUtilsMessengerCallback(VkDebugUtilsMessengerCallbackFn function_, void* user_data_)
        : function(std::move(function_)), user_data(user_data_) {}
  };
  using DebugUtilsMessengerList = std::list<DebugUtilsMessengerCallback>;
  using DebugUtilsMessengerCallbackHandle = DebugUtilsMessengerList::iterator;

  // Constructor.
  static fxl::RefPtr<VulkanInstance> New(Params params);

  ~VulkanInstance();

  // Get the name of Vulkan validation layer if it is supported. Otherwise
  // return std::nullopt instead.
  static std::optional<std::string> GetValidationLayerName();

  // Enumerate the available instance layers.  Return true if all required
  // layers are present, and false otherwise.
  static bool ValidateLayers(const std::set<std::string>& required_layer_names);

  // Enumerate the available instance extensions.  Return true if all required
  // extensions are present, and false otherwise.  NOTE: if an extension isn't
  // found at first, we look in all required layers to see if it is implemented
  // there.
  static bool ValidateExtensions(const std::set<std::string>& required_extension_names,
                                 const std::set<std::string>& required_layer_names);

  // Add debug report callback to registry. Registered callbacks are invoked
  // by |DebugUtilsMessengerCallbackEntrance| when validation error occurs.
  // The returned handle will be required when deregistering the callback.
  DebugUtilsMessengerCallbackHandle RegisterDebugUtilsMessengerCallback(
      VkDebugUtilsMessengerCallbackFn function, void* user_data = nullptr);

  void DeregisterDebugUtilsMessengerCallback(const DebugUtilsMessengerCallbackHandle& callback);

  vk::Instance vk_instance() const { return instance_; }

  // Return the parameterss that were used to create this instance.
  const Params& params() const { return params_; }

  // Return per-instance functions that were dynamically looked up.
  const ProcAddrs& proc_addrs() const { return proc_addrs_; }

  // Return Vulkan API Version of the instance.
  uint32_t api_version() const { return api_version_; }

 private:
  VulkanInstance(Params params, uint32_t api_version);

  vk::Result Initialize();

  // The "entrance" handler for all Vulkan instances. When validation error
  // occurs, this function will invoke all debug report callback functions
  // stored in vector |callbacks_|. This function always return VK_FALSE.
  static VkBool32 DebugUtilsMessengerCallbackEntrance(
      VkDebugUtilsMessageSeverityFlagBitsEXT message_severity,
      VkDebugUtilsMessageTypeFlagsEXT message_types,
      const VkDebugUtilsMessengerCallbackDataEXT* callback_data, void* user_data);
  constexpr static int getVkHeaderVersion() { return VK_HEADER_VERSION; }

  bool HasDebugUtilsExt() const {
    return std::find(params_.extension_names.begin(), params_.extension_names.end(),
                     VK_EXT_DEBUG_UTILS_EXTENSION_NAME) != params_.extension_names.end();
  }

  vk::Instance instance_;
  Params params_;
  ProcAddrs proc_addrs_;
  std::list<DebugUtilsMessengerCallback> callbacks_;
  vk::DebugUtilsMessengerEXT vk_callback_entrance_handle_;
  uint32_t api_version_;
};

}  // namespace escher

#endif  // SRC_UI_LIB_ESCHER_VK_VULKAN_INSTANCE_H_
