// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_EXAMPLES_VKPROTO_COMMON_PHYSICAL_DEVICE_H_
#define SRC_GRAPHICS_EXAMPLES_VKPROTO_COMMON_PHYSICAL_DEVICE_H_

#include <memory>
#include <optional>
#include <vector>

#include "src/graphics/examples/vkproto/common/instance.h"
#include "src/graphics/examples/vkproto/common/surface.h"

#include <vulkan/vulkan.hpp>

namespace vkp {

std::optional<uint32_t> GetGpuVendorId();

class PhysicalDeviceProperties {
 public:
  PhysicalDeviceProperties() {}
  PhysicalDeviceProperties(uint32_t api_version, uint32_t device_id,
                           vk::PhysicalDeviceType device_type);
  void set_api_version(uint32_t api_version) { api_version_ = api_version; }
  void set_device_id(uint32_t device_id) { device_id_ = device_id; }
  void set_device_type(vk::PhysicalDeviceType device_type) { device_type_ = device_type; }

  std::optional<uint32_t> api_version_;
  std::optional<uint32_t> device_id_;
  std::optional<vk::PhysicalDeviceType> device_type_;
};

class PhysicalDevice {
 public:
  explicit PhysicalDevice(std::shared_ptr<vk::Instance> instance,
                          const VkSurfaceKHR &surface = nullptr,
                          std::optional<PhysicalDeviceProperties> properties = std::nullopt,
                          const vk::QueueFlags &queue_flags = vk::QueueFlagBits::eGraphics);

  bool Init();
  const vk::PhysicalDevice &get() const;

  void set_swapchain_enabled(bool enabled) { swapchain_enabled_ = enabled; }

  static void AppendRequiredPhysDeviceExts(std::vector<const char *> *exts, bool swapchain_enabled);

 private:
  PhysicalDevice() = delete;
  VKP_DISALLOW_COPY_AND_ASSIGN(PhysicalDevice);

  bool swapchain_enabled_{true};
  bool initialized_;
  std::shared_ptr<vk::Instance> instance_;
  VkSurfaceKHR surface_;
  std::optional<PhysicalDeviceProperties> properties_;
  vk::QueueFlags queue_flags_;

  vk::PhysicalDevice physical_device_;
};

}  // namespace vkp

#endif  // SRC_GRAPHICS_EXAMPLES_VKPROTO_COMMON_PHYSICAL_DEVICE_H_
