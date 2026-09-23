// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/flatland/image_formats.h"

namespace flatland {

namespace {

// This list includes some exotic formats based on product needs - for example, to prevent nasty
// gralloc errors in system logs. At this time there is sufficient test coverage to ensure these
// formats are supported on all target platforms; however it's unclear how we would handle a
// platform that does not support one or more formats.
const std::vector<vk::Format> kSupportedClientImageFormats = {
    vk::Format::eR8G8B8A8Srgb,           vk::Format::eB8G8R8A8Srgb,
    vk::Format::eA2B10G10R10UnormPack32, vk::Format::eR8Unorm,
    vk::Format::eG8B8R83Plane420Unorm,   vk::Format::eR5G6B5UnormPack16,
    vk::Format::eG8B8R82Plane420Unorm};

const std::vector<vk::Format> kSupportedClientYuvImageFormats = {vk::Format::eG8B8R83Plane420Unorm,
                                                                 vk::Format::eG8B8R82Plane420Unorm};

}  // namespace

const std::vector<vk::Format>& SupportedClientImageFormats() {
  return kSupportedClientImageFormats;
}

const std::vector<vk::Format>& SupportedClientYuvImageFormats() {
  return kSupportedClientYuvImageFormats;
}

}  // namespace flatland
