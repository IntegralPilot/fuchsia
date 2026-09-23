// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/goldfish-display/display-engine.h"

#include <fidl/fuchsia.hardware.goldfish/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/time.h>
#include <lib/async/cpp/wait.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/image-format/image_format.h>
#include <lib/trace/event.h>
#include <lib/zx/result.h>

#include <algorithm>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <utility>
#include <vector>

#include <bind/fuchsia/goldfish/platform/sysmem/heap/cpp/bind.h>
#include <bind/fuchsia/sysmem/heap/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/auto_lock.h>

#include "src/graphics/display/drivers/goldfish-display/render_control.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/api-types/cpp/power-mode.h"

namespace goldfish {
namespace {

constexpr display::DisplayId kPrimaryDisplayId(1);

constexpr uint32_t FB_WIDTH = 1;
constexpr uint32_t FB_HEIGHT = 2;
constexpr uint32_t FB_FPS = 5;

constexpr uint32_t GL_RGBA = 0x1908;
constexpr uint32_t GL_BGRA_EXT = 0x80E1;

// TODO(https://fxbug.dev/42072949): The `Mode` struct does not have an ID field.
// The display coordinator requires each mode to have a unique ID. For now,
// we use a placeholder ID 1.
constexpr display::ModeId kModeId(1);

constexpr display::EngineInfo kEngineInfo({
    .max_layer_count = 1,
    .max_connected_display_count = 1,
    .is_capture_supported = false,
});

}  // namespace

DisplayEngine::DisplayEngine(fidl::ClientEnd<fuchsia_hardware_goldfish::ControlDevice> control,
                             fidl::ClientEnd<fuchsia_hardware_goldfish_pipe::Bus> pipe_bus,
                             fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_allocator,
                             std::unique_ptr<RenderControl> render_control,
                             async_dispatcher_t* display_event_dispatcher,
                             display::DisplayEngineEventsInterface* engine_events)
    : control_(std::move(control)),
      pipe_bus_(std::move(pipe_bus)),
      sysmem_allocator_client_(std::move(sysmem_allocator)),
      rc_(std::move(render_control)),
      display_event_dispatcher_(display_event_dispatcher),
      engine_events_(engine_events) {
  ZX_DEBUG_ASSERT(control_.is_valid());
  ZX_DEBUG_ASSERT(pipe_bus_.is_valid());
  ZX_DEBUG_ASSERT(sysmem_allocator_client_.is_valid());
  ZX_DEBUG_ASSERT(rc_ != nullptr);
  ZX_DEBUG_ASSERT(engine_events_ != nullptr);
}

DisplayEngine::~DisplayEngine() {}

zx::result<> DisplayEngine::Initialize() {
  fbl::AutoLock lock(&lock_);

  // Create primary display device.
  static constexpr int32_t kFallbackWidthPx = 1024;
  static constexpr int32_t kFallbackHeightPx = 768;
  static constexpr int32_t kFallbackRefreshRateHz = 60;
  primary_display_device_ = DisplayState{
      .width_px = rc_->GetFbParam(FB_WIDTH, kFallbackWidthPx),
      .height_px = rc_->GetFbParam(FB_HEIGHT, kFallbackHeightPx),
      .refresh_rate_hz = rc_->GetFbParam(FB_FPS, kFallbackRefreshRateHz),
  };

  // Set up display and set up flush task for each device.
  zx_status_t status = SetupPrimaryDisplay();
  if (status != ZX_OK) {
    fdf::error("Failed to set up the primary display: {}", zx::make_result(status));
    return zx::error(status);
  }

  status = async::PostTask(display_event_dispatcher_,
                           [this] { FlushPrimaryDisplay(display_event_dispatcher_); });
  if (status != ZX_OK) {
    fdf::error("Failed to post display flush task on the display event loop: {}",
               zx::make_result(status));
    return zx::error(status);
  }

  return zx::ok();
}

display::EngineInfo DisplayEngine::CompleteCoordinatorConnection() {
  const int32_t width = primary_display_device_.width_px;
  const int32_t height = primary_display_device_.height_px;
  const int32_t refresh_rate_hz = primary_display_device_.refresh_rate_hz;

  const display::Mode mode = {{
      .active_width = width,
      .active_height = height,
      .refresh_rate_millihertz = refresh_rate_hz * 1'000,
  }};

  const display::ModeAndId preferred_mode = {{
      .id = kModeId,
      .mode = mode,
  }};

  static constexpr std::array<display::PixelFormat, 2> pixel_formats = {
      display::PixelFormat::kB8G8R8A8,
      display::PixelFormat::kR8G8B8A8,
  };
  engine_events_->OnDisplayAdded(kPrimaryDisplayId, std::span(&preferred_mode, 1), pixel_formats);

  return kEngineInfo;
}

namespace {

uint32_t GetColorBufferFormatFromSysmemPixelFormat(
    const fuchsia_images2::PixelFormat& pixel_format) {
  switch (pixel_format) {
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
      return GL_RGBA;
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      return GL_BGRA_EXT;
    default:
      // This should not happen. The sysmem-negotiated pixel format must be supported.
      ZX_ASSERT_MSG(false, "Import unsupported image: %u", static_cast<uint32_t>(pixel_format));
  }
}

}  // namespace

zx::result<display::DriverImageId> DisplayEngine::ImportVmoImage(
    const display::ImageMetadata& image_metadata, const fuchsia_images2::PixelFormat& pixel_format,
    zx::vmo vmo, size_t offset) {
  auto color_buffer = std::make_unique<ColorBuffer>();
  color_buffer->is_linear_format =
      image_metadata.tiling_type() == display::ImageTilingType::kLinear;
  const uint32_t color_buffer_format = GetColorBufferFormatFromSysmemPixelFormat(pixel_format);

  fidl::Arena unused_arena;
  const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(
      PixelFormatAndModifier(pixel_format,
                             /* ignored */
                             fuchsia_images2::PixelFormatModifier::kLinear));
  color_buffer->size =
      fbl::round_up(image_metadata.width() * image_metadata.height() * bytes_per_pixel,
                    zx_system_get_page_size());

  // Linear images must be pinned.
  color_buffer->pinned_vmo =
      rc_->pipe_io()->PinVmo(vmo, ZX_BTI_PERM_READ | ZX_BTI_CONTIGUOUS, offset, color_buffer->size);

  color_buffer->vmo = std::move(vmo);
  color_buffer->width = image_metadata.width();
  color_buffer->height = image_metadata.height();
  color_buffer->format = color_buffer_format;

  zx::result<HostColorBufferId> create_result =
      rc_->CreateColorBuffer(image_metadata.width(), image_metadata.height(), color_buffer_format);
  if (create_result.is_error()) {
    fdf::error("failed to create color buffer: {}", create_result);
    return create_result.take_error();
  }
  color_buffer->host_color_buffer_id = create_result.value();

  // TODO(https://fxbug.dev/434967502): DisplayEngine should not directly use
  // ColorBuffer raw addresses as keys.
  const display::DriverImageId image_id(reinterpret_cast<uint64_t>(color_buffer.release()));
  return zx::ok(image_id);
}

zx::result<> DisplayEngine::ImportBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
  if (buffer_collections_.find(buffer_collection_id) != buffer_collections_.end()) {
    fdf::error("Buffer Collection (id={}) already exists", buffer_collection_id.value());
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  ZX_DEBUG_ASSERT_MSG(sysmem_allocator_client_.is_valid(), "sysmem allocator is not initialized");

  auto [collection_client_endpoint, collection_server_endpoint] =
      fidl::Endpoints<::fuchsia_sysmem2::BufferCollection>::Create();

  fidl::Arena arena;
  auto bind_result = sysmem_allocator_client_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .buffer_collection_request(std::move(collection_server_endpoint))
          .token(std::move(buffer_collection_token))
          .Build());
  if (!bind_result.ok()) {
    fdf::error("Cannot complete FIDL call BindSharedCollection: {}", bind_result.status_string());
    return zx::error(ZX_ERR_INTERNAL);
  }

