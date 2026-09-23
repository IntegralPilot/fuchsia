// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/bin/display-test/image.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/image-format/image_format.h>
#include <lib/zx/event.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <zircon/assert.h>
#include <zircon/process.h>
#include <zircon/status.h>
#include <zircon/syscalls.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <cstring>
#include <limits>

#include <fbl/algorithm.h>

#include "src/graphics/display/bin/display-test/utils.h"
#include "src/graphics/display/lib/api-types/cpp/buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"

static constexpr uint32_t kRenderPeriod = 120;

// TODO(reveman): Add sysmem helper functions instead of duplicating these constants.
static constexpr uint32_t kIntelTilePixelWidth = 32u;
static constexpr uint32_t kIntelTilePixelHeight = 32u;

static constexpr uint32_t kAfbcBodyAlignment = 1024u;
static constexpr uint32_t kAfbcBytesPerBlockHeader = 16u;
static constexpr uint32_t kAfbcTilePixelWidth = 16u;
static constexpr uint32_t kAfbcTilePixelHeight = 16u;

namespace fhd = fuchsia_hardware_display;
namespace fhdt = fuchsia_hardware_display_types;

namespace display_test {

Image::Image(uint32_t width, uint32_t height, int32_t stride,
             fuchsia_images2::wire::PixelFormat format,
             display::BufferCollectionId buffer_collection_id, void* buf, Pattern pattern,
             uint32_t fg_color, uint32_t bg_color,
             fuchsia_images2::wire::PixelFormatModifier modifier)
    : width_(width),
      height_(height),
      stride_(stride),
      format_(format),
      buffer_collection_id_(buffer_collection_id),
      buf_(buf),
      pattern_(pattern),
      fg_color_(fg_color),
      bg_color_(bg_color),
      modifier_(modifier) {}

Image* Image::Create(const fidl::WireSyncClient<fhd::Coordinator>& dc, uint32_t width,
                     uint32_t height, fuchsia_images2::PixelFormat format, Pattern pattern,
                     uint32_t fg_color, uint32_t bg_color,
                     fuchsia_images2::PixelFormatModifier modifier) {
  zx::result client_end = component::Connect<fuchsia_sysmem2::Allocator>();
  if (client_end.is_error()) {
    fprintf(stderr, "Failed to connect to sysmem: %s\n", client_end.status_string());
    return nullptr;
  }
  fidl::SyncClient allocator{std::move(client_end.value())};

  fidl::SyncClient<fuchsia_sysmem2::BufferCollectionToken> token;
  {
    auto [client, server] = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    fuchsia_sysmem2::AllocatorAllocateSharedCollectionRequest allocate_shared_request;
    allocate_shared_request.token_request() = std::move(server);
    const auto allocate_shared_result =
        allocator->AllocateSharedCollection(std::move(allocate_shared_request));
    if (!allocate_shared_result.is_ok()) {
      fprintf(stderr, "Failed to allocate shared collection: %s\n",
              allocate_shared_result.error_value().FormatDescription().c_str());
      return nullptr;
    }
    token.Bind(std::move(client));
  }
  fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> display_token_handle;
  {
    auto [client, server] = fidl::Endpoints<fuchsia_sysmem2::BufferCollectionToken>::Create();
    fuchsia_sysmem2::BufferCollectionTokenDuplicateRequest dup_request;
    dup_request.rights_attenuation_mask() = ZX_RIGHT_SAME_RIGHTS;
    dup_request.token_request() = std::move(server);
    const auto duplicate_result = token->Duplicate(std::move(dup_request));
    if (!duplicate_result.is_ok()) {
      fprintf(stderr, "Failed to duplicate token: %s\n",
              duplicate_result.error_value().FormatDescription().c_str());
      return nullptr;
    }
    display_token_handle = std::move(client);
  }

  static display::BufferCollectionId next_buffer_collection_id(fhdt::wire::kInvalidDispId + 1);
  display::BufferCollectionId buffer_collection_id = next_buffer_collection_id++;
  if (!token->Sync().is_ok()) {
    fprintf(stderr, "Failed to sync token\n");
    return nullptr;
  }

  fuchsia_hardware_display::wire::BufferCollectionId fidl_buffer_collection_id =
      buffer_collection_id.ToFidl();
  auto import_result =
      dc->ImportBufferCollection(fidl_buffer_collection_id, std::move(display_token_handle));
  if (!import_result.ok() || import_result.value().is_error()) {
    fprintf(stderr, "Failed to import buffer collection\n");
    return nullptr;
  }

  fhdt::wire::ImageBufferUsage image_buffer_usage = {
      .tiling_type = fhdt::wire::kImageTilingTypeLinear,
  };
  auto set_constraints_result =
      dc->SetBufferCollectionConstraints(fidl_buffer_collection_id, image_buffer_usage);
  if (!set_constraints_result.ok() || set_constraints_result.value().is_error()) {
    fprintf(stderr, "Failed to set constraints\n");
    return nullptr;
  }

  fidl::SyncClient<fuchsia_sysmem2::BufferCollection> collection;
  {
    auto [client, server] = fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();
    fuchsia_sysmem2::AllocatorBindSharedCollectionRequest bind_shared_request;
    bind_shared_request.token() = token.TakeClientEnd();
    bind_shared_request.buffer_collection_request() = std::move(server);
    const auto bind_shared_result = allocator->BindSharedCollection(std::move(bind_shared_request));
    if (!bind_shared_result.is_ok()) {
      fprintf(stderr, "Failed to bind shared collection: %s\n",
              bind_shared_result.error_value().FormatDescription().c_str());
      return nullptr;
    }
    collection.Bind(std::move(client));
  }

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest set_constraints_request;
  auto& constraints = set_constraints_request.constraints().emplace();
  constraints.usage().emplace();
  constraints.usage()->cpu() =
      fuchsia_sysmem2::kCpuUsageReadOften | fuchsia_sysmem2::kCpuUsageWriteOften;
  constraints.min_buffer_count_for_camping() = 1;
  auto& buffer_constraints = constraints.buffer_memory_constraints().emplace();
  buffer_constraints.ram_domain_supported() = true;
  auto& image_constraints = constraints.image_format_constraints().emplace().emplace_back();
  if (format == fuchsia_images2::wire::PixelFormat::kB8G8R8A8) {
    image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kB8G8R8A8;
    image_constraints.color_spaces().emplace().emplace_back(fuchsia_images2::ColorSpace::kSrgb);
  } else if (format == fuchsia_images2::PixelFormat::kR8G8B8A8) {
    image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kR8G8B8A8;
    image_constraints.color_spaces().emplace().emplace_back(fuchsia_images2::ColorSpace::kSrgb);
  } else if (format == fuchsia_images2::PixelFormat::kNv12) {
    image_constraints.pixel_format() = fuchsia_images2::PixelFormat::kNv12;
    image_constraints.color_spaces().emplace().emplace_back(fuchsia_images2::ColorSpace::kRec709);
  } else {
    fprintf(stderr, "Unsupported pixel format type: %u\n", static_cast<uint32_t>(format));
    return nullptr;
  }
  image_constraints.pixel_format_modifier() = modifier;

  image_constraints.min_size() = {width, height};
  image_constraints.max_size() = {width, height};

  if (!collection->SetConstraints(std::move(set_constraints_request)).is_ok()) {
    fprintf(stderr, "Failed to set local constraints\n");
    return nullptr;
  }

  auto info_result = collection->WaitForAllBuffersAllocated();
  if (!info_result.is_ok()) {
    fprintf(stderr, "Failed to wait for buffers allocated: %s",
            info_result.error_value().FormatDescription().c_str());
    return nullptr;
  }

  if (!collection->Release().is_ok()) {
    fprintf(stderr, "Failed to close buffer collection\n");
    return nullptr;
  }

  auto& buffer_collection_info = info_result.value().buffer_collection_info().value();
  uint64_t buffer_size = buffer_collection_info.settings()->buffer_settings()->size_bytes().value();
  zx::vmo vmo(std::move(buffer_collection_info.buffers()->at(0).vmo().value()));

  uint32_t minimum_row_bytes;
  bool result = ImageFormatMinimumRowBytes(
      buffer_collection_info.settings()->image_format_constraints().value(), width,
      &minimum_row_bytes);
  if (!result) {
    minimum_row_bytes =
        buffer_collection_info.settings()->image_format_constraints()->min_bytes_per_row().value();
  }

  uint32_t stride_pixels = minimum_row_bytes / ImageFormatStrideBytesPerWidthPixel(
                                                   PixelFormatAndModifier(format, modifier));
  uintptr_t addr;
  uint32_t perms = ZX_VM_PERM_READ | ZX_VM_PERM_WRITE;
  if (zx::vmar::root_self()->map(perms, 0, vmo, 0, buffer_size, &addr) != ZX_OK) {
    printf("Failed to map vmar\n");
    return nullptr;
  }

  // We don't expect stride to be much more than width, or expect the buffer
  // to be much more than stride * height, so just fill the whole buffer with
  // bg_color.
  uint32_t* ptr = reinterpret_cast<uint32_t*>(addr);
  for (unsigned i = 0; i < buffer_size / sizeof(uint32_t); i++) {
    ptr[i] = bg_color;
  }
  if (modifier == fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16) {
    uint32_t width_in_tiles = (width + kAfbcTilePixelWidth - 1) / kAfbcTilePixelWidth;
    uint32_t height_in_tiles = (height + kAfbcTilePixelHeight - 1) / kAfbcTilePixelHeight;
    uint32_t tile_count = width_in_tiles * height_in_tiles;
    // Initialize all block headers to |bg_color|.
    for (unsigned i = 0; i < tile_count; ++i) {
      unsigned offset = i * kAfbcBytesPerBlockHeader / sizeof(uint32_t);
      ptr[offset + 0] = 0;
      ptr[offset + 1] = 0;
      // Solid colors are stored as R8G8B8A8 starting at offset 8 in block header.
      ptr[offset + 2] = bg_color;
      ptr[offset + 3] = 0;
    }
  }
  zx_cache_flush(ptr, buffer_size, ZX_CACHE_FLUSH_DATA);

  return new Image(width, height, stride_pixels, format, buffer_collection_id, ptr, pattern,
                   fg_color, bg_color, modifier);
}

#define STRIPE_SIZE 37  // prime to make movement more interesting

void Image::Render(int32_t prev_step, int32_t step_num) {
  if (format_ == fuchsia_images2::wire::PixelFormat::kNv12) {
    RenderNv12(prev_step, step_num);
  } else {
    uint32_t start, end;
    bool draw_stripe;
    if (step_num < 0) {
      start = 0;
      end = height_;
      draw_stripe = true;
    } else {
      uint32_t prev = interpolate(height_, prev_step, kRenderPeriod);
      uint32_t cur = interpolate(height_, step_num, kRenderPeriod);
      start = std::min(cur, prev);
      end = std::max(cur, prev);
      draw_stripe = cur > prev;
    }
    if (pattern_ == Pattern::kCheckerboard) {
      auto pixel_generator = [this, draw_stripe](uint32_t x, uint32_t y) {
        bool in_stripe = draw_stripe && ((x / STRIPE_SIZE % 2) != (y / STRIPE_SIZE % 2));

        int32_t color = in_stripe ? fg_color_ : bg_color_;
        return color;
      };

      if (modifier_ == fuchsia_images2::wire::PixelFormatModifier::kLinear) {
        RenderLinear(pixel_generator, start, end);
      } else {
        RenderTiled(pixel_generator, start, end);
      }
    } else if (pattern_ == Pattern::kBorder) {
      auto pixel_generator = [this](uint32_t x, uint32_t y) {
        bool in_stripe = (y == 0) || (x == 0) || (y == height_ - 1) || (x == width_ - 1);

        int32_t color = in_stripe ? fg_color_ : bg_color_;
        return color;
      };

      if (modifier_ == fuchsia_images2::wire::PixelFormatModifier::kLinear) {
        RenderLinear(pixel_generator, start, end);
      } else {
        RenderTiled(pixel_generator, start, end);
      }
    } else {
      ZX_DEBUG_ASSERT(false);
    }
  }
}

void Image::RenderNv12(int32_t prev_step, int32_t step_num) {
  ZX_DEBUG_ASSERT(pattern_ == Pattern::kCheckerboard);
  uint32_t byte_stride =
      stride_ * ImageFormatStrideBytesPerWidthPixel(PixelFormatAndModifier(format_, modifier_));
  uint32_t real_height = height_;
  for (uint32_t y = 0; y < real_height; y++) {
    uint8_t* buf = static_cast<uint8_t*>(buf_) + y * stride_;
    memset(buf, 128, stride_);
  }

  for (uint32_t y = 0; y < real_height / 2; y++) {
    for (uint32_t x = 0; x < width_ / 2; x++) {
      uint8_t* buf = static_cast<uint8_t*>(buf_) + real_height * stride_ + y * stride_ + x * 2;
      int32_t in_stripe = (((x * 2) / STRIPE_SIZE % 2) != ((y * 2) / STRIPE_SIZE % 2));
      if (in_stripe) {
        buf[0] = 16;
        buf[1] = 256 - 16;
      } else {
        buf[0] = 256 - 16;
        buf[1] = 16;
      }
    }
  }
  zx_cache_flush(reinterpret_cast<uint8_t*>(buf_), byte_stride * height_ * 3 / 2,
                 ZX_CACHE_FLUSH_DATA);
}

template <typename T>
void Image::RenderLinear(T pixel_generator, uint32_t start_y, uint32_t end_y) {
  for (unsigned y = start_y; y < end_y; y++) {
    for (unsigned x = 0; x < width_; x++) {
      int32_t color = pixel_generator(x, y);

      uint32_t* ptr = static_cast<uint32_t*>(buf_) + (y * stride_) + x;
      *ptr = color;
    }
  }
  uint32_t byte_stride =
      stride_ * ImageFormatStrideBytesPerWidthPixel(PixelFormatAndModifier(format_, modifier_));
  zx_cache_flush(reinterpret_cast<uint8_t*>(buf_) + (byte_stride * start_y),
                 byte_stride * (end_y - start_y), ZX_CACHE_FLUSH_DATA);
}

template <typename T>
void Image::RenderTiled(T pixel_generator, uint32_t start_y, uint32_t end_y) {
  constexpr uint32_t kTileBytesPerPixel = 4u;

  uint32_t tile_pixel_width = 0u;
  uint32_t tile_pixel_height = 0u;
  uint8_t* body = nullptr;
  uint32_t width_in_tiles = 0;
  switch (modifier_) {
    case fuchsia_images2::wire::PixelFormatModifier::kIntelI915YTiled: {
      tile_pixel_width = kIntelTilePixelWidth;
      tile_pixel_height = kIntelTilePixelHeight;
      body = static_cast<uint8_t*>(buf_);
      width_in_tiles = (stride_ + tile_pixel_width - 1) / tile_pixel_width;
    } break;
    case fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16: {
      tile_pixel_width = kAfbcTilePixelWidth;
      tile_pixel_height = kAfbcTilePixelHeight;
      width_in_tiles = (width_ + tile_pixel_width - 1) / tile_pixel_width;
      uint32_t height_in_tiles = (height_ + tile_pixel_height - 1) / tile_pixel_height;
      uint32_t tile_count = width_in_tiles * height_in_tiles;
      uint32_t body_offset =
          ((tile_count * kAfbcBytesPerBlockHeader + kAfbcBodyAlignment - 1) / kAfbcBodyAlignment) *
          kAfbcBodyAlignment;
      body = static_cast<uint8_t*>(buf_) + body_offset;
    } break;
    default:
      // Not reached.
      assert(0);
  }

  uint32_t tile_num_bytes = tile_pixel_width * tile_pixel_height * kTileBytesPerPixel;
  uint32_t tile_num_pixels = tile_num_bytes / kTileBytesPerPixel;

  for (unsigned y = start_y; y < end_y; y++) {
    for (unsigned x = 0; x < width_; x++) {
      int32_t color = pixel_generator(x, y);

      uint32_t* ptr = reinterpret_cast<uint32_t*>(body);
      {
        // Add the offset to the pixel's tile
        uint32_t tile_idx = (y / tile_pixel_height) * width_in_tiles + (x / tile_pixel_width);
        ptr += (tile_num_pixels * tile_idx);
        switch (modifier_) {
          case fuchsia_images2::wire::PixelFormatModifier::kIntelI915YTiled: {
            constexpr uint32_t kSubtileColumnWidth = 4u;
            // Add the offset within the pixel's tile
            uint32_t subtile_column_offset =
                ((x % tile_pixel_width) / kSubtileColumnWidth) * tile_pixel_height;
            uint32_t subtile_line_offset =
                (subtile_column_offset + (y % tile_pixel_height)) * kSubtileColumnWidth;
            ptr += subtile_line_offset + (x % kSubtileColumnWidth);
          } break;
          case fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16: {
            constexpr uint32_t kAfbcSubtileOffset[4][4] = {
                {2u, 1u, 14u, 13u},
                {3u, 0u, 15u, 12u},
                {4u, 7u, 8u, 11u},
                {5u, 6u, 9u, 10u},
            };
            constexpr uint32_t kAfbcSubtileWidth = 4u;
            constexpr uint32_t kAfbcSubtileHeight = 4u;
            uint32_t subtile_num_pixels = kAfbcSubtileWidth * kAfbcSubtileHeight;
            uint32_t subtile_x = (x % tile_pixel_width) / kAfbcSubtileWidth;
            uint32_t subtile_y = (y % tile_pixel_height) / kAfbcSubtileHeight;
            ptr += kAfbcSubtileOffset[subtile_x][subtile_y] * subtile_num_pixels +
                   (y % kAfbcSubtileHeight) * kAfbcSubtileWidth + (x % kAfbcSubtileWidth);
          } break;
          default:
            // Not reached.
            assert(0);
        }
      }
      *ptr = color;
    }
  }
  uint32_t y_start_tile = start_y / tile_pixel_height;
  uint32_t y_end_tile = (end_y + tile_pixel_height - 1) / tile_pixel_height;
  for (unsigned i = 0; i < width_in_tiles; i++) {
    for (unsigned j = y_start_tile; j < y_end_tile; j++) {
      unsigned offset = (tile_num_bytes * (j * width_in_tiles + i));
      zx_cache_flush(body + offset, tile_num_bytes, ZX_CACHE_FLUSH_DATA);

      // We also need to update block header when using AFBC.
      if (modifier_ == fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16) {
        unsigned hdr_offset = kAfbcBytesPerBlockHeader * (j * width_in_tiles + i);
        uint8_t* hdr_ptr = reinterpret_cast<uint8_t*>(buf_) + hdr_offset;
        // Store offset of uncompressed tile memory in byte 0-3.
        const ptrdiff_t body_offset = body - reinterpret_cast<uint8_t*>(buf_);
        ZX_ASSERT(body_offset >= 0);
        ZX_ASSERT(body_offset <= std::numeric_limits<uint32_t>::max());
        *(reinterpret_cast<uint32_t*>(hdr_ptr)) = static_cast<uint32_t>(body_offset) + offset;
        // Set byte 4-15 to disable compression for tile memory.
        hdr_ptr[4] = hdr_ptr[7] = hdr_ptr[10] = hdr_ptr[13] = 0x41;
        hdr_ptr[5] = hdr_ptr[8] = hdr_ptr[11] = hdr_ptr[14] = 0x10;
        hdr_ptr[6] = hdr_ptr[9] = hdr_ptr[12] = hdr_ptr[15] = 0x04;
        zx_cache_flush(hdr_ptr, kAfbcBytesPerBlockHeader, ZX_CACHE_FLUSH_DATA);
      }
    }
  }
}

fuchsia_hardware_display_types::wire::ImageMetadata Image::GetMetadata() const {
  fuchsia_hardware_display_types::wire::ImageMetadata image_metadata = {
      .dimensions = {.width = width_, .height = height_},
  };
  if (modifier_ != fuchsia_images2::wire::PixelFormatModifier::kIntelI915YTiled) {
    image_metadata.tiling_type = fuchsia_hardware_display_types::wire::kImageTilingTypeLinear;
  } else {
    image_metadata.tiling_type = 2;  // IMAGE_TILING_TYPE_Y_LEGACY
  }
  return image_metadata;
}

bool Image::Import(const fidl::WireSyncClient<fhd::Coordinator>& dc, display::ImageId image_id,
                   image_import_t* info_out) const {
  for (int i = 0; i < 2; i++) {
    static display::EventId next_event_id(fhdt::wire::kInvalidDispId + 1);
    zx::event e1;
    if (zx_status_t status = zx::event::create(0, &e1); status != ZX_OK) {
      printf("Failed to create event: %s\n", zx_status_get_string(status));
      return false;
    }
    zx::event e2;
    if (zx_status_t status = e1.duplicate(ZX_RIGHT_SAME_RIGHTS, &e2); status != ZX_OK) {
      printf("Failed to duplicate event: %s\n", zx_status_get_string(status));
      return false;
    }

    const display::EventId event_id = next_event_id++;
    info_out->events[i] = std::move(e1);
    info_out->event_ids[i] = event_id;
    const fidl::Status result = dc->ImportEvent(std::move(e2), event_id.ToFidl());
    if (!result.ok()) {
      printf("Failed to import event: %s\n", result.FormatDescription().c_str());
      return false;
    }

    if (i != WAIT_EVENT) {
      info_out->events[i].signal(0, ZX_EVENT_SIGNALED);
    }
  }

  fhdt::wire::ImageMetadata image_metadata = GetMetadata();
  const fidl::WireResult import_result =
      dc->ImportImage(image_metadata, buffer_collection_id_.ToFidl(), 0, image_id.ToFidl());
  if (!import_result.ok()) {
    printf("Failed to import image: %s\n", import_result.FormatDescription().c_str());
    return false;
  }
  if (import_result.value().is_error()) {
    printf("Failed to import image: %s\n",
           zx_status_get_string(import_result.value().error_value()));
    return false;
  }
  info_out->id = image_id;

  // image has been imported. we can close the connection
  [[maybe_unused]] fidl::Status result =
      dc->ReleaseBufferCollection(buffer_collection_id_.ToFidl());
  return true;
}

}  // namespace display_test
