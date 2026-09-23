// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/imported-images.h"

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/result.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls/object.h>
#include <zircon/types.h>

#include <cinttypes>
#include <cstdint>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/intrusive_hash_table.h>
#include <fbl/string_printf.h>

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/imported-image.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"

namespace virtio_display {

ImportedBufferCollection::ImportedBufferCollection(
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollection> sysmem_client)
    : sysmem_client_(
          fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>(std::move(sysmem_client))) {
  ZX_DEBUG_ASSERT(sysmem_client_.is_valid());
}

ImportedBufferCollection::~ImportedBufferCollection() = default;

zx::result<SysmemBufferInfo> ImportedBufferCollection::GetSysmemMetadata(uint32_t buffer_index) {
  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.

  // Ensure that the WaitForAllBuffersAllocated() call below will return quickly.
  fidl::WireResult<fuchsia_sysmem2::BufferCollection::CheckAllBuffersAllocated> check_result =
      sysmem_client_->CheckAllBuffersAllocated();
  if (!check_result.ok()) {
    fdf::error("CheckAllBuffersAllocated() FIDL call failed: {}", check_result.status_string());
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
      sysmem_client_->WaitForAllBuffersAllocated();
  if (!wait_result.ok()) {
    fdf::error("WaitForAllBuffersAllocated() FIDL call failed: {}", wait_result.status_string());
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
  ZX_DEBUG_ASSERT_MSG(buffer.vmo().is_valid(), "Sysmem deviated from its contract");
  ZX_DEBUG_ASSERT_MSG(buffer.has_vmo_usable_start(), "Sysmem deviated from its contract");

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

  uint32_t bytes_per_row_divisor = image_format_constraints.has_bytes_per_row_divisor()
                                       ? image_format_constraints.bytes_per_row_divisor()
                                       : 1;
  ZX_DEBUG_ASSERT_MSG(bytes_per_row_divisor != 0, "Sysmem deviated from its contract");

  fuchsia_images2::wire::PixelFormat fidl_pixel_format = image_format_constraints.pixel_format();
  if (!display::PixelFormat::IsSupported(fidl_pixel_format)) {
    fdf::warn("Rejecting access to BufferCollection with unsupported PixelFormat: {}",
              static_cast<uint32_t>(fidl_pixel_format));
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok(SysmemBufferInfo{
      .image_vmo = std::move(buffer.vmo()),
      .image_vmo_offset = buffer.vmo_usable_start(),
      .pixel_format = display::PixelFormat(fidl_pixel_format),
      .pixel_format_modifier = image_format_constraints.pixel_format_modifier(),
      .minimum_size = image_format_constraints.min_size(),
      .minimum_bytes_per_row = image_format_constraints.min_bytes_per_row(),
      .bytes_per_row_divisor = bytes_per_row_divisor,
      .coherency_domain = buffer_memory_settings.coherency_domain(),
  });
}

ImportedImages::ImportedImages(fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_client)
    : sysmem_client_(std::move(sysmem_client)) {
  ZX_DEBUG_ASSERT(sysmem_client_.is_valid());
}

ImportedImages::~ImportedImages() = default;

namespace {

zx::result<zx_koid_t> GetOwnProcessKoid() {
  zx::unowned_process process = zx::process::self();
  zx_info_handle_basic_t process_info;
  zx_status_t status = process->get_info(ZX_INFO_HANDLE_BASIC, &process_info, sizeof(process_info),
                                         nullptr, nullptr);
  if (status != ZX_OK) {
    fdf::error("Failed to get process koid: {}", zx::make_result(status));
    return zx::error(status);
  }
  return zx::ok(process_info.koid);
}

}  // namespace

zx::result<> ImportedImages::Initialize() {
  zx::result<zx_koid_t> koid_result = GetOwnProcessKoid();
  if (koid_result.is_error()) {
    return koid_result.take_error();
  }

  // TODO(costan): fbl::StringPrintf() may allocate memory and crash. Fix it to use checked
  // dynamic memory allocation.
  fbl::String debug_name = fbl::StringPrintf("virtio-gpu-display[%lu]", koid_result.value());
  fidl::Arena arena;
  fidl::OneWayStatus set_debug_status = sysmem_client_->SetDebugClientInfo(
      fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
          .name(fidl::StringView::FromExternal(debug_name))
          .id(koid_result.value())
          .Build());
  if (!set_debug_status.ok()) {
    fdf::warn("Failed to set sysmem debug info: {}", set_debug_status.status_string());
    // This step was not essential to the initialization process. Sysmem debug
    // info is a developer convenience, and the driver can operate without it.
  }
  return zx::ok();
}

zx::result<> ImportedImages::ImportBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
  auto buffer_collection_it = buffer_collections_.find(buffer_collection_id);
  if (buffer_collection_it != buffer_collections_.end()) {
    fdf::warn("Rejected BufferCollection import request with existing ID: {}",
              buffer_collection_id.value());
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  auto [collection_client_endpoint, collection_server_endpoint] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

  // TODO(costan): fidl::Arena may allocate memory and crash. Find a way to get
  // control over memory allocation.
  fidl::Arena arena;
  fidl::OneWayStatus bind_result = sysmem_client_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(buffer_collection_token))
          .buffer_collection_request(std::move(collection_server_endpoint))
          .Build());
  if (!bind_result.ok()) {
    fdf::error("FIDL call BindSharedCollection failed: {}", bind_result.status_string());
    return zx::error(ZX_ERR_INTERNAL);
  }

  fbl::AllocChecker alloc_checker;
  auto buffer_collection_node = fbl::make_unique_checked<ImportedBufferCollectionNode>(
      &alloc_checker, buffer_collection_id, std::move(collection_client_endpoint));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for ImportedBufferCollectionNode");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  buffer_collections_.insert(std::move(buffer_collection_node));

  return zx::ok();
}

zx::result<> ImportedImages::ReleaseBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id) {
  auto buffer_collection_it = buffer_collections_.find(buffer_collection_id);
  if (buffer_collection_it == buffer_collections_.end()) {
    fdf::warn("Rejected request to release BufferCollection with unknown ID: {}",
              buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  buffer_collections_.erase(buffer_collection_it);
  return zx::ok();
}

zx::result<display::DriverImageId> ImportedImages::ImportImage(
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  auto buffer_collection_it = buffer_collections_.find(buffer_collection_id);
  if (buffer_collection_it == buffer_collections_.end()) {
    fdf::warn("Rejected request to release BufferCollection with unknown ID: {}",
              buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  zx::result<SysmemBufferInfo> sysmem_buffer_info =
      buffer_collection_it->imported_buffer_collection.GetSysmemMetadata(buffer_index);
  if (sysmem_buffer_info.is_error()) {
    // GetSysmemMetadata() already logged the error.
    return sysmem_buffer_info.take_error();
  }

  const display::DriverImageId image_id = next_image_id_;

  fbl::AllocChecker alloc_checker;
  auto image_node = fbl::make_unique_checked<ImportedImageNode>(
      &alloc_checker, image_id, std::move(sysmem_buffer_info).value());
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for ImportedImageNode");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  images_.insert(std::move(image_node));

  next_image_id_ = static_cast<display::DriverImageId>(next_image_id_.value() + 1);
  return zx::ok(image_id);
}

zx::result<> ImportedImages::ReleaseImage(display::DriverImageId image_id) {
  auto image_it = images_.find(image_id);
  if (image_it == images_.end()) {
    fdf::warn("Rejected request to release Image with unknown ID: {}", image_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  images_.erase(image_it);
  return zx::ok();
}

ImportedBufferCollection* ImportedImages::FindBufferCollectionById(
    display::DriverBufferCollectionId buffer_collection_id) {
  auto buffer_collection_it = buffer_collections_.find(buffer_collection_id);
  if (buffer_collection_it == buffer_collections_.end()) {
    return nullptr;
  }
  return &buffer_collection_it->imported_buffer_collection;
}

SysmemBufferInfo* ImportedImages::FindSysmemInfoById(display::DriverImageId image_id) {
  auto image_it = images_.find(image_id);
  if (image_it == images_.end()) {
    return nullptr;
  }
  return &image_it->sysmem_buffer_info;
}

ImportedImage* ImportedImages::FindImageById(display::DriverImageId image_id) {
  auto image_it = images_.find(image_id);
  if (image_it == images_.end()) {
    return nullptr;
  }
  return &image_it->imported_image;
}

ImportedImages::ImportedBufferCollectionNode::ImportedBufferCollectionNode(
    display::DriverBufferCollectionId id, fidl::ClientEnd<fuchsia_sysmem2::BufferCollection> client)
    : id(id), imported_buffer_collection(std::move(client)) {}

ImportedImages::ImportedBufferCollectionNode::~ImportedBufferCollectionNode() = default;

ImportedImages::ImportedImageNode::ImportedImageNode(display::DriverImageId id,
                                                     SysmemBufferInfo sysmem_buffer_info)
    : id(id),
      sysmem_buffer_info(std::move(sysmem_buffer_info)),
      imported_image(ImportedImage::CreateEmpty()) {
  ZX_DEBUG_ASSERT(this->sysmem_buffer_info.image_vmo.is_valid());
}

ImportedImages::ImportedImageNode::~ImportedImageNode() = default;

}  // namespace virtio_display
