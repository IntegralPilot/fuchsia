// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/imported-image.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/bti.h>
#include <lib/zx/pmt.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <limits>
#include <utility>

#include "src/graphics/lib/virtio/virtio-abi.h"

namespace virtio_display {

// static
zx::result<ImportedImage> ImportedImage::Create(const zx::bti& bti, zx::vmo& image_vmo,
                                                uint64_t image_vmo_offset, size_t image_size,
                                                virtio_abi::ResourceFormat resource_format,
                                                uint32_t stride) {
  ZX_DEBUG_ASSERT(bti.is_valid());
  ZX_DEBUG_ASSERT(image_vmo.is_valid());
  ZX_DEBUG_ASSERT(image_vmo_offset % zx_system_get_page_size() == 0);
  ZX_DEBUG_ASSERT(image_size > 0);

  // zx_bti_pin() requires an integer number of pages.
  const size_t pinned_memory_size = ImportedImage::RoundedUpImageSize(image_size);

  zx::pmt pinned_memory_token;
  zx_paddr_t image_physical_address;
  zx_status_t pin_status = bti.pin(ZX_BTI_PERM_READ | ZX_BTI_CONTIGUOUS, image_vmo,
                                   image_vmo_offset, pinned_memory_size, &image_physical_address,
                                   /*addrs_count=*/1, &pinned_memory_token);
  if (pin_status != ZX_OK) {
    fdf::error("Failed to pin image VMO: {}", zx::make_result(pin_status));
    return zx::error(pin_status);
  }

  return zx::ok(ImportedImage(image_physical_address, std::move(pinned_memory_token),
                              virtio_abi::kInvalidResourceId, resource_format, stride));
}

// static
ImportedImage ImportedImage::CreateEmpty() { return ImportedImage(); }

ImportedImage::ImportedImage() = default;

ImportedImage::ImportedImage(zx_paddr_t physical_address, zx::pmt pinned_memory_token,
                             uint32_t virtio_resource_id,
                             virtio_abi::ResourceFormat resource_format, uint32_t stride)
    : physical_address_(physical_address),
      pinned_memory_token_(std::move(pinned_memory_token)),
      virtio_resource_id_(virtio_resource_id),
      resource_format_(resource_format),
      stride_(stride) {
  ZX_DEBUG_ASSERT(pinned_memory_token_.is_valid());
}

ImportedImage::~ImportedImage() {
  if (!pinned_memory_token_.is_valid()) {
    return;
  }
  zx_status_t unpin_status = pinned_memory_token_.unpin();
  if (unpin_status != ZX_OK) {
    fdf::warn("Failed to unpin image memory: {}", zx::make_result(unpin_status));
  }
}

// static
size_t ImportedImage::RoundedUpImageSize(size_t original_image_size) {
  const uint32_t page_size = zx_system_get_page_size();
  ZX_DEBUG_ASSERT(page_size > 0);
  ZX_DEBUG_ASSERT_MSG((page_size & (page_size - 1)) == 0, "Page size must be a power of two");

  ZX_DEBUG_ASSERT_MSG(size_t{page_size} <= std::numeric_limits<size_t>::max(),
                      "size_t can't hold a single page");
  ZX_DEBUG_ASSERT_MSG(original_image_size <= std::numeric_limits<size_t>::max() - (page_size - 1),
                      "Rounding up the image size would overflow size_t");

  const size_t page_size_mask = size_t{page_size - 1};
  ZX_DEBUG_ASSERT((original_image_size & page_size_mask) == (original_image_size % page_size));

  ZX_DEBUG_ASSERT(((original_image_size + page_size_mask) & ~page_size_mask) ==
                  ((original_image_size + page_size - 1) / page_size) * page_size);
  return (original_image_size + page_size_mask) & (~page_size_mask);
}

}  // namespace virtio_display
