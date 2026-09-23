// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "glfw_surface.h"

#include "src/graphics/examples/vkproto/common/utils.h"

namespace vkp {

GlfwSurface::GlfwSurface(std::shared_ptr<vk::Instance> instance, GLFWwindow *window)
    : Surface(instance), window_(window) {}

GlfwSurface::~GlfwSurface() {
  if (initialized_) {
    vkDestroySurfaceKHR(*instance_, surface_, nullptr);
  }
}

bool GlfwSurface::Init() {
  if (initialized_) {
    RTN_MSG(false, "vkp::GlfwSurface::Init() failed - already initialized.\n");
  }
  if (!instance_.get()) {
    RTN_MSG(false, "vkp::GlfwSurface::Init() failed - must provide instance and window.\n");
  }
  auto rv = glfwCreateWindowSurface(*instance_, window_, nullptr, &surface_);

  if (rv != VK_SUCCESS) {
    RTN_MSG(false, "GLFW surface creation failed.\n");
  }

  initialized_ = true;
  return true;
}

}  // namespace vkp
