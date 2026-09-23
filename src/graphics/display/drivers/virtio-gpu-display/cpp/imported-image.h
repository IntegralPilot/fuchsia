// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_IMPORTED_IMAGE_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_IMPORTED_IMAGE_H_

#include <lib/zx/bti.h>
#include <lib/zx/pmt.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <utility>

#include "src/graphics/lib/virtio/virtio-abi.h"

namespace virtio_display {

// State associated with an image in an imported sysmem buffer.
class ImportedImage {
 public:
  // Creates an instance without an associated virtio resource ID.
  //
  // `bti` must be valid for the duration of the call. `image_vmo` must point to
  // a valid VMO whose size is at least image_vmo_offset + image_size.
  static zx::result<ImportedImage> Create(const zx::bti& bti, zx::vmo& image_vmo,
                                          uint64_t image_vmo_offset, size_t image_size,
                                          virtio_abi::ResourceFormat resource_format,
                                          uint32_t stride);

  // Creates an instance representing the image moved-out state.
  static ImportedImage CreateEmpty();

  // Exposed for testing. Production code must use Create*() factory methods.
  explicit ImportedImage(zx_paddr_t physical_address, zx::pmt pinned_memory_token,
                         uint32_t virtio_resource_id, virtio_abi::ResourceFormat resource_format,
                         uint32_t stride);

  ImportedImage(const ImportedImage&) = delete;
  ImportedImage& operator=(const ImportedImage&) = delete;

  ImportedImage(ImportedImage&& rhs) noexcept
      : physical_address_(rhs.physical_address_),
        pinned_memory_token_(std::move(rhs.pinned_memory_token_)),
        virtio_resource_id_(rhs.virtio_resource_id_),
        resource_format_(rhs.resource_format_),
        stride_(rhs.stride_) {
    rhs.physical_address_ = 0;
    rhs.virtio_resource_id_ = virtio_abi::kInvalidResourceId;
    rhs.resource_format_ = {};
    rhs.stride_ = 0;
  }

  ImportedImage& operator=(ImportedImage&& rhs) noexcept {
    physical_address_ = rhs.physical_address_;
    rhs.physical_address_ = 0;
    pinned_memory_token_ = std::move(rhs.pinned_memory_token_);
    virtio_resource_id_ = rhs.virtio_resource_id_;
    rhs.virtio_resource_id_ = virtio_abi::kInvalidResourceId;
    resource_format_ = rhs.resource_format_;
    rhs.resource_format_ = {};
    stride_ = rhs.stride_;
    rhs.stride_ = 0;
    return *this;
  }

  ~ImportedImage();

  // The starting physical address of the image's pixel data.
  //
  // The image's pixel data is stored in continuous memory.
  zx_paddr_t physical_address() const { return physical_address_; }

  // The virtio resource ID used to attach this image.
  uint32_t virtio_resource_id() const { return virtio_resource_id_; }

  virtio_abi::ResourceFormat resource_format() const { return resource_format_; }

  uint32_t stride() const { return stride_; }

  // See `virtio_resource_id()` for details.
  //
  // `virtio_resource_id` must be attached while it is used in this instance.
  //
  // TODO(costan): Use a RAII handle for `virtio_resource_id` that
  // auto-detaches and releases the resource on destruction.
  // `VirtioImageResource` seems like a good name.
  ImportedImage& set_virtio_resource_id(uint32_t virtio_resource_id) {
    virtio_resource_id_ = virtio_resource_id;
    return *this;
  }

  // Rounds up the given size to the system's page size.
  static size_t RoundedUpImageSize(size_t original_image_size);

 private:
  // Creates an instance representing the image moved-out state.
  //
  // This constructor is not exposed to reduce the odds of accidentally creating
  // empty instances by forgetting to initialize local or member variables. User
  // code must call CreateEmpty() explicitly.
  ImportedImage();

  zx_paddr_t physical_address_ = 0;
  zx::pmt pinned_memory_token_;
  uint32_t virtio_resource_id_ = virtio_abi::kInvalidResourceId;
  virtio_abi::ResourceFormat resource_format_ = {};
  uint32_t stride_ = {};
};

}  // namespace virtio_display

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_VIRTIO_GPU_DISPLAY_CPP_IMPORTED_IMAGE_H_
