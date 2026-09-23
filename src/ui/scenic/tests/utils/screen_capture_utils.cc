// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/tests/utils/screen_capture_utils.h"

#include <lib/syslog/cpp/macros.h>

#include <fbl/algorithm.h>
#include <zxtest/zxtest.h>

#include "src/ui/scenic/lib/utils/helpers.h"
#include "src/ui/testing/util/buffers_helper.h"

namespace integration_tests {
namespace fuc = fuchsia_ui_composition;
using ui_testing::MapHostPointer;

bool PixelEquals(const uint8_t* a, const uint8_t* b) { return memcmp(a, b, kBytesPerPixel) == 0; }

void AppendPixel(std::vector<uint8_t>* values, const uint8_t* pixel) {
  values->insert(values->end(), pixel, pixel + kBytesPerPixel);
}

void GenerateImageForFlatlandInstance(uint32_t buffer_collection_index,
                                      FlatlandClientWithEventHandler& flatland,
                                      fuc::TransformId parent_transform,
                                      fuc::BufferCollectionImportToken import_token,
                                      fuchsia_math::SizeU size, fuchsia_math::Vec translation,
                                      uint32_t image_id, uint32_t transform_id) {
  // Create the image in the Flatland instance.
  fuc::ImageProperties image_properties;
  image_properties.size(size);
  const fuc::ContentId content_id(image_id);
  FX_CHECK(flatland
               ->CreateImage({{.image_id = content_id,
                               .import_token = std::move(import_token),
                               .vmo_index = buffer_collection_index,
                               .properties = std::move(image_properties)}})
               .is_ok());

  // Add the created image as a child of the parent transform specified. Apply the right size and
  // orientation commands.
  const fuc::TransformId kTransform(transform_id);
  FX_CHECK(flatland->CreateTransform({{.transform_id = kTransform}}).is_ok());

  FX_CHECK(flatland->SetContent({{.transform_id = kTransform, .content_id = content_id}}).is_ok());
  FX_CHECK(flatland->SetImageDestinationSize({{.image_id = content_id, .size = size}}).is_ok());
  FX_CHECK(
      flatland->SetTranslation({{.transform_id = kTransform, .translation = translation}}).is_ok());

  FX_CHECK(
      flatland
          ->AddChild({{.parent_transform_id = parent_transform, .child_transform_id = kTransform}})
          .is_ok());
}

// This method writes to a sysmem buffer, taking into account any potential stride width
// differences. The method also flushes the cache if the buffer is in RAM domain.
void WriteToSysmemBuffer(const std::vector<uint8_t>& write_values,
                         const fuchsia_sysmem2::BufferCollectionInfo& buffer_collection_info,
                         uint32_t buffer_collection_idx, uint32_t kBytesPerPixel,
                         uint32_t image_width, uint32_t image_height) {
  FX_DCHECK(kBytesPerPixel == utils::GetBytesPerPixel(buffer_collection_info.settings().value()));
  uint32_t pixels_per_row =
      utils::GetPixelsPerRow(buffer_collection_info.settings().value(), image_width);
  // Flush the cache if we are operating in RAM.
  const bool need_flush =
      buffer_collection_info.settings()->buffer_settings()->coherency_domain() ==
      fuchsia_sysmem2::CoherencyDomain::kRam;

  MapHostPointer(
      buffer_collection_info, buffer_collection_idx, ui_testing::HostPointerAccessMode::kReadWrite,
      [&write_values, pixels_per_row, kBytesPerPixel, image_width, image_height, need_flush](
          uint8_t* vmo_host, uint32_t num_bytes) {
        uint32_t bytes_per_row = pixels_per_row * kBytesPerPixel;
        uint32_t valid_bytes_per_row = image_width * kBytesPerPixel;

        EXPECT_GE(bytes_per_row, valid_bytes_per_row);
        EXPECT_GE(num_bytes, bytes_per_row * image_height);

        if (bytes_per_row == valid_bytes_per_row) {
          // Fast path.
          memcpy(vmo_host, write_values.data(), write_values.size());
          if (need_flush) {
            EXPECT_EQ(ZX_OK, zx_cache_flush(vmo_host, write_values.size(), ZX_CACHE_FLUSH_DATA));
          }
        } else {
          // Copy over row-by-row.
          for (size_t i = 0; i < image_height; ++i) {
            memcpy(&vmo_host[i * bytes_per_row], &write_values[i * image_width * kBytesPerPixel],
                   valid_bytes_per_row);
          }
          if (need_flush) {
            EXPECT_EQ(ZX_OK,
                      zx_cache_flush(vmo_host, static_cast<size_t>(image_height) * bytes_per_row,
                                     ZX_CACHE_FLUSH_DATA));
          }
        }
      });
}

fuchsia_sysmem2::BufferCollectionInfo CreateBufferCollectionInfoWithConstraints(
    fuchsia_sysmem2::BufferCollectionConstraints constraints,
    fuc::BufferCollectionExportToken export_token,
    fidl::SyncClient<fuc::Allocator>& flatland_allocator,
    fidl::WireClient<fuchsia_sysmem2::Allocator>& sysmem_allocator,
    fuc::RegisterBufferCollectionUsages usage) {
  fuc::RegisterBufferCollectionArgs rbc_args;
  // Create Sysmem tokens.
  auto [local_token, dup_token] = utils::SysmemTokens::Create(sysmem_allocator);

  rbc_args.export_token(std::move(export_token));
  rbc_args.buffer_collection_token2(
      fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken>(dup_token.TakeChannel()));
  rbc_args.usages(usage);

  auto [collection_client_end, collection_server_end] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> buffer_collection(
      std::move(collection_client_end));

  fidl::Arena arena;
  fidl::OneWayStatus bind_status = sysmem_allocator->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(local_token))
          .buffer_collection_request(std::move(collection_server_end))
          .Build());
  FX_DCHECK(bind_status.ok());

  fuchsia_sysmem2::NodeSetNameRequest set_name_request;
  set_name_request.priority(100u);
  set_name_request.name("ScreenCaptureUtilsTest");
  auto set_name_res = buffer_collection->SetName(std::move(set_name_request));
  FX_DCHECK(set_name_res.is_ok());

  uint32_t constraints_min_buffer_count = constraints.min_buffer_count().value_or(1);

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  set_constraints_request.constraints(std::move(constraints));
  auto set_constraints_res = buffer_collection->SetConstraints(std::move(set_constraints_request));
  FX_DCHECK(set_constraints_res.is_ok());

  auto register_result = flatland_allocator->RegisterBufferCollection(std::move(rbc_args));
  FX_DCHECK(register_result.is_ok());

  // Wait for allocation.
  auto wait_result = buffer_collection->WaitForAllBuffersAllocated();
  FX_DCHECK(wait_result.is_ok());
  FX_DCHECK(wait_result->buffer_collection_info().has_value());
  auto buffer_collection_info = std::move(wait_result->buffer_collection_info().value());
  FX_DCHECK(constraints_min_buffer_count == buffer_collection_info.buffers()->size());

  auto release_res = buffer_collection->Release();
  EXPECT_TRUE(release_res.is_ok());
  return buffer_collection_info;
}

