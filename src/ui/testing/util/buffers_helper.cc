// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/testing/util/buffers_helper.h"

#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <functional>

namespace ui_testing {

namespace {

zx_vm_option_t HostPointerAccessModeToVmoOptions(HostPointerAccessMode host_pointer_access_mode) {
  switch (host_pointer_access_mode) {
    case HostPointerAccessMode::kReadOnly:
      return ZX_VM_PERM_READ;
    case HostPointerAccessMode::kWriteOnly:
    case HostPointerAccessMode::kReadWrite:
      return ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;
    default:
      ZX_ASSERT_MSG(false, "Invalid HostPointerAccessMode %u",
                    static_cast<unsigned int>(host_pointer_access_mode));
  }
}

}  // namespace

void MapHostPointer(const fuchsia::sysmem2::BufferCollectionInfo& collection_info, uint32_t vmo_idx,
                    HostPointerAccessMode host_pointer_access_mode,
                    std::function<void(uint8_t*, uint32_t)> callback) {
  // If the vmo idx is out of bounds pass in a nullptr and 0 bytes back to the caller.
  if (vmo_idx >= collection_info.buffers().size()) {
    callback(nullptr, 0);
    return;
  }

  auto vmo_bytes = collection_info.settings().buffer_settings().size_bytes();
  ZX_ASSERT(vmo_bytes > 0);

  MapHostPointer(collection_info.buffers()[vmo_idx].vmo(), host_pointer_access_mode, callback,
                 vmo_bytes);
}

void MapHostPointer(const fuchsia_sysmem2::BufferCollectionInfo& collection_info, uint32_t vmo_idx,
                    HostPointerAccessMode host_pointer_access_mode,
                    std::function<void(uint8_t*, uint32_t)> callback) {
  // If the vmo idx is out of bounds pass in a nullptr and 0 bytes back to the caller.
  if (!collection_info.buffers().has_value() || vmo_idx >= collection_info.buffers()->size()) {
    callback(nullptr, 0);
    return;
  }

  auto vmo_bytes = collection_info.settings()->buffer_settings()->size_bytes().value_or(0);
  ZX_ASSERT(vmo_bytes > 0);

  MapHostPointer(collection_info.buffers()->at(vmo_idx).vmo().value(), host_pointer_access_mode,
                 callback, vmo_bytes);
}

void MapHostPointer(const zx::vmo& vmo, HostPointerAccessMode host_pointer_access_mode,
                    std::function<void(uint8_t* mapped_ptr, uint32_t num_bytes)> callback,
                    uint64_t vmo_bytes) {
  if (vmo_bytes == 0) {
    auto status = vmo.get_prop_content_size(&vmo_bytes);
    // The content size is not always set, so when it's not available,
    // use the full VMO size.
    if (status != ZX_OK || vmo_bytes == 0) {
      vmo.get_size(&vmo_bytes);
    }
    ZX_ASSERT(vmo_bytes > 0);
  }

  uint8_t* vmo_host = nullptr;
  const uint32_t vmo_options = HostPointerAccessModeToVmoOptions(host_pointer_access_mode);
  auto status = zx::vmar::root_self()->map(vmo_options, /*vmar_offset*/ 0, vmo, /*vmo_offset*/ 0,
                                           vmo_bytes, reinterpret_cast<uintptr_t*>(&vmo_host));
  ZX_ASSERT(status == ZX_OK);

  if (host_pointer_access_mode == HostPointerAccessMode::kReadOnly ||
      host_pointer_access_mode == HostPointerAccessMode::kReadWrite) {
    // Flush the cache before reading back from the host VMO.
    status = zx_cache_flush(vmo_host, vmo_bytes, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
    ZX_ASSERT(status == ZX_OK);
  }

  callback(vmo_host, static_cast<uint32_t>(vmo_bytes));

  if (host_pointer_access_mode == HostPointerAccessMode::kWriteOnly ||
      host_pointer_access_mode == HostPointerAccessMode::kReadWrite) {
    // Flush the cache after writing to the host VMO.
    status = zx_cache_flush(vmo_host, vmo_bytes, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
    ZX_ASSERT(status == ZX_OK);
  }

  // Unmap the pointer.
  uintptr_t address = reinterpret_cast<uintptr_t>(vmo_host);
  status = zx::vmar::root_self()->unmap(address, vmo_bytes);
  ZX_ASSERT(status == ZX_OK);
}

}  // namespace ui_testing
