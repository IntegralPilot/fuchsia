// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_FLATLAND_IMAGE_FORMATS_H_
#define SRC_UI_SCENIC_LIB_FLATLAND_IMAGE_FORMATS_H_

#include <vector>

#include <vulkan/vulkan.hpp>

namespace flatland {

// The full list of supported vk::Formats for client images. These are used in sysmem negotiations.
const std::vector<vk::Format>& SupportedClientImageFormats();

// Subset of the formats from SupportedClientImageFormats(), containing only the YUV formats.
const std::vector<vk::Format>& SupportedClientYuvImageFormats();

// The list of supported vk::Formats for images that are used as render targets or readback targets
// for screenshots depend on the implementation details of the renderer. They are defined internally
// under renderer implementations, i.e. VkRenderer, NullRenderer.

}  // namespace flatland

#endif  // SRC_UI_SCENIC_LIB_FLATLAND_IMAGE_FORMATS_H_
