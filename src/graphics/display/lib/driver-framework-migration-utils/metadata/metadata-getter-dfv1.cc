// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/driver-framework-migration-utils/metadata/metadata-getter-dfv1.h"

#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/zx/result.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <cinttypes>
#include <cstdint>
#include <string_view>
#include <vector>

#include <fbl/alloc_checker.h>

namespace display {

// static
zx::result<std::unique_ptr<MetadataGetter>> MetadataGetterDfv1::Create(zx_device_t* device) {
  ZX_DEBUG_ASSERT(device != nullptr);

  fbl::AllocChecker alloc_checker;
  auto incoming = fbl::make_unique_checked<MetadataGetterDfv1>(&alloc_checker, device);
  if (!alloc_checker.check()) {
    zxlogf(ERROR, "Failed to allocate memory for MetadataGetterDfv1.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(std::move(incoming));
}

MetadataGetterDfv1::MetadataGetterDfv1(zx_device_t* device) : device_(device) {
  ZX_DEBUG_ASSERT(device != nullptr);
}

zx::result<std::vector<uint8_t>> MetadataGetterDfv1::GetBytes(uint32_t type,
                                                              std::string_view instance) {
  size_t metadata_size_bytes = 0;
  zx_status_t status = device_get_metadata(device_, type, nullptr, 0, &metadata_size_bytes);
  if (status != ZX_ERR_BUFFER_TOO_SMALL) {
    zxlogf(ERROR, "Failed to get metadata size of type %" PRIu32 ": %s", type,
           zx_status_get_string(status));
    return zx::error(status);
  }

  std::vector<uint8_t> bytes(metadata_size_bytes, 0);
  size_t actual_byte_count = 0;
  status = device_get_metadata(device_, type, bytes.data(), bytes.size(), &actual_byte_count);
  if (status != ZX_OK) {
    zxlogf(ERROR, "Failed to get metadata of type %" PRIu32 ": %s", type,
           zx_status_get_string(status));
    return zx::error(status);
  }
  if (actual_byte_count != metadata_size_bytes) {
    zxlogf(ERROR, "Got wrong size of metadata: expected %zu bytes, actual %zu bytes",
           metadata_size_bytes, actual_byte_count);
    return zx::error(ZX_ERR_INTERNAL);
  }

  return zx::ok(std::move(bytes));
}

}  // namespace display