// This function returns a linear buffer of pixels of size width * height.
std::vector<uint8_t> ExtractScreenCapture(
    uint32_t buffer_id, const fuchsia_sysmem2::BufferCollectionInfo& buffer_collection_info,
    uint32_t kBytesPerPixel, uint32_t render_target_width, uint32_t render_target_height) {
  // Copy ScreenCapture output for inspection. Note that the stride of the buffer may be different
  // than the width of the image, if the width of the image is not a multiple of 64.
  //
  // For instance, if the original image were 1024x600, the new width is 600. 600*4=2400 bytes,
  // which is not a multiple of 64. The next multiple would be 2432, which would mean the buffer
  // is actually a 608x1024 "pixel" buffer, since 2432/4=608. We must account for that 8 byte
  // padding when copying the bytes over to be inspected.

  FX_DCHECK(kBytesPerPixel == utils::GetBytesPerPixel(buffer_collection_info.settings().value()));
  uint32_t pixels_per_row =
      utils::GetPixelsPerRow(buffer_collection_info.settings().value(), render_target_width);
  std::vector<uint8_t> read_values;
  read_values.resize(static_cast<size_t>(render_target_width) * render_target_height *
                     kBytesPerPixel);

  MapHostPointer(
      buffer_collection_info, buffer_id, ui_testing::HostPointerAccessMode::kReadOnly,
      [&read_values, kBytesPerPixel, pixels_per_row, render_target_width, render_target_height](
          const uint8_t* vmo_host, uint32_t num_bytes) {
        uint32_t bytes_per_row = pixels_per_row * kBytesPerPixel;
        uint32_t valid_bytes_per_row = render_target_width * kBytesPerPixel;

        EXPECT_GE(bytes_per_row, valid_bytes_per_row);

        if (bytes_per_row == valid_bytes_per_row) {
          EXPECT_EQ(ZX_OK, zx_cache_flush(vmo_host,
                                          static_cast<size_t>(bytes_per_row) * render_target_height,
                                          ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
          // Fast path.
          memcpy(read_values.data(), vmo_host,
                 static_cast<size_t>(bytes_per_row) * render_target_height);
        } else {
          EXPECT_EQ(ZX_OK, zx_cache_flush(vmo_host,
                                          static_cast<size_t>(render_target_height) * bytes_per_row,
                                          ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE));
          for (size_t i = 0; i < render_target_height; ++i) {
            memcpy(&read_values[i * render_target_width * kBytesPerPixel],
                   &vmo_host[i * bytes_per_row], valid_bytes_per_row);
          }
        }
      });

  return read_values;
}

}  // namespace integration_tests