  buffer_collections_[buffer_collection_id] =
      fidl::SyncClient(std::move(collection_client_endpoint));
  return zx::ok();
}

zx::result<> DisplayEngine::ReleaseBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id) {
  if (buffer_collections_.find(buffer_collection_id) == buffer_collections_.end()) {
    fdf::error("Cannot release buffer collection {}: buffer collection doesn't exist",
               buffer_collection_id);
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  buffer_collections_.erase(buffer_collection_id);
  return zx::ok();
}

zx::result<display::DriverImageId> DisplayEngine::ImportImage(
    const display::ImageMetadata& image_metadata,
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  const auto it = buffer_collections_.find(buffer_collection_id);
  if (it == buffer_collections_.end()) {
    fdf::error("ImportImage: Cannot find imported buffer collection (id={})", buffer_collection_id);
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  const fidl::SyncClient<fuchsia_sysmem2::BufferCollection>& collection_client = it->second;
  fidl::Result check_result = collection_client->CheckAllBuffersAllocated();
  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.
  if (check_result.is_error()) {
    const auto& error = check_result.error_value();
    if (error.is_domain_error() && error.domain_error() == fuchsia_sysmem2::Error::kPending) {
      return zx::error(ZX_ERR_SHOULD_WAIT);
    }
    return zx::error(ZX_ERR_UNAVAILABLE);
  }

  fidl::Result wait_result = collection_client->WaitForAllBuffersAllocated();
  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.
  if (wait_result.is_error()) {
    const auto& error = wait_result.error_value();
    if (error.is_domain_error() && error.domain_error() == fuchsia_sysmem2::Error::kPending) {
      return zx::error(ZX_ERR_SHOULD_WAIT);
    }
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  auto& wait_response = wait_result.value();
  auto& collection_info = wait_response.buffer_collection_info();

  zx::vmo vmo;
  if (buffer_index < collection_info->buffers()->size()) {
    vmo = std::move(collection_info->buffers()->at(buffer_index).vmo().value());
    ZX_DEBUG_ASSERT(!collection_info->buffers()->at(buffer_index).vmo()->is_valid());
  }

  if (!vmo.is_valid()) {
    fdf::error("invalid index: {}", buffer_index);
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  if (!collection_info->settings()->image_format_constraints()) {
    fdf::error("Buffer collection doesn't have valid image format constraints");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  uint64_t offset = collection_info->buffers()->at(buffer_index).vmo_usable_start().value();
  if (collection_info->settings()->buffer_settings()->heap().value().heap_type() !=
      bind_fuchsia_goldfish_platform_sysmem_heap::HEAP_TYPE_DEVICE_LOCAL) {
    const auto& pixel_format =
        collection_info->settings()->image_format_constraints()->pixel_format();
    return ImportVmoImage(image_metadata, pixel_format.value(), std::move(vmo), offset);
  }

  if (offset != 0) {
    fdf::error("VMO offset ({}) not supported for Goldfish device local color buffers", offset);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  auto color_buffer = std::make_unique<ColorBuffer>();
  color_buffer->is_linear_format =
      image_metadata.tiling_type() == display::ImageTilingType::kLinear;
  color_buffer->vmo = std::move(vmo);
  const display::DriverImageId image_id(reinterpret_cast<uint64_t>(color_buffer.release()));
  return zx::ok(image_id);
}

zx::result<display::DriverCaptureImageId> DisplayEngine::ImportImageForCapture(
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

void DisplayEngine::ReleaseImage(display::DriverImageId image_id) {
  auto color_buffer = reinterpret_cast<ColorBuffer*>(image_id.value());

  // Color buffer is owned by image in the linear case.
  if (color_buffer->is_linear_format) {
    rc_->CloseColorBuffer(color_buffer->host_color_buffer_id);
  }

  async::PostTask(display_event_dispatcher_, [this, color_buffer] {
    if (primary_display_device_.incoming_config.has_value() &&
        primary_display_device_.incoming_config->color_buffer == color_buffer) {
      primary_display_device_.incoming_config = std::nullopt;
    }
    delete color_buffer;
  });
}

display::ConfigCheckResult DisplayEngine::CheckConfiguration(
    display::DisplayId display_id, display::ModeId display_mode_id,
    display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers) {
  // The display coordinator currently uses zero-display configs to blank a
  // display. We'll remove this eventually.
  if (layers.empty()) {
    return display::ConfigCheckResult::kOk;
  }

  ZX_DEBUG_ASSERT(display_id == kPrimaryDisplayId);
  if (display_mode_id != kModeId) {
    return display::ConfigCheckResult::kUnsupportedDisplayModes;
  }

  if (color_conversion != display::ColorConversion::kIdentity) {
    // Color Correction is not supported, but we will pretend we do.
    // TODO(https://fxbug.dev/435550805): Returning error will cause blank screen if scenic
    // requests color correction. For now, lets pretend we support it, until a proper fix is
    // done (either from scenic or from core display)
    fdf::warn("Color Correction not supported.");
  }

  // If the number of layers exceeds the maximum supported, the rest need to be
  // merged into the base layer.
  if (layers.size() > kEngineInfo.max_layer_count()) {
    return display::ConfigCheckResult::kUnsupportedConfig;
  }

  for (const display::DriverLayer& layer : layers) {
    if (layer.image_source().width() == 0 || layer.image_source().height() == 0) {
      // Solid color fill layers are not yet supported.
      // TODO(https://fxbug.dev/406525464): add support.
      return display::ConfigCheckResult::kUnsupportedConfig;
    }

    // Scaling is allowed if destination frame match display and
    // source frame match image.
    const display::Rectangle display_area = {{
        .x = 0,
        .y = 0,
        .width = primary_display_device_.width_px,
        .height = primary_display_device_.height_px,
    }};
    const display::Rectangle image_area = {{
        .x = 0,
        .y = 0,
        .width = layer.image_metadata().width(),
        .height = layer.image_metadata().height(),
    }};
    if (layer.display_destination() != display_area) {
      // TODO(https://fxbug.dev/42111727): Need to provide proper flag to indicate driver only
      // accepts full screen dest frame.
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.image_source() != image_area) {
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.alpha_mode() != display::AlphaMode::kDisable) {
      // Alpha is not supported.
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.image_source_transformation() != display::CoordinateTransformation::kIdentity) {
      // Transformation is not supported.
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
  }

  return display::ConfigCheckResult::kOk;
}

zx_status_t DisplayEngine::PresentPrimaryDisplayConfig(const DisplayConfig& display_config) {
  ColorBuffer* color_buffer = display_config.color_buffer;
  if (!color_buffer) {
    return ZX_OK;
  }

  zx::eventpair event_display, event_sync_device;
  zx_status_t status = zx::eventpair::create(0u, &event_display, &event_sync_device);
  if (status != ZX_OK) {
    fdf::error("zx_eventpair_create failed: {}", zx::make_result(status));
    return status;
  }

  // Set up async wait for the goldfish sync event. The zx::eventpair will be
  // stored in the async wait callback, which will be destroyed only when the
  // event is signaled or the wait is cancelled.
  primary_display_device_.pending_config_waits.emplace_back(event_display.get(),
                                                            ZX_EVENTPAIR_SIGNALED, 0);
  async::WaitOnce& wait = primary_display_device_.pending_config_waits.back();

  wait.Begin(
      display_event_dispatcher_,
      [this, event = std::move(event_display), pending_config_stamp = display_config.config_stamp](
          async_dispatcher_t* dispatcher, async::WaitOnce* current_wait, zx_status_t status,
          const zx_packet_signal_t*) {
        TRACE_DURATION("gfx", "DisplayEngine::SyncEventHandler", "config_stamp",
                       pending_config_stamp.value());
        if (status == ZX_ERR_CANCELED) {
          fdf::info("Wait for config stamp {} cancelled.", pending_config_stamp);
          return;
        }
        ZX_DEBUG_ASSERT_MSG(status == ZX_OK, "Invalid wait status: %d", status);

        // When the eventpair in |current_wait| is signalled, all the pending waits
        // that are queued earlier than that eventpair will be removed from the list
        // and the async WaitOnce will be cancelled.
        // Note that the cancelled waits will return early and will not reach here.
        ZX_DEBUG_ASSERT(std::any_of(primary_display_device_.pending_config_waits.begin(),
                                    primary_display_device_.pending_config_waits.end(),
                                    [current_wait](const async::WaitOnce& wait) {
                                      return wait.object() == current_wait->object();
                                    }));
        // Remove all the pending waits that are queued earlier than the current
        // wait, and the current wait itself. In WaitOnce, the callback is moved to
        // stack before current wait is removed, so it's safe to remove any item in
        // the list.
        for (auto it = primary_display_device_.pending_config_waits.begin();
             it != primary_display_device_.pending_config_waits.end();) {
          if (it->object() == current_wait->object()) {
            primary_display_device_.pending_config_waits.erase(it);
            break;
          }
          it = primary_display_device_.pending_config_waits.erase(it);
        }
        primary_display_device_.latest_config_stamp =
            std::max(primary_display_device_.latest_config_stamp, pending_config_stamp);
      });

  // Update host-writeable display buffers before presenting.
  if (color_buffer->pinned_vmo.region_count() > 0) {
    auto status = rc_->UpdateColorBuffer(
        color_buffer->host_color_buffer_id, color_buffer->pinned_vmo, color_buffer->width,
        color_buffer->height, color_buffer->format, color_buffer->size);
    if (status.is_error() || status.value()) {
      fdf::error("color buffer update failed: {}:{}", zx::make_result(status.status_value()),
                 status.value_or(0));
      return status.is_error() ? status.status_value() : ZX_ERR_INTERNAL;
    }
  }

  // Present the buffer.
  {
    zx_status_t status = rc_->FbPost(color_buffer->host_color_buffer_id);
    if (status != ZX_OK) {
      fdf::error("Failed to call render control command FbPost: {}", zx::make_result(status));
      return status;
    }

    fbl::AutoLock lock(&lock_);
    fidl::WireResult result = control_->CreateSyncFence(std::move(event_sync_device));
    if (!result.ok()) {
      fdf::error("Failed to call FIDL CreateSyncFence: {}", result.error().FormatDescription());
      return status;
    }
    if (result.value().is_error()) {
      fdf::error("Failed to create SyncFence: {}", zx::make_result(result.value().error_value()));
      return result.value().error_value();
    }
  }

  return ZX_OK;
}

void DisplayEngine::SubmitConfiguration(display::DisplayId display_id,
                                        display::ModeId display_mode_id,
                                        display::ColorConversion color_conversion,
                                        std::span<const display::DriverLayer> layers,
                                        display::DriverConfigStamp config_stamp) {
  ZX_DEBUG_ASSERT_MSG(layers.size() == kEngineInfo.max_layer_count(), "Invalid layer size: %zu",
                      layers.size());

  display::DriverImageId driver_image_id = display::kInvalidDriverImageId;

  if (display_id == kPrimaryDisplayId) {
    if (!layers.empty()) {
      driver_image_id = layers[0].image_id();
    }
  }

  ZX_DEBUG_ASSERT(display_mode_id == kModeId);

  if (driver_image_id == display::kInvalidDriverImageId) {
    // The display doesn't have any active layers right now. For layers that
    // previously existed, we should cancel waiting events on the pending
    // color buffer and remove references to both pending and current color
    // buffers.
    async::PostTask(display_event_dispatcher_, [this, config_stamp] {
      primary_display_device_.pending_config_waits.clear();
      primary_display_device_.incoming_config = std::nullopt;
      primary_display_device_.latest_config_stamp =
          std::max(primary_display_device_.latest_config_stamp, config_stamp);
    });
    return;
  }

  ColorBuffer* color_buffer = reinterpret_cast<ColorBuffer*>(driver_image_id.value());
  ZX_DEBUG_ASSERT(color_buffer != nullptr);
  if (color_buffer->host_color_buffer_id == kInvalidHostColorBufferId) {
    zx::vmo vmo;

    zx_status_t status = color_buffer->vmo.duplicate(ZX_RIGHT_SAME_RIGHTS, &vmo);
    if (status != ZX_OK) {
      fdf::error("Failed to duplicate vmo: {}", zx::make_result(status));
      return;
    }

    {
      fbl::AutoLock lock(&lock_);

      fidl::WireResult result = control_->GetBufferHandle(std::move(vmo));
      if (!result.ok()) {
        fdf::error("Failed to call FIDL GetBufferHandle: {}", result.error().FormatDescription());
        return;
      }
      if (result.value().res != ZX_OK) {
        fdf::error("Failed to get ColorBuffer handle: {}", zx::make_result(result.value().res));
        return;
      }
      if (result.value().type != fuchsia_hardware_goldfish::BufferHandleType::kColorBuffer) {
        fdf::error("Buffer handle type invalid. Expected ColorBuffer, actual {}",
                   static_cast<uint32_t>(result.value().type));
        return;
      }

      uint32_t render_control_encoded_color_buffer_id = result.value().id;
      color_buffer->host_color_buffer_id =
          ToHostColorBufferId(render_control_encoded_color_buffer_id);

      // Color buffers are in vulkan-only mode by default as that avoids
      // unnecessary copies on the host in some cases. The color buffer
      // needs to be moved out of vulkan-only mode before being used for
      // presentation.
      if (color_buffer->host_color_buffer_id != kInvalidHostColorBufferId) {
        static constexpr uint32_t kVulkanGlSharedMode = 0;
        zx::result<RenderControl::RcResult> status = rc_->SetColorBufferVulkanMode(
            color_buffer->host_color_buffer_id, /*mode=*/kVulkanGlSharedMode);
        if (status.is_error()) {
          fdf::error("Failed to call render control SetColorBufferVulkanMode: {}", status);
        }
        if (status.value() != 0) {
          fdf::error("Render control host failed to set vulkan mode: {}", status.value());
        }
      }
    }
  }

  async::PostTask(display_event_dispatcher_, [this, config_stamp, color_buffer] {
    primary_display_device_.incoming_config = {
        .color_buffer = color_buffer,
        .config_stamp = config_stamp,
    };
  });
}

zx::result<> DisplayEngine::SetBufferCollectionConstraints(
    const display::ImageBufferUsage& image_buffer_usage,
    display::DriverBufferCollectionId buffer_collection_id) {
  const auto it = buffer_collections_.find(buffer_collection_id);
  if (it == buffer_collections_.end()) {
    fdf::error("ImportImage: Cannot find imported buffer collection (id={})", buffer_collection_id);
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  const fidl::SyncClient<fuchsia_sysmem2::BufferCollection>& collection = it->second;

  auto constraints =
      fuchsia_sysmem2::BufferCollectionConstraints()
          .usage(fuchsia_sysmem2::BufferUsage().display(fuchsia_sysmem2::kDisplayUsageLayer))
          .buffer_memory_constraints(
              fuchsia_sysmem2::BufferMemoryConstraints()
                  .min_size_bytes(0)
                  .max_size_bytes(0xFFFFFFFF)
                  .physically_contiguous_required(true)
                  .secure_required(false)
                  .ram_domain_supported(true)
                  .cpu_domain_supported(true)
                  .inaccessible_domain_supported(true)
                  .permitted_heaps(std::vector{
                      fuchsia_sysmem2::Heap()
                          .heap_type(bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM)
                          .id(0),
                      fuchsia_sysmem2::Heap()
                          .heap_type(
                              bind_fuchsia_goldfish_platform_sysmem_heap::HEAP_TYPE_DEVICE_LOCAL)
                          .id(0),

                  }));
  std::vector<fuchsia_sysmem2::ImageFormatConstraints> image_constraints_vec;
  for (uint32_t i = 0; i < 4; i++) {
    auto image_constraints =
        fuchsia_sysmem2::ImageFormatConstraints()
            .pixel_format(i & 0b01 ? fuchsia_images2::PixelFormat::kR8G8B8A8
                                   : fuchsia_images2::PixelFormat::kB8G8R8A8)

            .pixel_format_modifier(
                i & 0b10 ? fuchsia_images2::PixelFormatModifier::kLinear
                         : fuchsia_images2::PixelFormatModifier::kGoogleGoldfishOptimal)
            .color_spaces(std::vector{fuchsia_images2::ColorSpace::kSrgb})
            .min_size(fuchsia_math::SizeU().width(0).height(0))
            .max_size(fuchsia_math::SizeU().width(0xFFFFFFFF).height(0xFFFFFFFF))
            .min_bytes_per_row(0)
            .max_bytes_per_row(0xFFFFFFFF)
            .max_width_times_height(0xFFFFFFFF)
            .bytes_per_row_divisor(1)
            .start_offset_divisor(1)
            .display_rect_alignment(fuchsia_math::SizeU().width(1).height(1));
    image_constraints_vec.push_back(std::move(image_constraints));
  }
  constraints.image_format_constraints(std::move(image_constraints_vec));

  fuchsia_sysmem2::BufferCollectionSetConstraintsRequest request;
  request.constraints(std::move(constraints));
  auto set_result = collection->SetConstraints(std::move(request));
  if (set_result.is_error()) {
    fdf::error("failed to set constraints: {}", set_result.error_value().status_string());
    return zx::error(set_result.error_value().status());
  }

  return zx::ok();
}

zx::result<> DisplayEngine::SetDisplayPowerMode(display::DisplayId display_id,
                                                display::PowerMode power_mode) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> DisplayEngine::StartCapture(display::DriverCaptureImageId capture_image_id) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> DisplayEngine::ReleaseCapture(display::DriverCaptureImageId capture_image_id) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<> DisplayEngine::SetMinimumRgb(uint8_t minimum_rgb) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx_status_t DisplayEngine::SetupPrimaryDisplay() {
  // On the host render control protocol, the "invalid" host display ID is used
  // to configure the primary display device.
  const HostDisplayId kPrimaryHostDisplayId = kInvalidHostDisplayId;
  zx::result<RenderControl::RcResult> status =
      rc_->SetDisplayPose(kPrimaryHostDisplayId, /*x=*/0, /*y=*/0, primary_display_device_.width_px,
                          primary_display_device_.height_px);
  if (status.is_error()) {
    fdf::error("Failed to call render control SetDisplayPose command: {}", status);
    return status.error_value();
  }
  if (status.value() != 0) {
    fdf::error("Render control host failed to set display pose: {}", status.value());
    return ZX_ERR_INTERNAL;
  }
  primary_display_device_.expected_next_flush = async::Now(display_event_dispatcher_);

  return ZX_OK;
}

void DisplayEngine::FlushPrimaryDisplay(async_dispatcher_t* dispatcher) {
  zx::duration period = zx::sec(1) / primary_display_device_.refresh_rate_hz;
  zx::time_monotonic expected_next_flush = primary_display_device_.expected_next_flush + period;

  if (primary_display_device_.incoming_config.has_value()) {
    zx_status_t status = PresentPrimaryDisplayConfig(*primary_display_device_.incoming_config);
    ZX_DEBUG_ASSERT(status == ZX_OK || status == ZX_ERR_SHOULD_WAIT);
  }

  {
    fbl::AutoLock lock(&flush_lock_);

    if (primary_display_device_.latest_config_stamp != display::kInvalidDriverConfigStamp) {
      zx::time_monotonic now = async::Now(dispatcher);
      engine_events_->OnDisplayVsync(kPrimaryDisplayId, now,
                                     primary_display_device_.latest_config_stamp);
    }
  }

  // If we've already passed the |expected_next_flush| deadline, skip the
  // Vsync and adjust the deadline to the earliest next available frame.
  zx::time_monotonic now = async::Now(dispatcher);
  if (now > expected_next_flush) {
    expected_next_flush +=
        period * (((now - expected_next_flush + period).get() - 1L) / period.get());
  }

  primary_display_device_.expected_next_flush = expected_next_flush;
  async::PostTaskForTime(
      dispatcher, [this, dispatcher] { FlushPrimaryDisplay(dispatcher); }, expected_next_flush);
}

void DisplayEngine::SetupPrimaryDisplayForTesting(int32_t width_px, int32_t height_px,
                                                  int32_t refresh_rate_hz) {
  primary_display_device_ = {
      .width_px = width_px,
      .height_px = height_px,
      .refresh_rate_hz = refresh_rate_hz,
  };
}

}  // namespace goldfish
