// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/framebuffer-display/framebuffer-display.h"
#include "src/graphics/display/lib/framebuffer-display/boot-framebuffer.h"

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/image-format/image_format.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <cinttypes>
#include <cstdint>
#include <memory>
#include <mutex>
#include <utility>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/api-types/cpp/power-mode.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"

namespace framebuffer_display {

namespace {

constexpr display::EngineInfo kEngineInfo({
    .max_layer_count = 1,
    .max_connected_display_count = 1,
    .is_capture_supported = false,
});

constexpr display::DisplayId kDisplayId(1);
constexpr display::ModeId kDisplayModeId(1);
constexpr int kRefreshRateHz = 30;

constexpr auto kVSyncInterval = zx::usec(1000000 / kRefreshRateHz);

zx_koid_t GetCurrentProcessKoid() {
  zx_handle_t handle = zx_process_self();
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

}  // namespace

// implement display controller protocol:

display::EngineInfo FramebufferDisplay::CompleteCoordinatorConnection() {
  const display::ModeAndId mode_and_id({
      .id = kDisplayModeId,
      .mode = display::Mode({
          .active_width = properties_.width_px,
          .active_height = properties_.height_px,
          .refresh_rate_millihertz = kRefreshRateHz * 1'000,
      }),
  });

  const std::span<const display::ModeAndId> preferred_modes(&mode_and_id, 1);
  const std::span<const display::PixelFormat> pixel_formats(&properties_.pixel_format, 1);
  engine_events_.OnDisplayAdded(kDisplayId, preferred_modes, pixel_formats);

  return kEngineInfo;
}

zx::result<> FramebufferDisplay::ImportBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
  if (buffer_collections_.find(buffer_collection_id) != buffer_collections_.end()) {
    fdf::error("Buffer Collection (id={}) already exists", buffer_collection_id.value());
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  ZX_DEBUG_ASSERT_MSG(sysmem_client_.is_valid(), "sysmem allocator is not initialized");

  auto [collection_client_endpoint, collection_server_endpoint] =
      fidl::Endpoints<fuchsia_sysmem2::BufferCollection>::Create();

  fidl::Arena arena;
  fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest bind_request =
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .token(std::move(buffer_collection_token))
          .buffer_collection_request(std::move(collection_server_endpoint))
          .Build();
  fidl::OneWayStatus fidl_transport_status =
      sysmem_client_->BindSharedCollection(std::move(bind_request));
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling BindSharedCollection: {}", fidl_transport_status.error());
    return zx::error(fidl_transport_status.status());
  }

  buffer_collections_[buffer_collection_id] =
      fidl::WireSyncClient(std::move(collection_client_endpoint));

