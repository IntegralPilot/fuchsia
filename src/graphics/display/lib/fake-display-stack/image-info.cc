// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/fake-display-stack/image-info.h"

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>

#include <cstddef>
#include <cstdint>
#include <functional>

#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace fake_display {

// static
zx::result<SysmemBufferInfo> SysmemBufferInfo::GetSysmemMetadata(
    fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& sysmem_client, uint32_t buffer_index) {
  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.

  // Ensure that the WaitForAllBuffersAllocated() call below will return quickly.
  fidl::WireResult<fuchsia_sysmem2::BufferCollection::CheckAllBuffersAllocated> check_result =
      sysmem_client->CheckAllBuffersAllocated();
  if (!check_result.ok()) {
    if (check_result.status() == ZX_ERR_PEER_CLOSED) {
      fdf::warn("CheckAllBuffersAllocated() FIDL call failed: FIDL server is closed");
    } else {
      fdf::error("CheckAllBuffersAllocated() FIDL call failed: {}", check_result.status_string());
    }
    return zx::error(check_result.status());
  }
  const fit::result<fuchsia_sysmem2::wire::Error>& check_response = check_result.value();
  if (check_response.is_error()) {
    fuchsia_sysmem2::wire::Error error_value = check_response.error_value();
    if (error_value == fuchsia_sysmem2::wire::Error::kPending) {
      return zx::error(ZX_ERR_SHOULD_WAIT);
    }

    fdf::error("CheckAllBuffersAllocated() failed: {}", fidl::ToUnderlying(error_value));
    return zx::error(ZX_ERR_INTERNAL);
  }

  fidl::WireResult<fuchsia_sysmem2::BufferCollection::WaitForAllBuffersAllocated> wait_result =
      sysmem_client->WaitForAllBuffersAllocated();
  if (!wait_result.ok()) {
    if (wait_result.status() == ZX_ERR_PEER_CLOSED) {
      fdf::warn("WaitForAllBuffersAllocated() FIDL call failed: FIDL server is closed");
    } else {
      fdf::error("WaitForAllBuffersAllocated() FIDL call failed: {}", wait_result.status_string());
    }
    return zx::error(wait_result.status());
  }
  const fit::result<fuchsia_sysmem2::wire::Error,
                    fuchsia_sysmem2::wire::BufferCollectionWaitForAllBuffersAllocatedResponse*>&
      wait_response = wait_result.value();
  if (wait_response.is_error()) {
    fuchsia_sysmem2::Error error_value = check_response.error_value();
    ZX_DEBUG_ASSERT_MSG(error_value != fuchsia_sysmem2::wire::Error::kPending,
                        "CheckAllBuffersAllocated() returned success incorrectly");

    fdf::error("WaitForAllBuffersAllocated() failed: {}", fidl::ToUnderlying(error_value));
    return zx::error(ZX_ERR_INTERNAL);
  }
  fuchsia_sysmem2::wire::BufferCollectionInfo& collection_info =
      wait_response->buffer_collection_info();

  ZX_DEBUG_ASSERT_MSG(collection_info.has_buffers(), "Sysmem deviated from its contract");
  if (buffer_index >= collection_info.buffers().size()) {
    fdf::warn("Rejecting access to out-of-range BufferCollection index: {}", buffer_index);
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }
  fuchsia_sysmem2::wire::VmoBuffer& buffer = collection_info.buffers().at(buffer_index);

  ZX_DEBUG_ASSERT_MSG(buffer.has_vmo(), "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(buffer.has_vmo_usable_start(), "Sysmem deviated from its contract");
  if (buffer.has_close_weak_asap()) {
    fdf::info("Sysmem buffer has close_weak_asap set; fake-display-stack will ignore it.");
  }

  ZX_DEBUG_ASSERT_MSG(collection_info.has_settings(), "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(collection_info.settings().has_buffer_settings(),
                      "Sysmem deviated from its contract");
  fuchsia_sysmem2::wire::BufferMemorySettings& buffer_memory_settings =
      collection_info.settings().buffer_settings();

  ZX_DEBUG_ASSERT_MSG(buffer_memory_settings.has_coherency_domain(),
                      "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(buffer_memory_settings.has_is_physically_contiguous(),
                      "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(buffer_memory_settings.has_is_secure(), "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(buffer_memory_settings.has_size_bytes(), "Sysmem deviated from its contract");

  if (!collection_info.settings().has_image_format_constraints()) {
    fdf::warn("Rejecting access to BufferCollection without ImageFormatConstraints");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  fuchsia_sysmem2::wire::ImageFormatConstraints& image_format_constraints =
      collection_info.settings().image_format_constraints();

  ZX_DEBUG_ASSERT_MSG(image_format_constraints.has_pixel_format(),
                      "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(image_format_constraints.has_pixel_format_modifier(),
                      "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(image_format_constraints.has_min_size(), "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(image_format_constraints.has_min_bytes_per_row(),
                      "Sysmem deviated from its contract");

  fuchsia_images2::wire::PixelFormat fidl_pixel_format = image_format_constraints.pixel_format();
  if (!display::PixelFormat::IsSupported(fidl_pixel_format)) {
    fdf::warn("Rejecting access to BufferCollection with unsupported PixelFormat: {}",
              static_cast<uint32_t>(fidl_pixel_format));
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(SysmemBufferInfo{
      .image_vmo = std::move(buffer.vmo()),
      .image_vmo_offset = buffer.vmo_usable_start(),
      .pixel_format = fidl_pixel_format,
      .pixel_format_modifier = image_format_constraints.pixel_format_modifier(),
      .minimum_size = image_format_constraints.min_size(),
      .minimum_bytes_per_row = image_format_constraints.min_bytes_per_row(),
      .coherency_domain = buffer_memory_settings.coherency_domain(),
  });
}

DisplayImageInfo::DisplayImageInfo(IdType id, display::ImageMetadata metadata,
                                   SysmemBufferInfo sysmem_buffer_info)
    : id_(id), metadata_(metadata), sysmem_buffer_info_(std::move(sysmem_buffer_info)) {}

DisplayImageInfo::HashTable::KeyType DisplayImageInfo::GetKey() const { return id_; }

// static
size_t DisplayImageInfo::GetHash(HashTable::KeyType key) {
  return std::hash<HashTable::KeyType>()(key);
}

CaptureImageInfo::CaptureImageInfo(IdType id, display::ImageMetadata metadata,
                                   SysmemBufferInfo sysmem_buffer_info)
    : id_(id), metadata_(metadata), sysmem_buffer_info_(std::move(sysmem_buffer_info)) {}

CaptureImageInfo::HashTable::KeyType CaptureImageInfo::GetKey() const { return id_; }

// static
size_t CaptureImageInfo::GetHash(HashTable::KeyType key) {
  return std::hash<HashTable::KeyType>()(key);
}

}  // namespace fake_display
