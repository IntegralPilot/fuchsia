// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <cstdlib>

#include "src/graphics/tests/common/config.h"
#include "vulkan_context.h"

namespace {

config::Config& GetConfig() {
  static auto config = config::Config::TakeFromStartupHandle();
  return config;
}
}  // namespace

std::optional<uint32_t> GetGpuVendorId() {
  auto& c = GetConfig();
  uint32_t vendor_id_int = c.gpu_vendor_id();
  if (vendor_id_int != 0) {
    return std::optional<uint32_t>{vendor_id_int};
  }

  return std::optional<uint32_t>{};
}

std::string DisabledTestPattern() { return GetConfig().disabled_test_pattern(); }
