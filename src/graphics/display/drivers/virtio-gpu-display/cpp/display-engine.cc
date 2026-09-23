// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/display-engine.h"

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/virtio/driver_utils.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <algorithm>
#include <array>
#include <cinttypes>
#include <cstdint>
#include <cstring>
#include <memory>
#include <span>
#include <utility>

#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>
#include <fbl/string_buffer.h>

#include "src/graphics/display/drivers/virtio-gpu-display/cpp/imported-image.h"
#include "src/graphics/display/drivers/virtio-gpu-display/cpp/virtio-gpu-device.h"
#include "src/graphics/display/drivers/virtio-gpu-display/cpp/virtio-pci-device.h"
#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/api-types/cpp/power-mode.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"
#include "src/graphics/lib/virtio/virtio-abi.h"

namespace virtio_display {

namespace {

constexpr display::EngineInfo kEngineInfo({
    .max_layer_count = 1,
    .max_connected_display_count = 1,
    .is_capture_supported = false,
});

constexpr display::Mode ToDisplayMode(const display::DisplayTiming& timing) {
  int32_t active_width = timing.horizontal_active_px;
  int32_t active_height = timing.vertical_active_lines;
  int32_t refresh_rate_millihertz = timing.vertical_field_refresh_rate_millihertz();
  return display::Mode({
      .active_width = active_width,
      .active_height = active_height,
      .refresh_rate_millihertz = refresh_rate_millihertz,
  });
}

constexpr auto kSupportedPixelFormats = std::to_array<display::PixelFormat>(
    {display::PixelFormat::kB8G8R8A8, display::PixelFormat::kR8G8B8A8});
constexpr uint32_t kRefreshRateHz = 30;
constexpr display::DisplayId kDisplayId(1);
constexpr display::ModeId kDisplayModeId(1);

}  // namespace

display::EngineInfo DisplayEngine::CompleteCoordinatorConnection() {
  engine_events_.OnDisplayAdded(kDisplayId, modes_, kSupportedPixelFormats);
  return kEngineInfo;
}

zx::result<> DisplayEngine::ImportBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
  return imported_images_.ImportBufferCollection(buffer_collection_id,
                                                 std::move(buffer_collection_token));
}

zx::result<> DisplayEngine::ReleaseBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id) {
  return imported_images_.ReleaseBufferCollection(buffer_collection_id);
}

