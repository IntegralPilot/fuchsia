// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_EXAMPLES_VKPROTO_COMMON_SURFACE_H_
#define SRC_GRAPHICS_EXAMPLES_VKPROTO_COMMON_SURFACE_H_

#include <memory>

#include "src/graphics/examples/vkproto/common/utils.h"

#include <vulkan/vulkan.hpp>

namespace vkp {

class Surface {
 public:
  explicit Surface(std::shared_ptr<vk::Instance> instance) : instance_(std::move(instance)) {}

  virtual bool Init() = 0;
  const VkSurfaceKHR &get() const { return surface_; }

 protected:
  bool initialized_ = false;
  std::shared_ptr<vk::Instance> instance_;
  VkSurfaceKHR surface_;

 private:
  VKP_DISALLOW_COPY_AND_ASSIGN(Surface);
};

}  // namespace vkp

#endif  // SRC_GRAPHICS_EXAMPLES_VKPROTO_COMMON_SURFACE_H_
