// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include "src/ui/examples/simplest_sysmem/sysmem_helper.h"

#include <fuchsia/images2/cpp/fidl.h>
#include <fuchsia/sysmem2/cpp/fidl.h>
#include <fuchsia/ui/composition/cpp/fidl.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/eventpair.h>

namespace sysmem_helper {

BufferCollectionImportExportTokens BufferCollectionImportExportTokens::New() {
  BufferCollectionImportExportTokens ref_pair;
  zx_status_t status =
      zx::eventpair::create(0, &ref_pair.export_token.value, &ref_pair.import_token.value);
  FX_CHECK(status == ZX_OK);
  return ref_pair;
}

BufferCollectionConstraints CreateDefaultConstraints(BufferConstraint buffer_constraint) {
  fuchsia::sysmem2::BufferCollectionConstraints constraints;
  constraints.mutable_buffer_memory_constraints()->set_cpu_domain_supported(true);
  constraints.mutable_buffer_memory_constraints()->set_ram_domain_supported(true);
  constraints.mutable_usage()->set_cpu(fuchsia::sysmem2::CPU_USAGE_READ_OFTEN |
                                       fuchsia::sysmem2::CPU_USAGE_WRITE_OFTEN);
  constraints.set_min_buffer_count(buffer_constraint.buffer_count);

  auto& image_constraints = constraints.mutable_image_format_constraints()->emplace_back();
  image_constraints.mutable_color_spaces()->push_back(fuchsia::images2::ColorSpace::SRGB);
  image_constraints.set_pixel_format(buffer_constraint.pixel_format_type);
  image_constraints.set_pixel_format_modifier(fuchsia::images2::PixelFormatModifier::LINEAR);

  image_constraints.set_required_min_size(
      {.width = buffer_constraint.image_width, .height = buffer_constraint.image_height});
  image_constraints.set_required_max_size(
      {.width = buffer_constraint.image_width, .height = buffer_constraint.image_height});
  image_constraints.set_bytes_per_row_divisor(buffer_constraint.bytes_per_pixel);

  return constraints;
}

void MapHostPointer(const fuchsia::sysmem2::BufferCollectionInfo& collection_info, uint32_t vmo_idx,
                    std::function<void(uint8_t*, uint64_t)> callback) {
  // If the vmo idx is out of bounds pass in a nullptr and 0 bytes back to the caller.
  if (vmo_idx >= collection_info.buffers().size()) {
    callback(nullptr, 0);
    return;
  }

  const zx::vmo& vmo = collection_info.buffers().at(vmo_idx).vmo();
  auto vmo_bytes = collection_info.settings().buffer_settings().size_bytes();
  FX_DCHECK(vmo_bytes > 0);

  uint8_t* vmo_host = nullptr;
  zx_status_t status = zx::vmar::root_self()->map(
      ZX_VM_PERM_WRITE | ZX_VM_PERM_READ, /*vmar_offset*/ 0, vmo, /*vmo_offset*/ 0, vmo_bytes,
      reinterpret_cast<uintptr_t*>(&vmo_host));
  FX_DCHECK(status == ZX_OK);

  // Flush the cache before reading back from host VMO.
  status = zx_cache_flush(vmo_host, vmo_bytes, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  FX_DCHECK(status == ZX_OK);

  callback(vmo_host, vmo_bytes);

  // Flush the cache after writing to host VMO.
  status = zx_cache_flush(vmo_host, vmo_bytes, ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  FX_DCHECK(status == ZX_OK);

  // Unmap the pointer.
  uintptr_t address = reinterpret_cast<uintptr_t>(vmo_host);
  status = zx::vmar::root_self()->unmap(address, vmo_bytes);
  FX_DCHECK(status == ZX_OK);
}
}  // namespace sysmem_helper
