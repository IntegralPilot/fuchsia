// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "config_query.h"

#include <cstdlib>

#include "src/graphics/tests/common/vulkan_context.h"
#include "src/graphics/tests/vkext/config.h"

config::Config& GetConfig() {
  static auto config = config::Config::TakeFromStartupHandle();
  return config;
}

std::optional<uint32_t> GetGpuVendorId() {
  auto& c = GetConfig();
  uint32_t vendor_id_int = c.gpu_vendor_id();
  if (vendor_id_int != 0) {
    return std::optional<uint32_t>{vendor_id_int};
  }

  return std::optional<uint32_t>{};
}

bool SupportsSysmemA2B10G10R10() {
  auto& c = GetConfig();
  return c.support_sysmem_a2b10g10r10();
}

bool SupportsSysmemYuv() {
  auto& c = GetConfig();
  return c.support_sysmem_yuv();
}

bool SupportsSysmemYv12() {
  auto& c = GetConfig();
  return c.support_sysmem_yv12();
}

bool SupportsSysmemRenderableLinear() { return GetConfig().support_sysmem_renderable_linear(); }

bool SupportsSysmemLinearNonRgba() { return GetConfig().support_sysmem_linear_nonrgba(); }

bool SupportsProtectedMemory() { return GetConfig().support_protected_memory(); }

std::string DisabledTestPattern() { return GetConfig().disabled_test_pattern(); }
