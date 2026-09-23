// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "fuchsia_surface.h"

#include "src/graphics/examples/vkproto/common/utils.h"

namespace vkp {

FuchsiaSurface::FuchsiaSurface(std::shared_ptr<vk::Instance> instance) : Surface(instance) {}

FuchsiaSurface::~FuchsiaSurface() {
  if (initialized_) {
    vkDestroySurfaceKHR(*instance_, surface_, nullptr);
  }
}

bool FuchsiaSurface::Init() {
  if (initialized_) {
    RTN_MSG(false, "vkp::FuchsiaSurface::Init() failed - already initialized.\n");
  }
  if (!instance_.get()) {
    RTN_MSG(false, "vkp::FuchsiaSurface::Init() failed - must provide instance.\n");
  }

  // TODO(https://fxbug.dev/42082627): Move to scenic (public) surface.
  VkImagePipeSurfaceCreateInfoFUCHSIA info = {
      .sType = VK_STRUCTURE_TYPE_IMAGEPIPE_SURFACE_CREATE_INFO_FUCHSIA,
      .pNext = nullptr,
  };

  auto rv = vkCreateImagePipeSurfaceFUCHSIA(*instance_, &info, nullptr, &surface_);

  if (rv != VK_SUCCESS) {
    RTN_MSG(false, "FuchsiaSurface creation failed.\n");
  }

  initialized_ = true;
  return true;
}

}  // namespace vkp
