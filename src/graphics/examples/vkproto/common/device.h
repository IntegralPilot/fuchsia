// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_EXAMPLES_VKPROTO_COMMON_DEVICE_H_
#define SRC_GRAPHICS_EXAMPLES_VKPROTO_COMMON_DEVICE_H_

#include <memory>
#include <vector>

#include "src/graphics/examples/vkproto/common/utils.h"

#include <vulkan/vulkan.hpp>

namespace vkp {

class Device {
 public:
  explicit Device(const vk::PhysicalDevice &physical_device, const VkSurfaceKHR &surface = nullptr,
                  vk::QueueFlags queue_flags = vk::QueueFlagBits::eGraphics);
  ~Device();

  void set_swapchain_enabled(bool enabled) { swapchain_enabled_ = enabled; }
  bool Init();
  std::shared_ptr<vk::Device> shared();
  const vk::Device &get() const;
  vk::Queue queue() const;
  uint32_t queue_family_index() const { return queue_family_index_; }
  bool initialized() const { return initialized_; }

 private:
  VKP_DISALLOW_COPY_AND_ASSIGN(Device);

  bool swapchain_enabled_{true};
  bool initialized_;
  vk::PhysicalDevice physical_device_;
  VkSurfaceKHR surface_;
  std::vector<const char *> layers_;

  vk::Queue queue_;
  uint32_t queue_family_index_{};
  vk::QueueFlags queue_flags_;

  std::shared_ptr<vk::Device> device_;
};

}  // namespace vkp

#endif  // SRC_GRAPHICS_EXAMPLES_VKPROTO_COMMON_DEVICE_H_