  return zx::ok();
}

zx::result<> FramebufferDisplay::ReleaseBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id) {
  if (buffer_collections_.find(buffer_collection_id) == buffer_collections_.end()) {
    fdf::error("Cannot release buffer collection {}: buffer collection doesn't exist",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  buffer_collections_.erase(buffer_collection_id);
  return zx::ok();
}

zx::result<display::DriverImageId> FramebufferDisplay::ImportImage(
    const display::ImageMetadata& image_metadata,
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  const auto it = buffer_collections_.find(buffer_collection_id);
  if (it == buffer_collections_.end()) {
    fdf::error("ImportImage: Cannot find imported buffer collection (id={})",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& collection = it->second;

  fidl::WireResult<fuchsia_sysmem2::BufferCollection::CheckAllBuffersAllocated>
      check_buffers_transport_result = collection->CheckAllBuffersAllocated();
  if (!check_buffers_transport_result.ok()) {
    fdf::error("FIDL error calling CheckAllBuffersAllocated: {}",
               check_buffers_transport_result.error());
    return zx::error(check_buffers_transport_result.status());
  }
  fit::result<fuchsia_sysmem2::wire::Error>& check_buffers_domain_result =
      check_buffers_transport_result.value();
  if (check_buffers_domain_result.is_error()) {
    fuchsia_sysmem2::wire::Error& check_buffers_domain_error =
        check_buffers_domain_result.error_value();
    fdf::warn("CheckAllBuffersAllocated failed: {}",
              static_cast<uint32_t>(check_buffers_domain_error));
    if (check_buffers_domain_error == fuchsia_sysmem2::Error::kPending) {
      return zx::error(ZX_ERR_SHOULD_WAIT);
    }
    return zx::error(sysmem::V1CopyFromV2Error(check_buffers_domain_error));
  }

  fidl::WireResult<fuchsia_sysmem2::BufferCollection::WaitForAllBuffersAllocated>
      wait_for_buffers_transport_result = collection->WaitForAllBuffersAllocated();
  if (!wait_for_buffers_transport_result.ok()) {
    fdf::error("FIDL error calling WaitForAllBuffersAllocated: {}",
               wait_for_buffers_transport_result.error());
    return zx::error(wait_for_buffers_transport_result.status());
  }
  fit::result<fuchsia_sysmem2::wire::Error,
              fuchsia_sysmem2::wire::BufferCollectionWaitForAllBuffersAllocatedResponse*>&
      wait_for_buffers_domain_result = wait_for_buffers_transport_result.value();
  if (wait_for_buffers_domain_result.is_error()) {
    fdf::warn("WaitForAllBuffersAllocated failed: {}",
              static_cast<uint32_t>(wait_for_buffers_domain_result.error_value()));
    return zx::error(sysmem::V1CopyFromV2Error(wait_for_buffers_domain_result.error_value()));
  }
  fuchsia_sysmem2::wire::BufferCollectionInfo& collection_info =
      wait_for_buffers_domain_result.value()->buffer_collection_info();

  if (!collection_info.settings().has_image_format_constraints()) {
    fdf::error("no image format constraints");
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (buffer_index >= collection_info.buffers().size()) {
    fdf::error("invalid buffer index {}, greater than or equal to collection size {}", buffer_index,
               collection_info.buffers().size());
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  fuchsia_images2::wire::PixelFormat sysmem2_collection_format =
      collection_info.settings().image_format_constraints().pixel_format();
  if (sysmem2_collection_format != properties_.pixel_format.ToFidl()) {
    fdf::error("Image format from sysmem ({}) doesn't match expected format ({})",
               static_cast<uint32_t>(sysmem2_collection_format),
               properties_.pixel_format.ValueForLogging());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  if (image_metadata.width() != properties_.width_px ||
      image_metadata.height() != properties_.height_px) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  // Map VMO at Import to avoid mapping buffer per-frame
  zx::vmo vmo = std::move(collection_info.buffers()[buffer_index].vmo());
  fzl::VmoMapper mapper;
  zx_status_t status = mapper.Map(vmo, /*offset=*/0, /*size=*/0, ZX_VM_PERM_READ);
  if (status != ZX_OK) {
    fdf::error("Failed to map VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  const display::DriverImageId image_id = next_image_id_++;
  imported_images_[image_id] = std::move(mapper);
  return zx::ok(image_id);
}

zx::result<display::DriverCaptureImageId> FramebufferDisplay::ImportImageForCapture(
    display::DriverBufferCollectionId driver_buffer_collection_id, uint32_t index) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

void FramebufferDisplay::ReleaseImage(display::DriverImageId image_id) {
  imported_images_.erase(image_id);
}

display::ConfigCheckResult FramebufferDisplay::CheckConfiguration(
    display::DisplayId display_id, display::ModeId display_mode_id,
    std::span<const display::DriverLayer> layers) {
  ZX_DEBUG_ASSERT(display_id == kDisplayId);

  if (layers.size() > kEngineInfo.max_layer_count()) {
    return display::ConfigCheckResult::kUnsupportedConfig;
  }

  if (display_mode_id != kDisplayModeId) {
    return display::ConfigCheckResult::kUnsupportedDisplayModes;
  }

  const display::Rectangle display_area({
      .x = 0,
      .y = 0,
      .width = properties_.width_px,
      .height = properties_.height_px,
  });

  for (const display::DriverLayer& layer : layers) {
    if (layer.display_destination() != display_area) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.image_source() != layer.display_destination()) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.image_metadata().dimensions() != layer.image_source().dimensions()) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.alpha_mode() != display::AlphaMode::kDisable) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.image_source_transformation() != display::CoordinateTransformation::kIdentity) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
  }
  return display::ConfigCheckResult::kOk;
}

void FramebufferDisplay::SubmitConfiguration(display::DisplayId display_id,
                                             display::ModeId display_mode_id,
                                             std::span<const display::DriverLayer> layers,
                                             display::DriverConfigStamp config_stamp) {
  ZX_DEBUG_ASSERT(display_id == kDisplayId);
  ZX_DEBUG_ASSERT(display_mode_id == kDisplayModeId);

  ZX_DEBUG_ASSERT_MSG(layers.size() == kEngineInfo.max_layer_count(), "Invalid layer size: %zu",
                      layers.size());

  // framebuffer-display's |kEngineInfo| only supports 1 layer
  auto it = imported_images_.find(layers[0].image_id());
  if (it == imported_images_.end()) {
    fdf::error("SubmitConfiguration: unknown image ID {}", layers[0].image_id());
    return;
  }

  // Copy image buffer into the linear framebuffer. Note that the underlying framebuffer is in
  // regular RAM, so this is a bulk memory copy and not a write to a device register window.
  const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(
      PixelFormatAndModifier(properties_.pixel_format.ToFidl(), kFormatModifier));
  const size_t row_bytes = static_cast<size_t>(properties_.row_stride_px) * bytes_per_pixel;
  const size_t total_bytes = row_bytes * static_cast<size_t>(properties_.height_px);
  const size_t copy_bytes =
      std::min({total_bytes, it->second.size(), framebuffer_mmio_.get_size()});
  if (properties_.framebuffer_format != 0) {
    if (copy_bytes < total_bytes) {
      fdf::error("Framebuffer image is shorter than its declared layout");
      return;
    }
    const auto* pixels = static_cast<const uint32_t*>(it->second.start());
    for (int32_t y = 0; y < properties_.height_px; ++y) {
      const size_t row = static_cast<size_t>(y) * properties_.row_stride_px;
      for (int32_t x = 0; x < properties_.width_px; ++x) {
        conversion_row_[x] = PackBgra8888(pixels[row + x], properties_.framebuffer_format);
      }
      // This is framebuffer RAM; one bulk write avoids a memory barrier per pixel.
      framebuffer_mmio_.WriteBuffer(row * sizeof(uint32_t), conversion_row_.get(),
                                    properties_.width_px * sizeof(uint32_t));
    }
  } else {
    framebuffer_mmio_.WriteBuffer(0, it->second.start(), copy_bytes);
  }

  has_image_ = true;
  {
    std::lock_guard lock(mtx_);
    config_stamp_ = config_stamp;
  }
}

zx::result<> FramebufferDisplay::SetBufferCollectionConstraints(
    const display::ImageBufferUsage& image_buffer_usage,
    display::DriverBufferCollectionId buffer_collection_id) {
  const auto it = buffer_collections_.find(buffer_collection_id);
  if (it == buffer_collections_.end()) {
    fdf::error("SetBufferCollectionConstraints: Cannot find imported buffer collection (id={})",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& collection = it->second;

  const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(
      PixelFormatAndModifier(properties_.pixel_format.ToFidl(), kFormatModifier));
  uint32_t bytes_per_row = properties_.row_stride_px * bytes_per_pixel;

  fidl::Arena arena;
  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  auto buffer_usage = fuchsia_sysmem2::wire::BufferUsage::Builder(arena);
  buffer_usage.display(fuchsia_sysmem2::wire::kDisplayUsageLayer);
  constraints.usage(buffer_usage.Build());
  auto buffer_constraints = fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena);
  buffer_constraints.min_size_bytes(0);
  buffer_constraints.max_size_bytes(properties_.height_px * bytes_per_row);
  buffer_constraints.physically_contiguous_required(false);
  buffer_constraints.secure_required(false);
  buffer_constraints.ram_domain_supported(true);
  buffer_constraints.cpu_domain_supported(true);
  constraints.buffer_memory_constraints(buffer_constraints.Build());
  auto image_constraints = fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena);
  image_constraints.pixel_format(properties_.pixel_format.ToFidl());
  image_constraints.pixel_format_modifier(kFormatModifier);
  image_constraints.color_spaces(std::array{fuchsia_images2::ColorSpace::kSrgb});
  image_constraints.min_size({.width = static_cast<uint32_t>(properties_.width_px),
                              .height = static_cast<uint32_t>(properties_.height_px)});
  image_constraints.max_size({.width = static_cast<uint32_t>(properties_.width_px),
                              .height = static_cast<uint32_t>(properties_.height_px)});
  image_constraints.min_bytes_per_row(bytes_per_row);
  image_constraints.max_bytes_per_row(bytes_per_row);
  constraints.image_format_constraints(std::array{image_constraints.Build()});

  auto set_request = fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena);
  set_request.constraints(constraints.Build());
  fidl::OneWayStatus set_constraints_transport_status =
      collection->SetConstraints(set_request.Build());

  if (!set_constraints_transport_status.ok()) {
    fdf::error("FIDL error calling SetConstraints: {}", set_constraints_transport_status.error());
    return zx::error(set_constraints_transport_status.status());
  }

  return zx::ok();
}

zx::result<> FramebufferDisplay::SetDisplayPowerMode(display::DisplayId display_id,
                                                     display::PowerMode power_mode) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> FramebufferDisplay::StartCapture(display::DriverCaptureImageId capture_image_id) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> FramebufferDisplay::ReleaseCapture(display::DriverCaptureImageId capture_image_id) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> FramebufferDisplay::SetMinimumRgb(uint8_t minimum_rgb) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

// implement driver object:

zx::result<> FramebufferDisplay::Initialize() {
  ZX_DEBUG_ASSERT(!initialized_);

  // Start vsync loop.
  vsync_task_.Post(&dispatcher_);

  fdf::info("Initialized display, {} x {} (stride={} format={})", properties_.width_px,
            properties_.height_px, properties_.row_stride_px,
            properties_.pixel_format.ValueForLogging());

  initialized_.store(true, std::memory_order_relaxed);
  return zx::ok();
}

void FramebufferDisplay::Deinitialize() {
  if (!initialized_.exchange(false)) {
    return;
  }

  // Cancel recurring VSync task to avoid callbacks after destruction.
  vsync_task_.Cancel();
}

FramebufferDisplay::~FramebufferDisplay() { Deinitialize(); }

FramebufferDisplay::FramebufferDisplay(
    display::DisplayEngineEventsInterface* engine_events,
    fidl::WireSyncClient<fuchsia_sysmem2::Allocator> sysmem_client,
    fdf::MmioBuffer framebuffer_mmio, const DisplayProperties& properties,
    async_dispatcher_t* dispatcher)
    : sysmem_client_(std::move(sysmem_client)),
      dispatcher_(*dispatcher),
      has_image_(false),
      initialized_(false),
      framebuffer_mmio_(std::move(framebuffer_mmio)),
      properties_(properties),
      conversion_row_(properties.framebuffer_format != 0
                          ? std::make_unique<uint32_t[]>(properties.width_px)
                          : nullptr),
      next_vsync_time_(zx::clock::get_monotonic()),
      engine_events_(*engine_events) {
  ZX_DEBUG_ASSERT(dispatcher != nullptr);
  ZX_DEBUG_ASSERT(engine_events != nullptr);

  if (sysmem_client_) {
    zx_koid_t current_process_koid = GetCurrentProcessKoid();
    std::string debug_name = "framebuffer-display[" + std::to_string(current_process_koid) + "]";
    fidl::Arena arena;
    auto set_debug_request =
        fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena);
    set_debug_request.name(debug_name);
    set_debug_request.id(current_process_koid);
    fidl::OneWayStatus set_debug_client_info_transport_status =
        sysmem_client_->SetDebugClientInfo(set_debug_request.Build());
    if (!set_debug_client_info_transport_status.ok()) {
      fdf::error("FIDL error calling SetDebugClientInfo: {}",
                 set_debug_client_info_transport_status.error());
    }
  }
}

void FramebufferDisplay::OnPeriodicVSync(async_dispatcher_t* dispatcher, async::TaskBase* task,
                                         zx_status_t status) {
  if (status != ZX_OK) {
    if (status == ZX_ERR_CANCELED) {
      fdf::info("Vsync task is canceled.");
    } else {
      fdf::error("Failed to run Vsync task: {}", zx::make_result(status));
    }
    return;
  }

  display::DriverConfigStamp vsync_config_stamp;
  {
    std::lock_guard lock(mtx_);
    vsync_config_stamp = config_stamp_;
  }
  if (vsync_config_stamp != display::kInvalidDriverConfigStamp) {
    engine_events_.OnDisplayVsync(kDisplayId, next_vsync_time_, vsync_config_stamp);
  }

  next_vsync_time_ += kVSyncInterval;
  zx_status_t post_status = vsync_task_.PostForTime(&dispatcher_, next_vsync_time_);
  if (post_status != ZX_OK) {
    fdf::error("Failed to post Vsync task for the next Vsync: {}", zx::make_result(status));
  }
}

}  // namespace framebuffer_display