zx::result<display::DriverImageId> DisplayEngine::ImportImage(
    const display::ImageMetadata& image_metadata,
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  if (image_metadata.tiling_type() != display::ImageTilingType::kLinear) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx::result<display::DriverImageId> image_id_result =
      imported_images_.ImportImage(buffer_collection_id, buffer_index);
  if (image_id_result.is_error()) {
    // ImportImage() already logged the error.
    return image_id_result.take_error();
  }

  const display::DriverImageId image_id = image_id_result.value();
  SysmemBufferInfo* sysmem_buffer_info = imported_images_.FindSysmemInfoById(image_id);
  ZX_DEBUG_ASSERT(sysmem_buffer_info != nullptr);

  ZX_DEBUG_ASSERT_MSG(std::ranges::find(kSupportedPixelFormats, sysmem_buffer_info->pixel_format) !=
                          kSupportedPixelFormats.end(),
                      "Sysmem negotiation resulted in unsupported pixel format: %" PRIu32,
                      sysmem_buffer_info->pixel_format.ValueForLogging());

  std::optional<virtio_abi::ResourceFormat> maybe_format =
      PixelFormatToVirtioResourceFormat(sysmem_buffer_info->pixel_format);
  if (!maybe_format) {
    fdf::error("ImportImage: unhandled pixel_format");
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  ZX_DEBUG_ASSERT(sysmem_buffer_info->pixel_format_modifier ==
                  fuchsia_images2::wire::PixelFormatModifier::kLinear);

  size_t bytes_per_pixel = sysmem_buffer_info->pixel_format.EncodingSize();
  size_t bytes_per_row = std::max<size_t>(sysmem_buffer_info->minimum_bytes_per_row,
                                          image_metadata.width() * bytes_per_pixel);
  bytes_per_row = fbl::round_up(bytes_per_row, sysmem_buffer_info->bytes_per_row_divisor);

  size_t image_size = bytes_per_row * static_cast<size_t>(image_metadata.height());

  zx::result<ImportedImage> imported_image_result = ImportedImage::Create(
      gpu_device_->bti(), sysmem_buffer_info->image_vmo, sysmem_buffer_info->image_vmo_offset,
      image_size, *maybe_format, static_cast<uint32_t>(bytes_per_row));
  if (imported_image_result.is_error()) {
    // Create() already logged the error.
    return imported_image_result.take_error();
  }

  ImportedImage* imported_image = imported_images_.FindImageById(image_id);
  ZX_DEBUG_ASSERT(imported_image != nullptr);
  *imported_image = std::move(imported_image_result).value();

  if (gpu_device_->UseBlobResource()) {
    zx::result<uint32_t> create_resource_result = gpu_device_->CreateBlobResource(
        imported_image->physical_address(),
        static_cast<uint32_t>(imported_image->RoundedUpImageSize(image_size)));
    if (create_resource_result.is_error()) {
      fdf::error("Failed to allocate blob resource: {}", create_resource_result);
      return create_resource_result.take_error();
    }

    imported_image->set_virtio_resource_id(create_resource_result.value());
  } else {
    zx::result<uint32_t> create_resource_result = gpu_device_->Create2DResource(
        image_metadata.width(), image_metadata.height(), sysmem_buffer_info->pixel_format);
    if (create_resource_result.is_error()) {
      fdf::error("Failed to allocate 2D resource: {}", create_resource_result);
      return create_resource_result.take_error();
    }

    imported_image->set_virtio_resource_id(create_resource_result.value());

    zx::result<> attach_result = gpu_device_->AttachResourceBacking(
        imported_image->virtio_resource_id(), imported_image->physical_address(), image_size);
    if (attach_result.is_error()) {
      fdf::error("Failed to attach resource backing store: {}", attach_result);
      return attach_result.take_error();
    }
  }

  return zx::ok(image_id);
}

zx::result<display::DriverCaptureImageId> DisplayEngine::ImportImageForCapture(
    display::DriverBufferCollectionId driver_buffer_collection_id, uint32_t index) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

void DisplayEngine::ReleaseImage(display::DriverImageId image_id) {
  zx::result result = imported_images_.ReleaseImage(image_id);
  if (result.is_error()) {
    // ReleaseImage() already logged the error.
    // The display coordinator API does not have error reporting for this call.
    return;
  }
}

display::ConfigCheckResult DisplayEngine::CheckConfiguration(
    display::DisplayId display_id, display::ModeId display_mode_id,
    std::span<const display::DriverLayer> layers) {
  ZX_DEBUG_ASSERT(display_id == kDisplayId);

  if (layers.size() > kEngineInfo.max_layer_count()) {
    return display::ConfigCheckResult::kUnsupportedConfig;
  }

  // Only the first mode is supported.
  if (display_mode_id != kDisplayModeId) {
    return display::ConfigCheckResult::kUnsupportedDisplayModes;
  }

  const display::Rectangle display_area({
      .x = 0,
      .y = 0,
      .width = static_cast<int32_t>(current_display_.scanout_info.geometry.width),
      .height = static_cast<int32_t>(current_display_.scanout_info.geometry.height),
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

void DisplayEngine::SubmitConfiguration(display::DisplayId display_id,
                                        display::ModeId display_mode_id,
                                        std::span<const display::DriverLayer> layers,
                                        display::DriverConfigStamp config_stamp) {
  ZX_DEBUG_ASSERT(display_id == kDisplayId);
  ZX_DEBUG_ASSERT(display_mode_id == kDisplayModeId);

  ZX_DEBUG_ASSERT_MSG(layers.size() == kEngineInfo.max_layer_count(), "Invalid layer size: %zu",
                      layers.size());
  const display::DriverImageId image_id = layers[0].image_id();
  const ImportedImage* imported_image = imported_images_.FindImageById(image_id);
  if (imported_image == nullptr) {
    fdf::error("SubmitConfiguration() used invalid image ID");
    return;
  }

  {
    fbl::AutoLock al(&flush_lock_);
    latest_framebuffer_resource_id_ = imported_image->virtio_resource_id();
    latest_framebuffer_resource_format_ = imported_image->resource_format();
    latest_framebuffer_stride_ = imported_image->stride();
    latest_config_stamp_ = config_stamp;
  }
}

zx::result<> DisplayEngine::SetBufferCollectionConstraints(
    const display::ImageBufferUsage& image_buffer_usage,
    display::DriverBufferCollectionId buffer_collection_id) {
  ImportedBufferCollection* imported_buffer_collection =
      imported_images_.FindBufferCollectionById(buffer_collection_id);
  if (imported_buffer_collection == nullptr) {
    fdf::warn("Rejected request to set constraints on BufferCollection with unknown ID: {}",
              buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  // TODO(costan): fidl::Arena may allocate memory and crash. Find a way to get
  // control over memory allocation.
  fidl::Arena arena;
  auto buffer_collection_constraints_builder =
      fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  buffer_collection_constraints_builder.usage(
      fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
          .display(fuchsia_sysmem2::wire::kDisplayUsageLayer)
          .Build());
  buffer_collection_constraints_builder.buffer_memory_constraints(
      fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
          .min_size_bytes(0)
          .max_size_bytes(std::numeric_limits<uint32_t>::max())
          .physically_contiguous_required(true)
          .secure_required(false)
          .ram_domain_supported(true)
          .cpu_domain_supported(true)
          .Build());

  static constexpr fuchsia_images2::wire::ColorSpace kColorSpaces[] = {
      fuchsia_images2::wire::ColorSpace::kSrgb};
  fuchsia_sysmem2::wire::ImageFormatConstraints
      image_format_constraints[kSupportedPixelFormats.size()];
  for (size_t i = 0; i < kSupportedPixelFormats.size(); ++i) {
    image_format_constraints[i] =
        fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena)
            .pixel_format(kSupportedPixelFormats[i].ToFidl())
            .pixel_format_modifier(fuchsia_images2::wire::PixelFormatModifier::kLinear)
            .color_spaces(kColorSpaces)
            .bytes_per_row_divisor(4)
            .Build();
  }
  buffer_collection_constraints_builder.image_format_constraints(image_format_constraints);

  fidl::OneWayStatus set_constraints_status =
      imported_buffer_collection->sysmem_client()->SetConstraints(
          fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
              .constraints(buffer_collection_constraints_builder.Build())
              .Build());
  if (!set_constraints_status.ok()) {
    fdf::error("SetConstraints() FIDL call failed: {}", set_constraints_status.status_string());
    return zx::error(set_constraints_status.status());
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

DisplayEngine::DisplayEngine(display::DisplayEngineEventsInterface* engine_events,
                             fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_client,
                             std::unique_ptr<VirtioGpuDevice> gpu_device)
    : imported_images_(std::move(sysmem_client)),
      engine_events_(*engine_events),
      gpu_device_(std::move(gpu_device)) {
  ZX_DEBUG_ASSERT(engine_events != nullptr);
  ZX_DEBUG_ASSERT(gpu_device_);
}

DisplayEngine::~DisplayEngine() = default;

// static
zx::result<std::unique_ptr<DisplayEngine>> DisplayEngine::Create(
    fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_client, zx::bti bti,
    std::unique_ptr<virtio::Backend> backend,
    display::DisplayEngineEventsInterface* engine_events) {
  zx::result<std::unique_ptr<VirtioPciDevice>> virtio_device_result =
      VirtioPciDevice::Create(std::move(bti), std::move(backend));
  if (!virtio_device_result.is_ok()) {
    // VirtioPciDevice::Create() logs on error.
    return virtio_device_result.take_error();
  }

  fbl::AllocChecker alloc_checker;
  auto gpu_device = fbl::make_unique_checked<VirtioGpuDevice>(
      &alloc_checker, std::move(virtio_device_result).value());
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for VirtioGpuDevice");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  auto display_engine = fbl::make_unique_checked<DisplayEngine>(
      &alloc_checker, engine_events, std::move(sysmem_client), std::move(gpu_device));
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngine");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  zx_status_t status = display_engine->Init();
  if (status != ZX_OK) {
    fdf::error("Failed to initialize device");
    return zx::error(status);
  }

  return zx::ok(std::move(display_engine));
}

void DisplayEngine::virtio_gpu_flusher() {
  fdf::trace("Entering VirtioGpuFlusher()");

  const bool use_blob = gpu_device_->UseBlobResource();
  fdf::info("VirtioGpuFlusher use_blob: {}", use_blob);

  uint32_t displayed_framebuffer_resource_id_ = virtio_abi::kInvalidResourceId;

  zx_instant_mono_t next_deadline = zx_clock_get_monotonic();
  zx_instant_mono_t period = ZX_SEC(1) / kRefreshRateHz;
  for (;;) {
    zx_nanosleep(next_deadline);

    bool fb_change;
    virtio_abi::ResourceFormat resource_format;
    uint32_t stride;
    {
      fbl::AutoLock al(&flush_lock_);
      fb_change = displayed_framebuffer_resource_id_ != latest_framebuffer_resource_id_;
      displayed_framebuffer_resource_id_ = latest_framebuffer_resource_id_;
      resource_format = latest_framebuffer_resource_format_;
      stride = latest_framebuffer_stride_;
      displayed_config_stamp_ = latest_config_stamp_;
    }

    fdf::trace("flushing");

    if (fb_change) {
      uint32_t resource_id = displayed_framebuffer_resource_id_ ? displayed_framebuffer_resource_id_
                                                                : virtio_abi::kInvalidResourceId;

      zx::result<> set_scanout_result;
      if (use_blob) {
        set_scanout_result =
            gpu_device_->SetScanoutBlob(current_display_.scanout_id, resource_id, resource_format,
                                        current_display_.scanout_info.geometry.width,
                                        current_display_.scanout_info.geometry.height, stride);
      } else {
        set_scanout_result = gpu_device_->SetScanoutProperties(
            current_display_.scanout_id, resource_id, current_display_.scanout_info.geometry.width,
            current_display_.scanout_info.geometry.height);
      }
      if (set_scanout_result.is_error()) {
        fdf::error("Failed to set scanout: {}", set_scanout_result);
        continue;
      }
    }

    if (displayed_framebuffer_resource_id_) {
      if (!use_blob) {
        zx::result<> transfer_result = gpu_device_->TransferToHost2D(
            displayed_framebuffer_resource_id_, current_display_.scanout_info.geometry.width,
            current_display_.scanout_info.geometry.height);
        if (transfer_result.is_error()) {
          fdf::error("Failed to transfer resource: {}", transfer_result);
          continue;
        }
      }

      zx::result<> flush_result = gpu_device_->FlushResource(
          displayed_framebuffer_resource_id_, current_display_.scanout_info.geometry.width,
          current_display_.scanout_info.geometry.height);
      if (flush_result.is_error()) {
        fdf::error("Failed to flush resource: {}", flush_result);
        continue;
      }
    }

    {
      fbl::AutoLock al(&flush_lock_);
      if (displayed_config_stamp_ != display::kInvalidDriverConfigStamp) {
        engine_events_.OnDisplayVsync(kDisplayId, zx::time_monotonic(next_deadline),
                                      displayed_config_stamp_);
      }
    }
    next_deadline = zx_time_add_duration(next_deadline, period);
  }
}

zx_status_t DisplayEngine::Start() {
  fdf::trace("Start()");

  // virtio14 5.7.5 "Device Requirements: Device Initialization"

  zx::result<fbl::Vector<DisplayInfo>> display_infos_result = gpu_device_->GetDisplayInfo();
  if (display_infos_result.is_error()) {
    fdf::error("Failed to get display info: {}", display_infos_result);
    return display_infos_result.error_value();
  }

  const DisplayInfo* current_display = FirstValidDisplay(display_infos_result.value());
  if (current_display == nullptr) {
    fdf::error("Failed to find a usable display");
    return ZX_ERR_NOT_FOUND;
  }
  current_display_ = *current_display;

  zx::result<fbl::Vector<uint8_t>> display_edid_bytes_result =
      gpu_device_->GetDisplayEdid(current_display->scanout_id);

  // EDID support is optional, and the driver can proceed without it.
  if (display_edid_bytes_result.is_ok()) {
    fbl::Vector<uint8_t> display_edid_bytes = std::move(display_edid_bytes_result).value();
    fit::result<const char*, edid::Edid> edid_result = edid::Edid::Create(display_edid_bytes);
    if (edid_result.is_error()) {
      fdf::warn("Failed to create Edid instance: {}", edid_result.error_value());
    } else {
      edid_ = std::move(edid_result).value();
    }
  }

  zx::result<fbl::Vector<display::DisplayTiming>> edid_timings_result = DisplayTimingsFromEdid();
  zx::result<fbl::Vector<display::ModeAndId>> modes_result =
      edid_timings_result.is_ok() ? DisplayModesFromEdidTimings(edid_timings_result.value())
                                  : DisplayModesFromScanoutInfo(current_display_.scanout_info);
  if (modes_result.is_error()) {
    // Errors have been logged in `DisplayModesFromEdidTimings()` and
    // `DisplayModesFromScanoutInfo()` functions above.
    return modes_result.error_value();
  }
  modes_ = std::move(modes_result).value();

  fdf::info("Found display at ({}, {}) size {}x{}, flags 0x{:08x}",
            current_display_.scanout_info.geometry.x, current_display_.scanout_info.geometry.y,
            current_display_.scanout_info.geometry.width,
            current_display_.scanout_info.geometry.height, current_display_.scanout_info.flags);
  LogEdidBytes();

  // Set the mouse cursor position to (0,0); the result is not critical.
  zx::result<uint32_t> move_cursor_result =
      gpu_device_->SetCursorPosition(current_display_.scanout_id, 0, 0);
  if (move_cursor_result.is_error()) {
    fdf::warn("Failed to move cursor: {}", move_cursor_result);
  }

  // Run a worker thread to shove in flush events
  auto virtio_gpu_flusher_entry = [](void* arg) {
    static_cast<DisplayEngine*>(arg)->virtio_gpu_flusher();
    return 0;
  };
  thrd_create(&flush_thread_, virtio_gpu_flusher_entry, this);
  thrd_detach(flush_thread_);

  fdf::trace("Start() completed");
  return ZX_OK;
}

const DisplayInfo* DisplayEngine::FirstValidDisplay(std::span<const DisplayInfo> display_infos) {
  return display_infos.empty() ? nullptr : &display_infos.front();
}

zx_status_t DisplayEngine::Init() {
  fdf::trace("Init()");

  zx::result<> imported_images_init_result = imported_images_.Initialize();
  if (imported_images_init_result.is_error()) {
    // Initialize() already logged the error.
    return imported_images_init_result.error_value();
  }

  return ZX_OK;
}

// static
zx::result<fbl::Vector<display::ModeAndId>> DisplayEngine::DisplayModesFromEdidTimings(
    std::span<const display::DisplayTiming> edid_timings) {
  fbl::Vector<display::ModeAndId> modes;
  fbl::AllocChecker alloc_checker;
  modes.reserve(edid_timings.size(), &alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for display modes");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  for (uint16_t i = 0; i < edid_timings.size(); ++i) {
    ZX_DEBUG_ASSERT_MSG(modes.size() < edid_timings.size(),
                        "The push_back() below was not supposed to allocate memory, but it might");
    modes.push_back(
        display::ModeAndId({.id = display::ModeId(i + 1), .mode = ToDisplayMode(edid_timings[i])}),
        &alloc_checker);
    ZX_DEBUG_ASSERT_MSG(alloc_checker.check(),
                        "The push_back() above failed to allocate memory; "
                        "it was not supposed to allocate at all");
  }
  return zx::ok(std::move(modes));
}

// static
zx::result<fbl::Vector<display::ModeAndId>> DisplayEngine::DisplayModesFromScanoutInfo(
    const virtio_abi::ScanoutInfo& scanout_info) {
  fbl::Vector<display::ModeAndId> modes;
  fbl::AllocChecker alloc_checker;
  modes.push_back(display::ModeAndId({
                      .id = kDisplayModeId,
                      .mode = display::Mode({
                          .active_width = static_cast<int32_t>(scanout_info.geometry.width),
                          .active_height = static_cast<int32_t>(scanout_info.geometry.height),
                          .refresh_rate_millihertz = kRefreshRateHz * 1'000,
                      }),
                  }),
                  &alloc_checker);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for display modes");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(modes));
}

zx::result<fbl::Vector<display::DisplayTiming>> DisplayEngine::DisplayTimingsFromEdid() {
  if (!edid_.has_value()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  zx::result<fbl::Vector<display::DisplayTiming>> edid_timings_result =
      edid_->GetSupportedDisplayTimings();
  if (edid_timings_result.is_error()) {
    fdf::warn("Failed to get supported display timings from EDID: {}",
              edid_timings_result.status_string());
  }
  return edid_timings_result;
}

void DisplayEngine::LogEdidBytes() {
  if (!edid_.has_value()) {
    fdf::info("EDID not available");
    return;
  }

  if constexpr (ZX_DEBUG_ASSERT_IMPLEMENTED) {
    std::span<const uint8_t> bytes(edid_->edid_bytes(), edid_->edid_length());

    // The virtio-gpu implementation in QEmu 9.2 reports a zero-padded EDID that
    // takes up the maximum buffer size in the virtio 1.4 specification.
    //
    // Trimming the trailing zeros significantly reduces the log output size.
    const size_t original_size = bytes.size();
    while (!bytes.empty() && bytes.back() == 0) {
      bytes = bytes.subspan(0, bytes.size() - 1);
    }

    fdf::info("--- BEGIN EDID DATA: {} BYTES ---", original_size);
    for (size_t line_start = 0; line_start < bytes.size();) {
      // The logger truncates lines that exceed 1,024 bytes. We pack the bytes
      // as compactly as possible, while meeting the constraint of mapping to
      // the C++ initializer syntax used in our unit tests.
      static constexpr int kMaxLoggingLineSize = 1020;
      // Each byte is logged using 6 characters -- "0xcc, ".
      static constexpr int kByteLoggingSize = 6;
      static constexpr int kMaxLineBytes = kMaxLoggingLineSize / kByteLoggingSize;
      const size_t line_byte_count = std::min<size_t>(kMaxLineBytes, bytes.size() - line_start);
      fbl::StringBuffer<1020> line;
      for (size_t i = 0; i < line_byte_count; ++i) {
        line.AppendPrintf("0x%02x, ", bytes[line_start + i]);
      }
      fdf::info("{}", line.c_str());
      line_start += line_byte_count;
    }
    fdf::info("--- END EDID DATA: {} BYTES; SKIPPED {} ZERO BYTES ---", original_size,
              original_size - bytes.size());
  } else {
    fdf::info("EDID available, uses {} bytes", edid_->edid_length());
  }
}

}  // namespace virtio_display
