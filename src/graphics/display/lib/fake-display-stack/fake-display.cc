// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/fake-display-stack/fake-display.h"

#include <fidl/fuchsia.images2/cpp/wire.h>
#include <fidl/fuchsia.math/cpp/wire.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/result.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/image-format/image_format.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/channel.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>
#include <zircon/types.h>

#include <algorithm>
#include <array>
#include <atomic>
#include <cstddef>
#include <cstdint>
#include <format>
#include <initializer_list>
#include <limits>
#include <mutex>
#include <span>
#include <string>
#include <utility>
#include <vector>

#include "src/graphics/display/lib/api-protocols/cpp/display-engine-events-interface.h"
#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/color.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/mode-and-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"
#include "src/graphics/display/lib/fake-display-stack/image-info.h"
#include "src/graphics/display/testing/software-compositor/pixel.h"
#include "src/graphics/display/testing/software-compositor/software-compositor.h"
#include "src/lib/fsl/handles/object_info.h"

namespace fake_display {

namespace {

// List of supported pixel formats.
constexpr auto kSupportedPixelFormats = std::to_array<display::PixelFormat>({
    display::PixelFormat::kB8G8R8A8,
    display::PixelFormat::kR8G8B8A8,
});
constexpr auto kSupportedFormatModifiers =
    std::to_array<fuchsia_images2::wire::PixelFormatModifier>({
        fuchsia_images2::wire::PixelFormatModifier::kLinear,
        fuchsia_images2::wire::PixelFormatModifier::kGoogleGoldfishOptimal,
    });
constexpr fuchsia_images2::wire::ColorSpace kSupportedColorSpaces[] = {
    fuchsia_images2::wire::ColorSpace::kSrgb,
};

}  // namespace

FakeDisplay::FakeDisplay(display::DisplayEngineEventsInterface* engine_events,
                         fidl::ClientEnd<fuchsia_sysmem2::Allocator> sysmem_client,
                         const FakeDisplayDeviceConfig& device_config, inspect::Inspector inspector)
    : engine_events_(*engine_events),
      device_config_(device_config),
      sysmem_client_(std::move(sysmem_client)),
      inspector_(std::move(inspector)) {
  ZX_DEBUG_ASSERT(engine_events != nullptr);
  ZX_DEBUG_ASSERT(sysmem_client_.is_valid());
  InitializeSysmemClient();

  if (device_config_.periodic_vsync) {
    vsync_thread_.emplace([](FakeDisplay* fake_display) { fake_display->VSyncThread(); }, this);
  }
  if (IsCaptureSupported()) {
    capture_thread_.emplace([](FakeDisplay* fake_display) { fake_display->CaptureThread(); }, this);
  }

  RecordDisplayConfigToInspectRootNode();
}

bool FakeDisplay::IsValidCaptureTarget(const CaptureImageInfo& capture_image) const {
  if (capture_image.metadata().dimensions() != device_config_.display_mode.active_area()) {
    fdf::error("Capture image dimension {}x{} doesn't match display size {}x{}",
               capture_image.metadata().width(), capture_image.metadata().height(),
               device_config_.display_mode.active_area().width(),
               device_config_.display_mode.active_area().height());
    return false;
  }

  switch (capture_image.sysmem_buffer_info().pixel_format) {
    case fuchsia_images2::PixelFormat::kR8G8B8A8:
    case fuchsia_images2::PixelFormat::kB8G8R8A8:
      break;
    default:
      fdf::error("Unsupported layer image pixel format: {}",
                 static_cast<uint32_t>(capture_image.sysmem_buffer_info().pixel_format));
      return false;
  }

  if (capture_image.sysmem_buffer_info().coherency_domain ==
      fuchsia_sysmem2::CoherencyDomain::kInaccessible) {
    fdf::error("the capture image cannot be on an inaccessible coherency domain");
    return false;
  }

  return true;
}

zx::result<> FakeDisplay::CompositeLayersToCaptureTargetLocked(
    std::span<display::DriverLayer> layers, const CaptureImageInfo& capture_target) {
  ZX_DEBUG_ASSERT(!layers.empty());
  ZX_DEBUG_ASSERT(IsValidCaptureTarget(capture_target));

  fzl::VmoMapper capture_target_mapper;
  zx::result map_result =
      zx::make_result(capture_target_mapper.Map(capture_target.vmo(), /*offset=*/0, /*size=*/0,
                                                /*map_flags=*/ZX_VM_PERM_READ | ZX_VM_PERM_WRITE));
  if (map_result.is_error()) {
    fdf::error("Failed to map capture target VMO: {}", map_result);
    return map_result;
  }

  const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(
      PixelFormatAndModifier(capture_target.sysmem_buffer_info().pixel_format,
                             capture_target.sysmem_buffer_info().pixel_format_modifier));
  const uint32_t stride_bytes = capture_target.sysmem_buffer_info().minimum_bytes_per_row;
  // Guaranteed by the constraints specified by
  // `SetCaptureImageFormatConstraints()`.
  ZX_DEBUG_ASSERT(stride_bytes >= capture_target.metadata().width() * bytes_per_pixel);

  software_compositor::OutputImage canvas = {
      .buffer = std::span<uint8_t>(reinterpret_cast<uint8_t*>(capture_target_mapper.start()),
                                   capture_target_mapper.size()),
      .properties = {
          .width = capture_target.metadata().width(),
          .height = capture_target.metadata().height(),
          .stride_bytes = static_cast<int>(stride_bytes),
          .pixel_format =
              software_compositor::ToPixelFormat(capture_target.sysmem_buffer_info().pixel_format),
      }};
  software_compositor::SoftwareCompositor software_compositor(canvas);

  // Retains layer image memory mappers, so that the image VMOs can be unmapped
  // until the composition finishes.
  std::vector<fzl::VmoMapper> layer_vmo_mappers;
  std::vector<software_compositor::SoftwareCompositor::LayerForComposition> composite_layers;

  composite_layers.reserve(layers.size());
  layer_vmo_mappers.reserve(layers.size());

  for (const display::DriverLayer& layer : layers) {
    display::DriverImageId driver_image_id = layer.image_id();
    if (driver_image_id == display::kInvalidDriverImageId) {
      composite_layers.push_back({
          .image = software_compositor::InputImage::kNoInputImage,
          .properties =
              {
                  .image_source = layer.image_source(),
                  .canvas_destination = layer.display_destination(),
                  .transform = layer.image_source_transformation(),
                  .alpha_mode = layer.alpha_mode(),
                  .fallback_color = layer.fallback_color(),
              },
      });
      continue;
    }

    auto layer_image = imported_images_.find(driver_image_id);
    ZX_DEBUG_ASSERT(layer_image.IsValid());

    fzl::VmoMapper layer_image_mapper;
    zx::result<> map_result =
        zx::make_result(layer_image_mapper.Map(layer_image->vmo(), /*offset=*/0, /*size=*/0,
                                               /*map_flags=*/ZX_VM_PERM_READ));
    if (map_result.is_error()) {
      fdf::error("Failed to map layer image VMO: {}", map_result);
      return map_result;
    }

    if (layer_image->sysmem_buffer_info().coherency_domain ==
        fuchsia_sysmem2::wire::CoherencyDomain::kRam) {
      zx_cache_flush(layer_image_mapper.start(), layer_image_mapper.size(),
                     ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
    }

    const uint32_t bytes_per_pixel = ImageFormatStrideBytesPerWidthPixel(
        PixelFormatAndModifier(layer_image->sysmem_buffer_info().pixel_format,
                               layer_image->sysmem_buffer_info().pixel_format_modifier));
    // We don't specify `minimum_bytes_per_row` in the fake-display's image
    // format constraints, so it's possible that the returned
    // `minimum_bytes_per_row` is less than `width * bytes_per_pixel` if none of
    // the other clients specify it in their buffer collection constraints as
    // well.
    const uint32_t stride_bytes = std::max(layer_image->sysmem_buffer_info().minimum_bytes_per_row,
                                           layer_image->metadata().width() * bytes_per_pixel);

    software_compositor::InputImage layer_image_to_composite = {
        .buffer =
            std::span<const uint8_t>(reinterpret_cast<const uint8_t*>(layer_image_mapper.start()),
                                     layer_image_mapper.size()),
        .properties = {
            .width = layer_image->metadata().width(),
            .height = layer_image->metadata().height(),
            .stride_bytes = static_cast<int>(stride_bytes),
            .pixel_format =
                software_compositor::ToPixelFormat(layer_image->sysmem_buffer_info().pixel_format),
        }};

    // Retain the VMO mapper so that the mapped image won't be unmapped until
    // the composition finishes.
    layer_vmo_mappers.push_back(std::move(layer_image_mapper));
    composite_layers.push_back({
        .image = layer_image_to_composite,
        .properties =
            {
                .image_source = layer.image_source(),
                .canvas_destination = layer.display_destination(),
                .transform = layer.image_source_transformation(),
                .alpha_mode = layer.alpha_mode(),
                .fallback_color = layer.fallback_color(),
            },
    });
  }
  software_compositor.CompositeLayers(composite_layers);

  if (capture_target.sysmem_buffer_info().coherency_domain ==
      fuchsia_sysmem2::wire::CoherencyDomain::kRam) {
    zx_cache_flush(capture_target_mapper.start(), capture_target_mapper.size(),
                   ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  }
  return zx::ok();
}

FakeDisplay::~FakeDisplay() {
  vsync_thread_shutdown_requested_.store(true, std::memory_order_relaxed);
  capture_thread_shutdown_requested_.store(true, std::memory_order_relaxed);

  if (vsync_thread_.has_value()) {
    vsync_thread_->join();
  }
  if (capture_thread_.has_value()) {
    capture_thread_->join();
  }
}

zx::result<> FakeDisplay::SetMinimumRgb(uint8_t minimum_rgb) {
  std::lock_guard lock(mutex_);

  clamp_rgb_value_ = minimum_rgb;
  return zx::ok();
}

void FakeDisplay::InitializeSysmemClient() {
  zx_koid_t koid = fsl::GetCurrentProcessKoid();

  std::string debug_name = std::format("virtio-gpu-display[{}]", koid);

  fidl::Arena arena;
  std::lock_guard lock(mutex_);
  fidl::OneWayStatus set_debug_status = sysmem_client_->SetDebugClientInfo(
      fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
          .name(fidl::StringView::FromExternal(debug_name))
          .id(koid)
          .Build());
  if (!set_debug_status.ok()) {
    // Errors here mean that the FIDL transport was not set up correctly, and
    // all future Sysmem client calls will fail. Crashing here exposes the
    // failure early.
    fdf::fatal("SetDebugClientInfo() FIDL call failed: {}", set_debug_status.status_string());
  }
}

display::EngineInfo FakeDisplay::CompleteCoordinatorConnection() {
  const display::ModeAndId mode_and_id({
      .id = device_config_.display_mode_id,
      .mode = device_config_.display_mode,
  });

  const std::span<const display::ModeAndId> preferred_modes(&mode_and_id, 1);
  engine_events_.OnDisplayAdded(device_config_.display_id, preferred_modes, kSupportedPixelFormats);
  return device_config_.engine_info;
}

namespace {

bool IsAcceptableImageTilingType(display::ImageTilingType image_tiling_type) {
  return image_tiling_type == display::ImageTilingType::kLinear;
}

}  // namespace

zx::result<> FakeDisplay::ImportBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
  std::lock_guard lock(mutex_);

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

  buffer_collections_.insert(
      buffer_collection_it,
      std::make_pair(buffer_collection_id, fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>(
                                               std::move(collection_client_endpoint))));

  return zx::ok();
}

zx::result<> FakeDisplay::ReleaseBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id) {
  std::lock_guard lock(mutex_);

  auto buffer_collection_it = buffer_collections_.find(buffer_collection_id);
  if (buffer_collection_it == buffer_collections_.end()) {
    fdf::warn("Rejected request to release BufferCollection with unknown ID: {}",
              buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  buffer_collections_.erase(buffer_collection_it);
  return zx::ok();
}

zx::result<display::DriverImageId> FakeDisplay::ImportImage(
    const display::ImageMetadata& image_metadata,
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  std::lock_guard lock(mutex_);

  auto buffer_collection_it = buffer_collections_.find(buffer_collection_id);
  if (buffer_collection_it == buffer_collections_.end()) {
    fdf::error("ImportImage: Cannot find imported buffer collection ID: {}",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& buffer_collection =
      buffer_collection_it->second;

  if (!IsAcceptableImageTilingType(image_metadata.tiling_type())) {
    fdf::info("ImportImage: Invalid image tiling type: {}",
              image_metadata.tiling_type().ValueForLogging());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  zx::result<SysmemBufferInfo> sysmem_buffer_info_result =
      SysmemBufferInfo::GetSysmemMetadata(buffer_collection, buffer_index);
  if (sysmem_buffer_info_result.is_error()) {
    // SysmemBufferInfo::GetSysmemMetadata() has already logged the error.
    return sysmem_buffer_info_result.take_error();
  }

  // TODO(https://fxbug.dev/42079320): When capture is enabled
  // (IsCaptureSupported() is true), we should perform a check to ensure that
  // the display images should not be of "inaccessible" coherency domain.

  display::DriverImageId driver_image_id = next_imported_display_driver_image_id_++;

  auto display_image_info = std::make_unique<DisplayImageInfo>(
      driver_image_id, image_metadata, std::move(sysmem_buffer_info_result).value());

  imported_images_.insert(std::move(display_image_info));
  return zx::ok(driver_image_id);
}

void FakeDisplay::ReleaseImage(display::DriverImageId image_id) {
  std::lock_guard lock(mutex_);

  auto layer_with_image_id_it = std::ranges::find_if(
      applied_layers_,
      [&](const display::DriverLayer& layer) { return layer.image_id() == image_id; });
  if (layer_with_image_id_it != applied_layers_.end()) {
    fdf::fatal("Cannot safely release an image used in currently applied configuration");
    return;
  }

  auto image_it = imported_images_.find(image_id);
  if (image_it == imported_images_.end()) {
    fdf::warn("Rejected request to release Image with unknown ID: {}", image_id.value());
    return;
  }
  imported_images_.erase(image_it);
}

display::ConfigCheckResult FakeDisplay::CheckConfiguration(
    display::DisplayId display_id, display::ModeId display_mode_id,
    std::span<const display::DriverLayer> layers) {
  ZX_DEBUG_ASSERT(display_id == device_config_.display_id);
  // `layers` is required to be non-empty by the `fuchsia.hardware.
  // display.engine/Engine` protocol.
  ZX_DEBUG_ASSERT(!layers.empty());

  if (display_mode_id != device_config_.display_mode_id) {
    return display::ConfigCheckResult::kUnsupportedDisplayModes;
  }

  // The cast does not result in UB because the maximum layer count is
  // guaranteed to be positive.
  const size_t max_layer_count = static_cast<size_t>(device_config_.engine_info.max_layer_count());
  if (layers.size() > max_layer_count) {
    fdf::error(
        "CheckConfiguration: Number of requested layers ({}) is greater "
        " than maximum supported layer count ({}).",
        layers.size(), max_layer_count);
    return display::ConfigCheckResult::kUnsupportedConfig;
  }

  const display::Rectangle display_area({
      .x = 0,
      .y = 0,
      .width = device_config_.display_mode.active_area().width(),
      .height = device_config_.display_mode.active_area().height(),
  });

  for (const display::DriverLayer& layer : layers) {
    if (layer.image_source().dimensions().IsEmpty()) {
      // Solid color fill layer.
      if (layer.fallback_color().format().EncodingSize() != sizeof(uint32_t)) {
        // The capture simulation implementation is currently optimized for 32-bit
        // colors. Removing this constraint will require updating that
        // implementation.
        return display::ConfigCheckResult::kUnsupportedConfig;
      }
    } else {
      // Image layer.
      if (layer.image_source().dimensions() != layer.display_destination().dimensions()) {
        return display::ConfigCheckResult::kUnsupportedConfig;
      }
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

void FakeDisplay::SubmitConfiguration(display::DisplayId display_id,
                                      display::ModeId display_mode_id,
                                      std::span<const display::DriverLayer> layers,
                                      display::DriverConfigStamp config_stamp) {
  ZX_DEBUG_ASSERT(display_id == device_config_.display_id);
  ZX_DEBUG_ASSERT(display_mode_id == device_config_.display_mode_id);
  ZX_DEBUG_ASSERT(config_stamp != display::kInvalidDriverConfigStamp);
  ZX_DEBUG_ASSERT(layers.size() <=
                  static_cast<size_t>(device_config_.engine_info.max_layer_count()));

  std::lock_guard lock(mutex_);

  for (const display::DriverLayer& layer : layers) {
    if (layer.image_id() != display::kInvalidDriverImageId) {
      ZX_DEBUG_ASSERT_MSG(imported_images_.find(layer.image_id()) != imported_images_.end(),
                          "Configuration contains invalid image ID: %" PRIu64,
                          layer.image_id().value());
    }
  }
  applied_layers_.assign(layers.begin(), layers.end());
  displayed_config_stamp_ = config_stamp;
}

enum class FakeDisplay::BufferCollectionUsage {
  kPrimaryLayer = 1,
  kCapture = 2,
};

namespace {

fuchsia_sysmem2::wire::BufferMemoryConstraints CreateBufferMemoryConstraints(
    fidl::AnyArena& arena) {
  return fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
      .min_size_bytes(0)
      .max_size_bytes(std::numeric_limits<uint32_t>::max())
      .physically_contiguous_required(false)
      .secure_required(false)
      .ram_domain_supported(true)
      .cpu_domain_supported(true)
      .inaccessible_domain_supported(true)
      .Build();
}

void SetLayerImageFormatConstraints(
    fidl::WireTableBuilder<fuchsia_sysmem2::wire::ImageFormatConstraints>& constraints_builder) {
  constraints_builder.min_size(fuchsia_math::wire::SizeU{.width = 0, .height = 0})
      .max_size(fuchsia_math::wire::SizeU{.width = std::numeric_limits<uint32_t>::max(),
                                          .height = std::numeric_limits<uint32_t>::max()})
      .min_bytes_per_row(0)
      .max_bytes_per_row(std::numeric_limits<uint32_t>::max())
      .max_width_times_height(std::numeric_limits<uint32_t>::max());
}

}  // namespace

void FakeDisplay::SetCaptureImageFormatConstraints(
    fidl::WireTableBuilder<fuchsia_sysmem2::wire::ImageFormatConstraints>& constraints_builder) {
  constraints_builder.min_size(device_config_.display_mode.active_area().ToFidl())
      .max_size(device_config_.display_mode.active_area().ToFidl())
      .min_bytes_per_row(device_config_.display_mode.active_area().width() * 4)
      .max_bytes_per_row(device_config_.display_mode.active_area().width() * 4)
      .max_width_times_height(device_config_.display_mode.active_area().width() *
                              device_config_.display_mode.active_area().height());
}

fuchsia_sysmem2::wire::BufferCollectionConstraints FakeDisplay::CreateBufferCollectionConstraints(
    BufferCollectionUsage usage, fidl::AnyArena& arena) {
  fidl::WireTableBuilder<fuchsia_sysmem2::wire::BufferCollectionConstraints> constraints_builder =
      fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);

  fidl::WireTableBuilder<fuchsia_sysmem2::wire::BufferUsage> usage_builder =
      fuchsia_sysmem2::wire::BufferUsage::Builder(arena);
  switch (usage) {
    case BufferCollectionUsage::kCapture:
      usage_builder.cpu(fuchsia_sysmem2::kCpuUsageReadOften | fuchsia_sysmem2::kCpuUsageWriteOften);
      break;
    case BufferCollectionUsage::kPrimaryLayer:
      if (IsCaptureSupported()) {
        usage_builder.cpu(fuchsia_sysmem2::kCpuUsageReadOften);
      }
      usage_builder.display(fuchsia_sysmem2::kDisplayUsageLayer);
      break;
  }
  constraints_builder.usage(usage_builder.Build());

  // TODO(https://fxbug.dev/42079320): In order to support capture, both capture sources
  // and capture targets must not be in the "inaccessible" coherency domain.
  constraints_builder.buffer_memory_constraints(CreateBufferMemoryConstraints(arena));

  std::vector<fuchsia_sysmem2::wire::ImageFormatConstraints> image_format_constraints;
  image_format_constraints.reserve(kSupportedPixelFormats.size() *
                                   kSupportedFormatModifiers.size());
  for (display::PixelFormat pixel_format : kSupportedPixelFormats) {
    for (fuchsia_images2::wire::PixelFormatModifier format_modifier : kSupportedFormatModifiers) {
      fidl::WireTableBuilder<fuchsia_sysmem2::wire::ImageFormatConstraints>
          image_constraints_builder = fuchsia_sysmem2::wire::ImageFormatConstraints::Builder(arena);
      image_constraints_builder.pixel_format(pixel_format.ToFidl())
          .pixel_format_modifier(format_modifier)
          .color_spaces(kSupportedColorSpaces)
          .size_alignment(fuchsia_math::wire::SizeU{.width = 1, .height = 1})
          .bytes_per_row_divisor(1)
          .start_offset_divisor(1)
          .display_rect_alignment(fuchsia_math::wire::SizeU{.width = 1, .height = 1});
      switch (usage) {
        case BufferCollectionUsage::kCapture:
          SetCaptureImageFormatConstraints(image_constraints_builder);
          break;
        case BufferCollectionUsage::kPrimaryLayer:
          SetLayerImageFormatConstraints(image_constraints_builder);
          break;
      }
      image_format_constraints.push_back(image_constraints_builder.Build());
    }
  }
  constraints_builder.image_format_constraints(image_format_constraints);

  return constraints_builder.Build();
}

zx::result<> FakeDisplay::SetBufferCollectionConstraints(
    const display::ImageBufferUsage& image_buffer_usage,
    display::DriverBufferCollectionId buffer_collection_id) {
  std::lock_guard lock(mutex_);

  const auto buffer_collection_it = buffer_collections_.find(buffer_collection_id);
  if (buffer_collection_it == buffer_collections_.end()) {
    fdf::error("SetBufferCollectionConstraints: Cannot find imported buffer collection ID: {}",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& buffer_collection =
      buffer_collection_it->second;

  BufferCollectionUsage buffer_collection_usage =
      (image_buffer_usage.tiling_type() == display::ImageTilingType::kCapture)
          ? BufferCollectionUsage::kCapture
          : BufferCollectionUsage::kPrimaryLayer;

  fidl::Arena arena;
  fidl::OneWayStatus set_constraints_status = buffer_collection->SetConstraints(
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
          .constraints(CreateBufferCollectionConstraints(buffer_collection_usage, arena))
          .Build());
  if (!set_constraints_status.ok()) {
    fdf::error("SetConstraints() FIDL call failed: {}", set_constraints_status.status_string());
    return zx::error(set_constraints_status.status());
  }

  return zx::ok();
}

zx::result<> FakeDisplay::SetDisplayPowerMode(display::DisplayId display_id,
                                              display::PowerMode power_mode) {
  return zx::error(ZX_ERR_NOT_SUPPORTED);
}

zx::result<display::DriverCaptureImageId> FakeDisplay::ImportImageForCapture(
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  if (!IsCaptureSupported()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  std::lock_guard lock(mutex_);

  auto buffer_collection_it = buffer_collections_.find(buffer_collection_id);
  if (buffer_collection_it == buffer_collections_.end()) {
    fdf::error("ImportImage: Cannot find imported buffer collection ID: {}",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& buffer_collection =
      buffer_collection_it->second;

  zx::result<SysmemBufferInfo> sysmem_buffer_info_result =
      SysmemBufferInfo::GetSysmemMetadata(buffer_collection, buffer_index);
  if (sysmem_buffer_info_result.is_error()) {
    // SysmemBufferInfo::GetSysmemMetadata() has already logged the error.
    return sysmem_buffer_info_result.take_error();
  }
  SysmemBufferInfo sysmem_buffer_info = std::move(sysmem_buffer_info_result).value();

  if (sysmem_buffer_info.coherency_domain == fuchsia_sysmem2::CoherencyDomain::kInaccessible) {
    fdf::error("the capture image cannot be on an inaccessible coherency domain");
    return zx::error(ZX_ERR_BAD_STATE);
  }

  display::ImageMetadata capture_image_metadata({
      .width = static_cast<int32_t>(sysmem_buffer_info.minimum_size.width),
      .height = static_cast<int32_t>(sysmem_buffer_info.minimum_size.height),
      .tiling_type = display::ImageTilingType::kLinear,
  });
  display::DriverCaptureImageId driver_capture_image_id = next_imported_driver_capture_image_id_++;
  auto capture_image_info = std::make_unique<CaptureImageInfo>(
      driver_capture_image_id, capture_image_metadata, std::move(sysmem_buffer_info));

  ZX_DEBUG_ASSERT_MSG(IsValidCaptureTarget(*capture_image_info),
                      "Imported capture image info is invalid");

  imported_captures_.insert(std::move(capture_image_info));
  return zx::ok(driver_capture_image_id);
}

zx::result<> FakeDisplay::StartCapture(display::DriverCaptureImageId capture_image_id) {
  if (!IsCaptureSupported()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  std::lock_guard lock(mutex_);

  if (started_capture_target_id_ != display::kInvalidDriverCaptureImageId) {
    fdf::error("Capture start request declined while a capture is already in-progress");
    return zx::error(ZX_ERR_SHOULD_WAIT);
  }

  // Confirm the handle was previously imported (hence valid)
  auto imported_capture_it = imported_captures_.find(capture_image_id);
  if (imported_capture_it == imported_captures_.end()) {
    fdf::error("Capture start request with invalid handle: {}", capture_image_id.value());
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  started_capture_target_id_ = capture_image_id;
  return zx::ok();
}

zx::result<> FakeDisplay::ReleaseCapture(display::DriverCaptureImageId capture_image_id) {
  if (!IsCaptureSupported()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  std::lock_guard lock(mutex_);

  if (started_capture_target_id_ == capture_image_id) {
    fdf::fatal("Refusing to release the target of an in-progress capture");

    // TODO(https://fxrev.dev/394954078): The return code is not meaningful. It will be
    // removed when the ReleaseCapture() error code is eliminated.
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  if (imported_captures_.erase(capture_image_id) == nullptr) {
    fdf::error("Capture release request with unused handle: {}", capture_image_id.value());

    // TODO(https://fxrev.dev/394954078): The return code is not meaningful. It will be
    // removed when the ReleaseCapture() error code is eliminated.
    return zx::error(ZX_ERR_INVALID_ARGS);
  }
  return zx::ok();
}

bool FakeDisplay::IsCaptureSupported() const {
  return device_config_.engine_info.is_capture_supported();
}

void FakeDisplay::CaptureThread() {
  ZX_DEBUG_ASSERT(IsCaptureSupported());

  while (!capture_thread_shutdown_requested_.load(std::memory_order_relaxed)) {
    [[maybe_unused]] zx::result<> capture_result = ServiceAnyCaptureRequest();
    // ServiceAnyCaptureRequest() has already logged the error.

    zx::nanosleep(
        zx::deadline_after(zx::sec(1'000) / device_config_.display_mode.refresh_rate_millihertz()));
  }
}

zx::result<> FakeDisplay::ServiceAnyCaptureRequest() {
  std::lock_guard lock(mutex_);
  if (started_capture_target_id_ == display::kInvalidDriverCaptureImageId) {
    return zx::ok();
  }

  auto imported_captures_it = imported_captures_.find(started_capture_target_id_);

  ZX_ASSERT_MSG(imported_captures_it.IsValid(),
                "Driver allowed releasing the target of an in-progress capture");
  CaptureImageInfo& capture_destination_info = *imported_captures_it;

  if (applied_layers_.empty()) {
    return zx::ok();
  }

  ZX_DEBUG_ASSERT(applied_layers_.size() > 0);
  zx::result<> composite_result =
      CompositeLayersToCaptureTargetLocked(applied_layers_, capture_destination_info);

  if (composite_result.is_error()) {
    fdf::error("Cannot composite layers to the capture target image: {}", composite_result);
    return composite_result;
  }

  engine_events_.OnCaptureComplete();
  started_capture_target_id_ = display::kInvalidDriverCaptureImageId;

  return zx::ok();
}

// static
zx::result<> FakeDisplay::DoImageCapture(DisplayImageInfo& source_info,
                                         CaptureImageInfo& destination_info) {
  if (source_info.sysmem_buffer_info().pixel_format !=
      destination_info.sysmem_buffer_info().pixel_format) {
    fdf::error("Capture will fail; trying to capture format={} as format={}\n",
               static_cast<uint32_t>(source_info.sysmem_buffer_info().pixel_format),
               static_cast<uint32_t>(destination_info.sysmem_buffer_info().pixel_format));
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  size_t source_vmo_size;
  zx_status_t status = source_info.vmo().get_size(&source_vmo_size);
  if (status != ZX_OK) {
    fdf::error("Failed to get the size of the displayed image VMO: {}", zx::make_result(status));
    return zx::error(status);
  }
  if (source_vmo_size % sizeof(uint32_t) != 0) {
    fdf::error("Capture will fail; the displayed image VMO size {} is not a 32-bit multiple",
               source_vmo_size);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  size_t destination_vmo_size;
  status = destination_info.vmo().get_size(&destination_vmo_size);
  if (status != ZX_OK) {
    fdf::error("Failed to get the size of the VMO for the captured image: {}",
               zx::make_result(status));
    return zx::error(status);
  }
  if (destination_vmo_size != source_vmo_size) {
    fdf::error(
        "Capture will fail; the displayed image VMO size {} does not match the "
        "captured image VMO size {}",
        source_vmo_size, destination_vmo_size);
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  fzl::VmoMapper source_mapper;
  status = source_mapper.Map(source_info.vmo(), 0, source_vmo_size, ZX_VM_PERM_READ);
  if (status != ZX_OK) {
    fdf::error("Capture will fail; failed to map displayed image VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  // Inline implementation of std::is_sufficiently_aligned() from C++26.
  ZX_ASSERT_MSG(std::bit_cast<std::uintptr_t>(source_mapper.start()) % sizeof(uint32_t) == 0,
                "Page size <= 32 bits; the pointer cast below will cause UB");
  std::span<const uint32_t> source_colors(static_cast<const uint32_t*>(source_mapper.start()),
                                          source_vmo_size / sizeof(uint32_t));

  fzl::VmoMapper destination_mapper;
  status = destination_mapper.Map(destination_info.vmo(), 0, destination_vmo_size,
                                  ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
  if (status != ZX_OK) {
    fdf::error("Capture will fail; failed to map capture image VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  // Inline implementation of std::is_sufficiently_aligned() from C++26.
  ZX_ASSERT_MSG(std::bit_cast<std::uintptr_t>(destination_mapper.start()) % sizeof(uint32_t) == 0,
                "Page size <= 32 bits; the pointer cast below will cause UB");
  std::span<uint32_t> destination_colors(static_cast<uint32_t*>(destination_mapper.start()),
                                         destination_vmo_size / sizeof(uint32_t));

  if (source_info.sysmem_buffer_info().coherency_domain == fuchsia_sysmem2::CoherencyDomain::kRam) {
    zx_cache_flush(source_mapper.start(), source_vmo_size,
                   ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  }
  std::ranges::copy(source_colors, destination_colors.begin());
  if (destination_info.sysmem_buffer_info().coherency_domain ==
      fuchsia_sysmem2::CoherencyDomain::kRam) {
    zx_cache_flush(destination_mapper.start(), destination_vmo_size,
                   ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  }

  return zx::ok();
}

// static
zx::result<> FakeDisplay::DoColorFillCapture(display::Color fill_color,
                                             CaptureImageInfo& destination_info) {
  // TODO(https://fxbug.dev/394954078): Capture requests issued before a
  // configuration is applied are constrained to the initial fill color format,
  // which happens to be 32-bit BGRA. This rough edge will be removed when we
  // explicitly disallow starting a capture before a config is applied.
  if (fill_color.format().ToFidl() != destination_info.sysmem_buffer_info().pixel_format) {
    fdf::error("Capture will fail; trying to capture format={} as format={}\n",
               fill_color.format().ValueForLogging(),
               static_cast<uint32_t>(destination_info.sysmem_buffer_info().pixel_format));
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  ZX_ASSERT_MSG(std::bit_cast<std::uintptr_t>(fill_color.bytes().data()) % sizeof(uint32_t) == 0,
                "Color byte buffer not 32-bit aligned; the pointer cast below will cause UB");
  const uint32_t source_color = *(reinterpret_cast<const uint32_t*>(fill_color.bytes().data()));

  size_t destination_vmo_size;
  zx_status_t status = destination_info.vmo().get_size(&destination_vmo_size);
  if (status != ZX_OK) {
    fdf::error("Failed to get the size of the VMO for the captured image: {}",
               zx::make_result(status));
    return zx::error(status);
  }
  if (destination_vmo_size % sizeof(uint32_t) != 0) {
    fdf::error("Capture will fail; the captured image VMO size {} is not a 32-bit multiple",
               destination_vmo_size);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  fzl::VmoMapper destination_mapper;
  status = destination_mapper.Map(destination_info.vmo(), 0, destination_vmo_size,
                                  ZX_VM_PERM_READ | ZX_VM_PERM_WRITE);
  if (status != ZX_OK) {
    fdf::error("Capture will fail; failed to map capture image VMO: {}", zx::make_result(status));
    return zx::error(status);
  }

  // Inline implementation of std::is_sufficiently_aligned() from C++26.
  ZX_ASSERT_MSG(std::bit_cast<std::uintptr_t>(destination_mapper.start()) % sizeof(uint32_t) == 0,
                "Page size <= 32 bits; the pointer cast below will cause UB");
  std::span<uint32_t> destination_colors(static_cast<uint32_t*>(destination_mapper.start()),
                                         destination_vmo_size / sizeof(uint32_t));

  std::ranges::fill(destination_colors, source_color);
  if (destination_info.sysmem_buffer_info().coherency_domain ==
      fuchsia_sysmem2::CoherencyDomain::kRam) {
    zx_cache_flush(destination_mapper.start(), destination_vmo_size,
                   ZX_CACHE_FLUSH_DATA | ZX_CACHE_FLUSH_INVALIDATE);
  }

  return zx::ok();
}

void FakeDisplay::TriggerVsync() {
  ZX_ASSERT_MSG(!device_config_.periodic_vsync,
                "TriggerVsync() called on a device with periodic VSync enabled");

  {
    std::lock_guard lock(mutex_);
    ZX_ASSERT_MSG(displayed_config_stamp_ != display::kInvalidDriverConfigStamp,
                  "TriggerVsync() called before the driver received a display configuration");
  }
  // The check above may appear vulnerable to TOCTOU, but it is not. Once the predicate
  // becomes true, it will never be false again.

  SendVsync();
}

void FakeDisplay::VSyncThread() {
  while (!vsync_thread_shutdown_requested_.load(std::memory_order_relaxed)) {
    SendVsync();
    zx::nanosleep(
        zx::deadline_after(zx::sec(1'000) / device_config_.display_mode.refresh_rate_millihertz()));
  }
}

void FakeDisplay::SendVsync() {
  zx::time_monotonic vsync_timestamp = zx::clock::get_monotonic();

  display::DriverConfigStamp vsync_config_stamp;
  {
    std::lock_guard lock(mutex_);
    vsync_config_stamp = displayed_config_stamp_;
  }
  if (vsync_config_stamp == display::kInvalidDriverConfigStamp) {
    // No configuration was applied yet.
    return;
  }

  engine_events_.OnDisplayVsync(device_config_.display_id, vsync_timestamp, vsync_config_stamp);
}

void FakeDisplay::RecordDisplayConfigToInspectRootNode() {
  inspect::Node& root_node = inspector_.GetRoot();
  ZX_ASSERT(root_node);
  root_node.RecordChild("device_config", [&](inspect::Node& config_node) {
    config_node.RecordInt("width_px", device_config_.display_mode.active_area().width());
    config_node.RecordInt("height_px", device_config_.display_mode.active_area().height());
    config_node.RecordDouble("refresh_rate_hz",
                             device_config_.display_mode.refresh_rate_millihertz() / 1'000.0);
    config_node.RecordBool("periodic_vsync", device_config_.periodic_vsync);
    config_node.RecordBool("is_capture_supported",
                           device_config_.engine_info.is_capture_supported());
  });
}

}  // namespace fake_display
