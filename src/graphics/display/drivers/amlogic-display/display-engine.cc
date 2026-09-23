// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/amlogic-display/display-engine.h"

#include <fidl/fuchsia.hardware.amlogiccanvas/cpp/wire.h>
#include <fidl/fuchsia.hardware.platform.device/cpp/wire.h>
#include <fidl/fuchsia.hardware.sysmem/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/fidl.h>
#include <fidl/fuchsia.math/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/fidl.h>
#include <fidl/fuchsia.sysmem2/cpp/wire.h>
#include <lib/component/incoming/cpp/constants.h>
#include <lib/ddk/metadata.h>
#include <lib/ddk/platform-defs.h>
#include <lib/device-protocol/display-panel.h>
#include <lib/driver/compat/cpp/metadata.h>
#include <lib/driver/incoming/cpp/namespace.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/function.h>
#include <lib/image-format/image_format.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/limits.h>
#include <zircon/process.h>
#include <zircon/syscalls.h>
#include <zircon/types.h>

#include <algorithm>
#include <array>
#include <cstddef>
#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <type_traits>
#include <utility>
#include <vector>

#include <bind/fuchsia/amlogic/platform/sysmem/heap/cpp/bind.h>
#include <bind/fuchsia/sysmem/heap/cpp/bind.h>
#include <fbl/algorithm.h>
#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>

#include "src/graphics/display/drivers/amlogic-display/board-resources.h"
#include "src/graphics/display/drivers/amlogic-display/capture.h"
#include "src/graphics/display/drivers/amlogic-display/hot-plug-detection.h"
#include "src/graphics/display/drivers/amlogic-display/image-info.h"
#include "src/graphics/display/drivers/amlogic-display/pixel-grid-size2d.h"
#include "src/graphics/display/drivers/amlogic-display/vout-dsi.h"
#include "src/graphics/display/drivers/amlogic-display/vout-hdmi.h"
#include "src/graphics/display/drivers/amlogic-display/vout.h"
#include "src/graphics/display/drivers/amlogic-display/vpu.h"
#include "src/graphics/display/drivers/amlogic-display/vsync-receiver.h"
#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/config-check-result.h"
#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/display-timing.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/driver-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-layer.h"
#include "src/graphics/display/lib/api-types/cpp/engine-info.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/image-tiling-type.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode.h"
#include "src/graphics/display/lib/api-types/cpp/pixel-format.h"
#include "src/graphics/display/lib/api-types/cpp/power-mode.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"
#include "src/lib/fxl/strings/string_printf.h"

namespace amlogic_display {

// Currently amlogic-display implementation uses pointers to ImageInfo as
// handles to images, while handles to images are defined as a fixed-size
// uint64_t in the banjo protocol. This works on platforms where uint64_t and
// uintptr_t are equivalent but this may cause portability issues in the future.
// TODO(https://fxbug.dev/42079128): Do not use pointers as handles.
static_assert(std::is_same_v<uint64_t, uintptr_t>);

namespace {

// TODO(https://fxbug.dev/42148348): Add more supported formats.
constexpr std::array<display::PixelFormat, 2> kSupportedPixelFormats = {
    display::PixelFormat::kB8G8R8A8,
    display::PixelFormat::kR8G8B8A8,
};

constexpr uint32_t kBufferAlignment = 64;

constexpr display::EngineInfo kEngineInfo({
    .max_layer_count = 1,
    .max_connected_display_count = 1,
    .is_capture_supported = true,
});

bool IsFormatSupported(display::PixelFormat format) {
  return std::ranges::find(kSupportedPixelFormats, format) != kSupportedPixelFormats.end();
}

void SetDefaultImageFormatConstraints(fuchsia_images2::wire::PixelFormat format, uint64_t modifier,
                                      fuchsia_sysmem2::wire::ImageFormatConstraints& constraints,
                                      fidl::AnyArena& arena) {
  constraints.Allocate(arena);
  constraints.set_color_spaces(arena, std::array{fuchsia_images2::wire::ColorSpace::kSrgb});
  constraints.set_pixel_format(format);
  constraints.set_pixel_format_modifier(arena, fuchsia_images2::PixelFormatModifier(modifier));
  constraints.set_bytes_per_row_divisor(kBufferAlignment);
  constraints.set_start_offset_divisor(kBufferAlignment);
}

ColorSpaceConversionMode GetColorSpaceConversionMode(VoutType vout_type) {
  switch (vout_type) {
    case VoutType::kDsi:
      return ColorSpaceConversionMode::kRgbInternalRgbOut;
    case VoutType::kHdmi:
      return ColorSpaceConversionMode::kRgbInternalYuvOut;
  }
  ZX_ASSERT_MSG(false, "Invalid VoutType: %u", static_cast<uint8_t>(vout_type));
}

// TODO(https://fxbug.dev/329375540): Use driver metadata instead of hardcoded
// per-board rules to determine the display engine reset policy.
bool IsFullHardwareResetRequired(
    fidl::UnownedClientEnd<fuchsia_hardware_platform_device::Device> platform_device) {
  constexpr bool kDefaultValue = true;

  zx::result<BoardInfo> board_info_result = GetBoardInfo(platform_device);
  if (board_info_result.is_error()) {
    fdf::error(
        "Failed to get board information: {}. Falling back to "
        "default option ({})",
        board_info_result, kDefaultValue);
    return kDefaultValue;
  }

  const uint32_t vendor_id = board_info_result->board_vendor_id;
  const uint32_t product_id = board_info_result->board_product_id;

  // On Khadas VIM3, the bootloader display driver may set the display engine
  // and the display device in an invalid state. We completely clear the stale
  // hardware configuration by performing a reset on the display engine.
  if (vendor_id == PDEV_VID_KHADAS && product_id == PDEV_PID_VIM3) {
    return true;
  }

  // On Astro, Sherlock and Nelson, the bootloader display driver sets up the
  // display engine and the panel. The Fuchsia driver doesn't initialize the
  // hardware registers and only loads the current hardware state.
  if (vendor_id == PDEV_VID_GOOGLE && product_id == PDEV_PID_ASTRO) {
    return false;
  }
  if (vendor_id == PDEV_VID_GOOGLE && product_id == PDEV_PID_SHERLOCK) {
    return false;
  }
  if (vendor_id == PDEV_VID_GOOGLE && product_id == PDEV_PID_NELSON) {
    return false;
  }

  fdf::info("Unknown board type (vid={}, pid={}). Falling back to default option ({}).", vendor_id,
            product_id, kDefaultValue);
  return kDefaultValue;
}

}  // namespace

zx::result<> DisplayEngine::SetMinimumRgb(uint8_t minimum_rgb) {
  if (fully_initialized()) {
    video_input_unit_->SetMinimumRgb(minimum_rgb);
    return zx::ok();
  }
  return zx::error(ZX_ERR_INTERNAL);
}

zx::result<> DisplayEngine::ResetDisplayEngine() {
  ZX_DEBUG_ASSERT(!fully_initialized());
  fdf::trace("Display engine reset started.");

  zx::result<> result = vout_->PowerOff();
  if (!result.is_ok()) {
    fdf::error("Failed to power off Vout before VPU reset: {}", result);
  }

  vpu_->PowerOff();
  vpu_->PowerOn();

  // TODO(https://fxbug.dev/42082920): Instead of enabling it ad-hoc here, make
  // `Vpu::PowerOn()` idempotent and always enable the AFBC in `Vpu::PowerOn()`.
  vpu_->AfbcPower(/*power_on=*/true);

  const ColorSpaceConversionMode color_conversion_mode = GetColorSpaceConversionMode(vout_->type());
  vpu_->SetupPostProcessorColorConversion(color_conversion_mode);
  // All the VPU registers are now reset. We need to claim the hardware
  // ownership again.
  vpu_->CheckAndClaimHardwareOwnership();

  result = vout_->PowerOn();
  if (!result.is_ok()) {
    fdf::error("Failed to power on Vout after VPU reset: {}", result);
    return result.take_error();
  }

  fdf::trace("Display engine reset finished successfully.");
  return zx::ok();
}

display::EngineInfo DisplayEngine::CompleteCoordinatorConnection() {
  fbl::AutoLock display_lock(&display_mutex_);

  if (display_attached_) {
    const AddedDisplayInfo added_display_info = vout_->CreateAddedDisplayInfo(display_id_);
    engine_events_.OnDisplayAdded(added_display_info.display_id, added_display_info.preferred_modes,
                                  kSupportedPixelFormats);
  }

  return kEngineInfo;
}

zx::result<> DisplayEngine::ImportBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> buffer_collection_token) {
  if (buffer_collections_.find(buffer_collection_id) != buffer_collections_.end()) {
    fdf::error("Buffer Collection (id={}) already exists", buffer_collection_id.value());
    return zx::error(ZX_ERR_ALREADY_EXISTS);
  }

  ZX_DEBUG_ASSERT_MSG(sysmem_.is_valid(), "sysmem allocator is not initialized");

  auto endpoints = fidl::CreateEndpoints<fuchsia_sysmem2::BufferCollection>();
  if (!endpoints.is_ok()) {
    fdf::error("Failed to create sysmem BufferCollection endpoints: {}", endpoints);
    return zx::error(ZX_ERR_INTERNAL);
  }
  auto& [collection_client_endpoint, collection_server_endpoint] = endpoints.value();

  fidl::Arena arena;
  auto bind_result = sysmem_->BindSharedCollection(
      fuchsia_sysmem2::wire::AllocatorBindSharedCollectionRequest::Builder(arena)
          .buffer_collection_request(std::move(collection_server_endpoint))
          .token(std::move(buffer_collection_token))
          .Build());
  if (!bind_result.ok()) {
    fdf::error("Failed to complete FIDL call BindSharedCollection: {}",
               bind_result.error().FormatDescription());
    return zx::error(ZX_ERR_INTERNAL);
  }

  buffer_collections_[buffer_collection_id] =
      fidl::WireSyncClient(std::move(collection_client_endpoint));
  return zx::ok();
}

zx::result<> DisplayEngine::ReleaseBufferCollection(
    display::DriverBufferCollectionId buffer_collection_id) {
  if (buffer_collections_.find(buffer_collection_id) == buffer_collections_.end()) {
    fdf::error("Failed to release buffer collection {}: buffer collection doesn't exist",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  buffer_collections_.erase(buffer_collection_id);
  return zx::ok();
}

zx::result<display::DriverImageId> DisplayEngine::ImportImage(
    const display::ImageMetadata& image_metadata,
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  if (buffer_collections_.find(buffer_collection_id) == buffer_collections_.end()) {
    fdf::error("Failed to import Image on collection {}: buffer collection doesn't exist",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  auto import_info = std::make_unique<ImageInfo>();
  if (import_info == nullptr) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  if (image_metadata.tiling_type() != display::ImageTilingType::kLinear) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& collection =
      buffer_collections_.at(buffer_collection_id);
  fidl::WireResult check_result = collection->CheckAllBuffersAllocated();
  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.
  if (!check_result.ok()) {
    return zx::error(check_result.status());
  }
  const auto& check_response = check_result.value();
  if (check_response.is_error() &&
      check_response.error_value() == fuchsia_sysmem2::Error::kPending) {
    return zx::error(ZX_ERR_SHOULD_WAIT);
  }
  if (check_response.is_error()) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }

  fidl::WireResult wait_result = collection->WaitForAllBuffersAllocated();

  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.
  if (!wait_result.ok()) {
    return zx::error(wait_result.status());
  }
  auto& wait_response = wait_result.value();
  if (wait_response.is_error()) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  fuchsia_sysmem2::wire::BufferCollectionInfo& collection_info =
      wait_response.value()->buffer_collection_info();

  if (!collection_info.settings().has_image_format_constraints() ||
      buffer_index >= collection_info.buffers().size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  const fuchsia_images2::wire::PixelFormat fidl_pixel_format =
      collection_info.settings().image_format_constraints().pixel_format();
  if (!display::PixelFormat::IsSupported(fidl_pixel_format)) {
    fdf::error("Failed to import image: pixel format {} not supported by display::PixelFormat",
               static_cast<uint32_t>(fidl_pixel_format));
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  display::PixelFormat pixel_format(fidl_pixel_format);
  if (!format_support_check_(pixel_format)) {
    fdf::error("Failed to import image: pixel format {} not supported by display engine",
               pixel_format);
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  ZX_DEBUG_ASSERT(
      collection_info.settings().image_format_constraints().has_pixel_format_modifier());

  const auto format_modifier =
      collection_info.settings().image_format_constraints().pixel_format_modifier();

  import_info->pixel_format = PixelFormatAndModifier(fidl_pixel_format, format_modifier);

  switch (format_modifier) {
    case fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuv:
    case fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTe: {
      fidl::Arena arena;
      // AFBC does not use canvas.
      uint64_t offset = collection_info.buffers().at(buffer_index).vmo_usable_start();
      const uint64_t kPageSize = zx_system_get_page_size();
      size_t size = fbl::round_up(
          ImageFormatImageSize(
              ImageConstraintsToFormat(arena, collection_info.settings().image_format_constraints(),
                                       image_metadata.dimensions().width(),
                                       image_metadata.dimensions().height())
                  .value()),
          kPageSize);
      zx_paddr_t paddr;
      zx::result<> pin_result = zx::make_result(bti_.pin(
          ZX_BTI_PERM_READ | ZX_BTI_CONTIGUOUS, collection_info.buffers().at(buffer_index).vmo(),
          offset & ~(kPageSize - 1), size, &paddr, 1, &import_info->pmt));
      if (pin_result.is_error()) {
        fdf::error("Failed to pin BTI: {}", pin_result);
        return pin_result.take_error();
      }
      import_info->paddr = paddr;
      import_info->image_height = image_metadata.dimensions().height();
      import_info->image_width = image_metadata.dimensions().width();
      import_info->is_afbc = true;
    } break;
    case fuchsia_images2::wire::PixelFormatModifier::kLinear:
    case fuchsia_images2::wire::PixelFormatModifier::kArmLinearTe: {
      uint32_t minimum_row_bytes;
      if (!ImageFormatMinimumRowBytes(collection_info.settings().image_format_constraints(),
                                      image_metadata.dimensions().width(), &minimum_row_bytes)) {
        fdf::error("Invalid image width {} for collection", image_metadata.dimensions().width());
        return zx::error(ZX_ERR_INVALID_ARGS);
      }

      fuchsia_hardware_amlogiccanvas::wire::CanvasInfo canvas_info;
      canvas_info.height = image_metadata.dimensions().height();
      canvas_info.stride_bytes = minimum_row_bytes;
      canvas_info.blkmode = fuchsia_hardware_amlogiccanvas::CanvasBlockMode::kLinear;
      canvas_info.endianness = fuchsia_hardware_amlogiccanvas::CanvasEndianness();
      canvas_info.flags = fuchsia_hardware_amlogiccanvas::CanvasFlags::kRead;

      fidl::WireResult result = canvas_->Config(
          std::move(collection_info.buffers().at(buffer_index).vmo()),
          collection_info.buffers().at(buffer_index).vmo_usable_start(), canvas_info);
      if (!result.ok()) {
        fdf::error("Failed to configure canvas: {}", result.error().FormatDescription());
        return zx::error(ZX_ERR_NO_RESOURCES);
      }
      fidl::WireResultUnwrapType<fuchsia_hardware_amlogiccanvas::Device::Config>& response =
          result.value();
      if (response.is_error()) {
        fdf::error("Failed to configure canvas: {}", zx::make_result(response.error_value()));
        return zx::error(ZX_ERR_NO_RESOURCES);
      }

      import_info->canvas = canvas_.client_end();
      import_info->canvas_idx = result->value()->canvas_idx;
      import_info->image_height = image_metadata.dimensions().height();
      import_info->image_width = image_metadata.dimensions().width();
      import_info->is_afbc = false;
    } break;
    default:
      ZX_DEBUG_ASSERT_MSG(false, "Invalid pixel format modifier: %lu",
                          static_cast<uint64_t>(format_modifier));
      return zx::error(ZX_ERR_INVALID_ARGS);
  }
  // TODO(https://fxbug.dev/42079128): Using pointers as handles impedes portability of
  // the driver. Do not use pointers as handles.
  display::DriverImageId driver_image_id(reinterpret_cast<uint64_t>(import_info.get()));
  fbl::AutoLock lock(&image_mutex_);
  imported_images_.push_back(std::move(import_info));
  return zx::ok(driver_image_id);
}

void DisplayEngine::ReleaseImage(display::DriverImageId image_id) {
  fbl::AutoLock lock(&image_mutex_);
  auto info = reinterpret_cast<ImageInfo*>(image_id.value());
  imported_images_.erase(*info);
}

display::ConfigCheckResult DisplayEngine::CheckConfiguration(
    display::DisplayId display_id, display::ModeId mode_id,
    display::ColorConversion color_conversion, std::span<const display::DriverLayer> layers) {
  fbl::AutoLock lock(&display_mutex_);

  // no-op, just wait for the client to try a new config
  if (!display_attached_ || display_id != display_id_) {
    return display::ConfigCheckResult::kOk;
  }

  // Guaranteed by DisplayEngineInterface.
  ZX_DEBUG_ASSERT(mode_id != display::kInvalidModeId);

  std::optional<display::Mode> get_display_mode_result = vout_->GetDisplayMode(mode_id);
  if (!get_display_mode_result.has_value()) {
    fdf::warn("CheckConfig failure: display mode ID {} not supported", mode_id);
    return display::ConfigCheckResult::kUnsupportedDisplayModes;
  }

  display::Mode target_display_mode = std::move(get_display_mode_result).value();

  if (layers.size() > kEngineInfo.max_layer_count()) {
    fdf::warn("CheckConfig failure: {} layers requested, we only support {}", layers.size(),
              kEngineInfo.max_layer_count());
    return display::ConfigCheckResult::kUnsupportedConfig;
  }

  // TODO(https://fxbug.dev/42080882): Move color conversion validation code to a common
  // library.
  for (float preoffset : color_conversion.preoffsets()) {
    if (preoffset <= -1 || preoffset >= 1) {
      fdf::warn("CheckConfig failure: preoffset {} out of range (-1, 1)", preoffset);
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
  }

  for (float postoffset : color_conversion.postoffsets()) {
    if (postoffset <= -1 || postoffset >= 1) {
      fdf::warn("CheckConfig failure: postoffset {} out of range (-1, 1)", postoffset);
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
  }

  // Make sure the layer configuration is supported.
  const display::Rectangle display_area({
      .x = 0,
      .y = 0,
      .width = target_display_mode.active_area().width(),
      .height = target_display_mode.active_area().height(),
  });

  for (const display::DriverLayer& layer : layers) {
    if (layer.alpha_mode() == display::AlphaMode::kPremultiplied) {
      // we don't support pre-multiplied alpha mode
      fdf::warn("CheckConfig failure: pre-multipled alpha not supported");
      return display::ConfigCheckResult::kUnsupportedConfig;
    }

    if (layer.image_source_transformation() != display::CoordinateTransformation::kIdentity) {
      fdf::warn("CheckConfig failure: coordinate transformation not supported");
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.image_metadata().dimensions().width() != target_display_mode.active_area().width() ||
        layer.image_metadata().dimensions().height() !=
            target_display_mode.active_area().height()) {
      // TODO(https://fxbug.dev/409473403): Restore to `warn` level once the infrastructure issue
      // is resolved.
      fdf::debug("CheckConfig failure: image metadata dimensions {}x{} do not match display {}",
                 layer.image_metadata().dimensions().width(),
                 layer.image_metadata().dimensions().height(), target_display_mode);
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.display_destination() != display_area) {
      fdf::warn(
          "CheckConfig failure: layer output {}x{} at ({}, {}) does not cover entire display {}",
          layer.display_destination().width(), layer.display_destination().height(),
          layer.display_destination().x(), layer.display_destination().y(), target_display_mode);
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.image_source().width() == 0 || layer.image_source().height() == 0) {
      // TODO(https://fxbug.dev/401286733): color layers not yet supported.
      fdf::warn("CheckConfig failure: color layers not supported");
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
    if (layer.image_source() != display_area) {
      fdf::warn("CheckConfig failure: image source {}x{} at ({}, {}) requires cropping or scaling",
                layer.image_source().width(), layer.image_source().height(),
                layer.image_source().x(), layer.image_source().y());
      return display::ConfigCheckResult::kUnsupportedConfig;
    }
  }

  return display::ConfigCheckResult::kOk;
}

void DisplayEngine::SubmitConfiguration(display::DisplayId display_id, display::ModeId mode_id,
                                        display::ColorConversion color_conversion,
                                        std::span<const display::DriverLayer> layers,
                                        display::DriverConfigStamp config_stamp) {
  fbl::AutoLock lock(&display_mutex_);

  ZX_DEBUG_ASSERT_MSG(layers.size() == kEngineInfo.max_layer_count(), "Invalid layer size: %zu",
                      layers.size());

  if (!layers.empty()) {
    // Perform Vout modeset first.
    //
    // Setting up OSD may require Vout framebuffer information, which may be
    // changed on each SubmitConfiguration(), so we need to apply the
    // configuration to Vout first before initializing the display and OSD.
    zx::result<> vout_apply_config_result = vout_->SubmitConfiguration(mode_id);
    if (!vout_apply_config_result.is_ok()) {
      fdf::error("Failed to apply config to Vout: {}", vout_apply_config_result);
      return;
    }

    std::optional<display::Mode> get_display_mode_result = vout_->GetDisplayMode(mode_id);
    // Guaranteed to be true in `CheckConfiguration()`.
    ZX_DEBUG_ASSERT(get_display_mode_result.has_value());
    display::Mode display_mode = std::move(get_display_mode_result).value();

    // The only way a checked configuration could now be invalid is if display was
    // unplugged. If that's the case, then the upper layers will give a new configuration
    // once they finish handling the unplug event. So just return.
    if (!display_attached_ || display_id != display_id_) {
      return;
    }
    // Since Amlogic does not support plug'n play (fixed display), there is no way
    // a checked configuration could be invalid at this point.
    video_input_unit_->FlipOnVsync(layers[0], display_mode, color_conversion, config_stamp);
  } else {
    if (fully_initialized()) {
      {
        fbl::AutoLock lock2(&capture_mutex_);
        if (current_capture_target_image_ != nullptr) {
          // there's an active capture. stop it before disabling osd
          vpu_->CaptureDone();
          current_capture_target_image_ = nullptr;
        }
      }
      video_input_unit_->DisableLayer(config_stamp);
    }
  }
}

void DisplayEngine::Deinitialize() {
  if (!fully_initialized()) {
    return;
  }
  full_init_done_.store(false, std::memory_order_release);

  vsync_receiver_.reset();

  // TODO(https://fxbug.dev/42082206): Power off should occur after all threads are
  // destroyed. Otherwise other threads may still write to the VPU MMIO which
  // can cause the system to hang.
  if (video_input_unit_) {
    video_input_unit_->Release();
  }
  if (vpu_) {
    vpu_->PowerOff();
  }

  capture_.reset();
  hot_plug_detection_.reset();
}

zx::result<> DisplayEngine::SetBufferCollectionConstraints(
    const display::ImageBufferUsage& image_buffer_usage,
    display::DriverBufferCollectionId buffer_collection_id) {
  if (buffer_collections_.find(buffer_collection_id) == buffer_collections_.end()) {
    fdf::error(
        "Failed to set buffer collection constraints for %lu: buffer collection doesn't exist",
        buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  fidl::Arena arena;
  const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& collection =
      buffer_collections_.at(buffer_collection_id);
  auto constraints = fuchsia_sysmem2::wire::BufferCollectionConstraints::Builder(arena);
  const char* buffer_name;
  if (image_buffer_usage.tiling_type() == display::ImageTilingType::kCapture) {
    constraints.usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                          .cpu(fuchsia_sysmem2::wire::kCpuUsageReadOften |
                               fuchsia_sysmem2::wire::kCpuUsageWriteOften)
                          .Build());
  } else {
    constraints.usage(fuchsia_sysmem2::wire::BufferUsage::Builder(arena)
                          .display(fuchsia_sysmem2::wire::kDisplayUsageLayer)
                          .Build());
  }
  constraints.buffer_memory_constraints(
      fuchsia_sysmem2::wire::BufferMemoryConstraints::Builder(arena)
          .physically_contiguous_required(true)
          .secure_required(false)
          .ram_domain_supported(true)
          .cpu_domain_supported(false)
          .inaccessible_domain_supported(true)
          .permitted_heaps(
              arena,
              std::vector{
                  fuchsia_sysmem2::wire::Heap::Builder(arena)
                      .heap_type(arena, bind_fuchsia_sysmem_heap::HEAP_TYPE_SYSTEM_RAM)
                      .id(0)
                      .Build(),
                  fuchsia_sysmem2::wire::Heap::Builder(arena)
                      .heap_type(arena, bind_fuchsia_amlogic_platform_sysmem_heap::HEAP_TYPE_SECURE)
                      .id(0)
                      .Build()})
          .Build());

  if (image_buffer_usage.tiling_type() == display::ImageTilingType::kCapture) {
    fuchsia_sysmem2::wire::ImageFormatConstraints image_constraints;

    SetDefaultImageFormatConstraints(
        fuchsia_images2::wire::PixelFormat::kB8G8R8,
        static_cast<uint64_t>(fuchsia_images2::wire::PixelFormatModifier::kLinear),
        image_constraints, arena);

    const PixelGridSize2D display_contents_size = video_input_unit_->display_contents_size();
    image_constraints.set_min_size(
        arena,
        fuchsia_math::wire::SizeU{.width = static_cast<uint32_t>(display_contents_size.width),
                                  .height = static_cast<uint32_t>(display_contents_size.height)});
    // Amlogic display capture engine (VDIN) outputs in formats with 3 bytes per
    // pixel.
    constexpr uint32_t kCaptureImageBytesPerPixel = 3;
    image_constraints.set_min_bytes_per_row(
        fbl::round_up(display_contents_size.width * kCaptureImageBytesPerPixel, kBufferAlignment));
    image_constraints.set_max_width_times_height(
        arena, display_contents_size.width * display_contents_size.height);
    buffer_name = "Display capture";

    constraints.image_format_constraints(arena, std::vector{image_constraints});
  } else {
    // TODO(https://fxbug.dev/42176441): Currently the buffer collection constraints are
    // applied to all displays. If the |vout_| device type changes, then the
    // existing image formats might not work for the new device type. To resolve
    // this, the driver should set per-display buffer collection constraints
    // instead.
    ZX_DEBUG_ASSERT(format_support_check_ != nullptr);
    std::vector<fuchsia_sysmem2::wire::ImageFormatConstraints> image_constraints_vec = {};
    if (format_support_check_(display::PixelFormat::kB8G8R8A8)) {
      for (const auto format_modifier :
           {fuchsia_images2::wire::PixelFormatModifier::kLinear,
            fuchsia_images2::wire::PixelFormatModifier::kArmLinearTe}) {
        fuchsia_sysmem2::wire::ImageFormatConstraints image_constraints;
        SetDefaultImageFormatConstraints(fuchsia_images2::wire::PixelFormat::kB8G8R8A8,
                                         static_cast<uint64_t>(format_modifier), image_constraints,
                                         arena);
        image_constraints_vec.push_back(image_constraints);
      }
    }
    if (format_support_check_(display::PixelFormat::kR8G8B8A8)) {
      for (const auto format_modifier :
           {fuchsia_images2::wire::PixelFormatModifier::kLinear,
            fuchsia_images2::wire::PixelFormatModifier::kArmLinearTe,
            fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuv,
            fuchsia_images2::wire::PixelFormatModifier::kArmAfbc16X16SplitBlockSparseYuvTe}) {
        fuchsia_sysmem2::wire::ImageFormatConstraints image_constraints;
        SetDefaultImageFormatConstraints(fuchsia_images2::wire::PixelFormat::kR8G8B8A8,
                                         static_cast<uint64_t>(format_modifier), image_constraints,
                                         arena);
        image_constraints_vec.push_back(image_constraints);
      }
    }
    constraints.image_format_constraints(arena, image_constraints_vec);
    buffer_name = "Display";
  }

  // Set priority to 10 to override the Vulkan driver name priority of 5, but be less than most
  // application priorities.
  constexpr uint32_t kNamePriority = 10;
  fidl::OneWayStatus set_name_result =
      collection->SetName(fuchsia_sysmem2::wire::NodeSetNameRequest::Builder(arena)
                              .priority(kNamePriority)
                              .name(fidl::StringView::FromExternal(buffer_name))
                              .Build());
  if (!set_name_result.ok()) {
    fdf::error("Failed to set name: {}", set_name_result.status());
    return zx::error(set_name_result.status());
  }
  fidl::OneWayStatus set_constraints_result = collection->SetConstraints(
      fuchsia_sysmem2::wire::BufferCollectionSetConstraintsRequest::Builder(arena)
          .constraints(constraints.Build())
          .Build());
  if (!set_constraints_result.ok()) {
    fdf::error("Failed to set constraints: {}", set_constraints_result.status());
    return zx::error(set_constraints_result.status());
  }

  return zx::ok();
}

zx::result<> DisplayEngine::SetDisplayPowerMode(display::DisplayId display_id,
                                                display::PowerMode power_mode) {
  fbl::AutoLock lock(&display_mutex_);
  if (display_id != display_id_ || !display_attached_) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  fdf::info("Setting display {} power mode to {}", display_id.value(), power_mode.ToString());
  if (power_mode != display::PowerMode::kOn && power_mode != display::PowerMode::kOff) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  bool power_on = power_mode == display::PowerMode::kOn;

  if (vout_->type() == VoutType::kHdmi) {
    // TODO(https://fxbug.dev/335303016): This is a workaround to not trigger
    // display disconnection interrupts on HDMI display power off. It doesn't
    // enable or disable the display hardware. Instead, it only enables /
    // disables the Vsync interrupt delivery and shows / hides the current
    // frame on the display.

    zx::result<> set_receiving_state_result =
        vsync_receiver_->SetReceivingState(/*receiving=*/power_on);
    if (set_receiving_state_result.is_error()) {
      fdf::error("Failed to set Vsync interrupt receiving state: {}", set_receiving_state_result);
      return set_receiving_state_result.take_error();
    }

    zx::result<> set_frame_visibility_result =
        vout_->SetFrameVisibility(/*frame_visible=*/power_on);
    if (set_frame_visibility_result.is_error()) {
      fdf::error("Failed to set frame visibility: {}", set_frame_visibility_result);
      return set_frame_visibility_result.take_error();
    }
    return zx::ok();
  }

  ZX_DEBUG_ASSERT(vout_->type() == VoutType::kDsi);
  if (power_on) {
    return vout_->PowerOn();
  }
  return vout_->PowerOff();
}

zx::result<display::DriverCaptureImageId> DisplayEngine::ImportImageForCapture(
    display::DriverBufferCollectionId buffer_collection_id, uint32_t buffer_index) {
  if (buffer_collections_.find(buffer_collection_id) == buffer_collections_.end()) {
    fdf::error("Failed to import capture image on collection {}: buffer collection doesn't exist",
               buffer_collection_id.value());
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  const fidl::WireSyncClient<fuchsia_sysmem2::BufferCollection>& collection =
      buffer_collections_.at(buffer_collection_id);
  auto import_capture = std::make_unique<ImageInfo>();
  if (import_capture == nullptr) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  fbl::AutoLock lock(&capture_mutex_);
  fidl::WireResult check_result = collection->CheckAllBuffersAllocated();
  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.
  if (!check_result.ok()) {
    return zx::error(check_result.status());
  }
  const auto& check_response = check_result.value();
  if (check_response.is_error() &&
      check_response.error_value() == fuchsia_sysmem2::Error::kPending) {
    return zx::error(ZX_ERR_SHOULD_WAIT);
  }
  if (check_response.is_error()) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }

  fidl::WireResult wait_result = collection->WaitForAllBuffersAllocated();
  // TODO(https://fxbug.dev/42072690): The sysmem FIDL error logging patterns are
  // inconsistent across drivers. The FIDL error handling and logging should be
  // unified.
  if (!wait_result.ok()) {
    return zx::error(wait_result.status());
  }
  auto& wait_response = wait_result.value();
  if (wait_response.is_error()) {
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  fuchsia_sysmem2::wire::BufferCollectionInfo collection_info =
      wait_response->buffer_collection_info();

  if (!collection_info.settings().has_image_format_constraints() ||
      buffer_index >= collection_info.buffers().size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  // Ensure the proper format
  ZX_DEBUG_ASSERT(collection_info.settings().image_format_constraints().pixel_format() ==
                  fuchsia_images2::wire::PixelFormat::kB8G8R8);

  // Allocate a canvas for the capture image
  fuchsia_hardware_amlogiccanvas::wire::CanvasInfo canvas_info;
  canvas_info.height = collection_info.settings().image_format_constraints().min_size().height;
  canvas_info.stride_bytes =
      collection_info.settings().image_format_constraints().min_bytes_per_row();
  canvas_info.blkmode = fuchsia_hardware_amlogiccanvas::CanvasBlockMode::kLinear;

  // Canvas images are by default little-endian for each 128-bit (16-byte)
  // chunk. By default, for 8-bit YUV444 images, the pixels are interpreted as
  //   Y0  U0  V0  Y1  U1  V1  Y2  U2    V2  Y3  U3  V3  Y4  U4  V4  Y5...
  //
  // However, capture memory interface uses big-endian for each 128-bit
  // (16-byte) chunk (defined in Vpu::CaptureInit), and the high- and low-64
  // bits (8 bytes) are already swapped. This is effectively big-endian for
  // each 64-bit chunk. So, the 8-bit YUV444 pixels are stored by the capture
  // memory interface as
  //   U2  Y2  V1  U1  Y1  V0  U0  Y0    Y5  V4  U4  Y4  V3  U3  Y3  V2...
  //
  // In order to read / write the captured canvas image correctly, the canvas
  // endianness must match that of capture memory interface.
  //
  // To convert 128-bit little-endian to 64-bit big-endian, we need to swap
  // every 8-bit pairs, 16-bit pairs and 32-bit pairs within every 64-bit chunk:
  //
  //   The original bytes written by the capture memory interface:
  //     U2  Y2  V1  U1  Y1  V0  U0  Y0    Y5  V4  U4  Y4  V3  U3  Y3  V2...
  //   Swapping every 8-bit pairs we get:
  //     Y2  U2  U1  V1  V0  Y1  Y0  U0    V4  Y5  Y4  U4  U3  V3  V2  Y3...
  //   Then we swap every 16-bit pairs:
  //     U1  V1  Y2  U2  Y0  U0  V0  Y1    Y4  U4  V4  Y5  V2  Y3  U3  V3...
  //   Then we swap every 32-bit pairs:
  //     Y0  U0  V0  Y1  U1  V1  Y2  U2    V2  Y3  U3  V3  Y4  U4  V4  Y5...
  //   Then we got the correct pixel interpretation.
  constexpr fuchsia_hardware_amlogiccanvas::wire::CanvasEndianness kCanvasBigEndian64Bit =
      fuchsia_hardware_amlogiccanvas::wire::CanvasEndianness::kSwap8BitPairs |
      fuchsia_hardware_amlogiccanvas::wire::CanvasEndianness::kSwap16BitPairs |
      fuchsia_hardware_amlogiccanvas::wire::CanvasEndianness::kSwap32BitPairs;

  canvas_info.endianness = fuchsia_hardware_amlogiccanvas::CanvasEndianness(kCanvasBigEndian64Bit);
  canvas_info.flags = fuchsia_hardware_amlogiccanvas::CanvasFlags::kRead |
                      fuchsia_hardware_amlogiccanvas::CanvasFlags::kWrite;

  fidl::WireResult result =
      canvas_->Config(std::move(collection_info.buffers().at(buffer_index).vmo()),
                      collection_info.buffers().at(buffer_index).vmo_usable_start(), canvas_info);
  if (!result.ok()) {
    fdf::error("Failed to configure canvas: {}", result.error().FormatDescription());
    return zx::error(ZX_ERR_NO_RESOURCES);
  }
  fidl::WireResultUnwrapType<fuchsia_hardware_amlogiccanvas::Device::Config>& response =
      result.value();
  if (response.is_error()) {
    fdf::error("Failed to configure canvas: {}", zx::make_result(response.error_value()));
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  // At this point, we have setup a canvas with the BufferCollection-based VMO. Store the
  // capture information
  //
  // TODO(https://fxbug.dev/42082204): Currently there's no guarantee in the canvas API
  // for the uniqueness of `canvas_idx`, and this driver doesn't check if there
  // is any image with the same canvas index either. We should either make this
  // a formal guarantee in Canvas.Config() API, or perform a check against all
  // imported images to make sure the canvas is unique so that the driver won't
  // overwrite other images.
  import_capture->canvas_idx = result->value()->canvas_idx;
  import_capture->canvas = canvas_.client_end();
  import_capture->image_height =
      collection_info.settings().image_format_constraints().min_size().height;
  import_capture->image_width =
      collection_info.settings().image_format_constraints().min_size().width;
  // TODO(https://fxbug.dev/42079128): Using pointers as handles impedes portability of
  // the driver. Do not use pointers as handles.
  display::DriverCaptureImageId driver_capture_image_id(
      reinterpret_cast<uint64_t>(import_capture.get()));
  imported_captures_.push_back(std::move(import_capture));

  return zx::ok(driver_capture_image_id);
}

zx::result<> DisplayEngine::StartCapture(display::DriverCaptureImageId capture_image_id) {
  if (!fully_initialized()) {
    fdf::error("Failed to start capture before initializing the display");
    return zx::error(ZX_ERR_SHOULD_WAIT);
  }

  fbl::AutoLock lock(&capture_mutex_);
  if (current_capture_target_image_ != nullptr) {
    fdf::error("Failed to start capture while another capture is in progress");
    return zx::error(ZX_ERR_SHOULD_WAIT);
  }

  // Confirm that the handle was previously imported (hence valid)
  // TODO(https://fxbug.dev/42079128): This requires an enumeration over all the imported
  // capture images for each StartCapture(). We should use hash maps to map
  // handles (which shouldn't be pointers) to ImageInfo instead.
  ImageInfo* info = reinterpret_cast<ImageInfo*>(capture_image_id.value());
  uint8_t canvas_index = info->canvas_idx;
  if (imported_captures_.find_if([canvas_index](const ImageInfo& info) {
        return info.canvas_idx == canvas_index;
      }) == imported_captures_.end()) {
    // invalid handle
    fdf::error("Invalid capture image ID: {}", capture_image_id);
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  // TODO(https://fxbug.dev/42082204): A valid canvas index can be zero.
  ZX_DEBUG_ASSERT(info->canvas_idx > 0);
  ZX_DEBUG_ASSERT(info->image_height > 0);
  ZX_DEBUG_ASSERT(info->image_width > 0);

  auto status = vpu_->CaptureInit(info->canvas_idx, info->image_height, info->image_width);
  if (status != ZX_OK) {
    fdf::error("Failed to init capture: {}", zx::make_result(status));
    return zx::make_result(status);
  }

  status = vpu_->CaptureStart();
  if (status != ZX_OK) {
    fdf::error("Failed to start capture: {}", zx::make_result(status));
    return zx::make_result(status);
  }
  current_capture_target_image_ = info;
  return zx::ok();
}

zx::result<> DisplayEngine::ReleaseCapture(display::DriverCaptureImageId capture_image_id) {
  fbl::AutoLock lock(&capture_mutex_);
  if (capture_image_id ==
      display::DriverCaptureImageId(reinterpret_cast<uint64_t>(current_capture_target_image_))) {
    return zx::error(ZX_ERR_SHOULD_WAIT);
  }

  // Find and erase previously imported capture
  // TODO(https://fxbug.dev/42079128): This requires an enumeration over all the imported
  // capture images for each StartCapture(). We should use hash maps to map
  // handles (which shouldn't be pointers) to ImageInfo instead.
  uint8_t canvas_index = reinterpret_cast<ImageInfo*>(capture_image_id.value())->canvas_idx;
  if (imported_captures_.erase_if(
          [canvas_index](const ImageInfo& i) { return i.canvas_idx == canvas_index; }) == nullptr) {
    fdf::error("Tried to release non-existent capture image {}", canvas_index);
    return zx::error(ZX_ERR_NOT_FOUND);
  }

  return zx::ok();
}

void DisplayEngine::OnVsync(zx::time_monotonic timestamp) {
  display::DriverConfigStamp current_config_stamp = display::kInvalidDriverConfigStamp;
  if (fully_initialized()) {
    current_config_stamp = video_input_unit_->GetLastConfigStampApplied();
  }
  if (current_config_stamp == display::kInvalidDriverConfigStamp) {
    return;
  }

  fbl::AutoLock lock(&display_mutex_);
  if (display_attached_) {
    engine_events_.OnDisplayVsync(display_id_, timestamp, current_config_stamp);
  }
}

void DisplayEngine::OnCaptureComplete() {
  if (!fully_initialized()) {
    fdf::warn("Capture interrupt fired before the display was initialized");
    return;
  }

  vpu_->CaptureDone();
  {
    fbl::AutoLock display_lock(&display_mutex_);
    engine_events_.OnCaptureComplete();
  }
  {
    fbl::AutoLock capture_lock(&capture_mutex_);
    current_capture_target_image_ = nullptr;
  }
}

void DisplayEngine::OnHotPlugStateChange(HotPlugDetectionState current_state) {
  fbl::AutoLock lock(&display_mutex_);

  if (current_state == HotPlugDetectionState::kDetected && !display_attached_) {
    fdf::info("Display is connected");

    display_attached_ = true;

    zx::result<> update_vout_display_state_result = vout_->UpdateStateOnDisplayConnected();
    if (update_vout_display_state_result.is_error()) {
      fdf::error("Failed to update Vout display state: {}", update_vout_display_state_result);
      return;
    }

    const AddedDisplayInfo added_display_info = vout_->CreateAddedDisplayInfo(display_id_);
    engine_events_.OnDisplayAdded(added_display_info.display_id, added_display_info.preferred_modes,
                                  kSupportedPixelFormats);
    return;
  }

  if (current_state == HotPlugDetectionState::kNotDetected && display_attached_) {
    fdf::info("Display Disconnected!");
    vout_->DisplayDisconnected();

    const display::DisplayId removed_display_id = display_id_;
    display_id_++;
    display_attached_ = false;

    engine_events_.OnDisplayRemoved(removed_display_id);
    return;
  }
}

zx_status_t DisplayEngine::SetupHotplugDisplayDetection() {
  ZX_DEBUG_ASSERT_MSG(!hot_plug_detection_, "HPD already set up");

  zx::result<std::unique_ptr<HotPlugDetection>> hot_plug_detection_result =
      HotPlugDetection::Create(*incoming_,
                               fit::bind_member<&DisplayEngine::OnHotPlugStateChange>(this));

  if (hot_plug_detection_result.is_error()) {
    // HotPlugDetection::Create() logged the error.
    return hot_plug_detection_result.status_value();
  }
  hot_plug_detection_ = std::move(hot_plug_detection_result).value();
  return ZX_OK;
}

zx_status_t DisplayEngine::InitializeVout() {
  ZX_ASSERT(vout_ == nullptr);

  zx::result<std::unique_ptr<display::PanelType>> metadata_result =
      compat::GetMetadata<display::PanelType>(incoming_, DEVICE_METADATA_DISPLAY_PANEL_TYPE,
                                              component::kDefaultInstance);
  if (metadata_result.is_ok()) {
    display::PanelType panel_type = *std::move(metadata_result).value();
    fdf::info("Provided panel type: {}", static_cast<uint32_t>(panel_type));
    fbl::AutoLock lock(&display_mutex_);
    zx::result<std::unique_ptr<VoutDsi>> dsi_vout_result =
        VoutDsi::Create(*incoming_, panel_type, root_node_.CreateChild("vout"));
    if (dsi_vout_result.is_error()) {
      fdf::error("Failed to create DSI Vout: %s", dsi_vout_result.status_string());
      return dsi_vout_result.status_value();
    }
    vout_ = std::move(dsi_vout_result).value();
    display_attached_ = true;
    return ZX_OK;
  }

  if (metadata_result.status_value() == ZX_ERR_NOT_FOUND) {
    zx::result<std::unique_ptr<VoutHdmi>> hdmi_vout_result = VoutHdmi::Create(
        *incoming_, root_node_.CreateChild("vout"), structured_config_.visual_debugging_level());
    if (hdmi_vout_result.is_error()) {
      fdf::error("Failed to create HDMI Vout: %s", hdmi_vout_result.status_string());
      return hdmi_vout_result.status_value();
    }
    vout_ = std::move(hdmi_vout_result).value();
    return ZX_OK;
  }

  fdf::error("Failed to get display panel metadata: {}", metadata_result);
  return metadata_result.status_value();
}

zx_status_t DisplayEngine::GetCommonProtocolsAndResources() {
  ZX_ASSERT(!pdev_.is_valid());
  ZX_ASSERT(!sysmem_.is_valid());
  ZX_ASSERT(!canvas_.is_valid());
  ZX_ASSERT(!bti_.is_valid());

  static constexpr char kPdevFragmentName[] = "pdev";
  zx::result<fidl::ClientEnd<fuchsia_hardware_platform_device::Device>> pdev_result =
      incoming_->Connect<fuchsia_hardware_platform_device::Service::Device>(kPdevFragmentName);
  if (pdev_result.is_error()) {
    fdf::error("Failed to get the pdev client: {}", pdev_result);
    return pdev_result.status_value();
  }

  pdev_ = fidl::WireSyncClient(std::move(pdev_result).value());
  if (!pdev_.is_valid()) {
    fdf::error("Failed to get a valid platform device client");
    return ZX_ERR_INTERNAL;
  }

  zx::result sysmem_client_result = incoming_->Connect<fuchsia_sysmem2::Allocator>();
  if (sysmem_client_result.is_error()) {
    fdf::error("Failed to get sysmem protocol: {}", sysmem_client_result);
    return sysmem_client_result.status_value();
  }
  sysmem_.Bind(std::move(sysmem_client_result.value()));

  zx::result<fidl::ClientEnd<fuchsia_hardware_amlogiccanvas::Device>> canvas_client_result =
      incoming_->Connect<fuchsia_hardware_amlogiccanvas::Service::Device>("canvas");
  if (canvas_client_result.is_error()) {
    fdf::error("Failed to get Amlogic canvas protocol: {}", canvas_client_result);
    return canvas_client_result.status_value();
  }
  canvas_.Bind(std::move(canvas_client_result.value()));

  zx::result<zx::bti> bti_result = GetBti(BtiResourceIndex::kDma, pdev_.client_end());
  if (bti_result.is_error()) {
    return bti_result.error_value();
  }
  bti_ = std::move(bti_result).value();

  return ZX_OK;
}

zx_koid_t GetKoid(zx_handle_t handle) {
  zx_info_handle_basic_t info;
  zx_status_t status =
      zx_object_get_info(handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
  return status == ZX_OK ? info.koid : ZX_KOID_INVALID;
}

zx_status_t DisplayEngine::InitializeSysmemAllocator() {
  ZX_ASSERT(sysmem_.is_valid());
  const zx_koid_t current_process_koid = GetKoid(zx_process_self());
  const std::string debug_name = fxl::StringPrintf("amlogic-display[%lu]", current_process_koid);
  fidl::Arena arena;
  fidl::OneWayStatus set_debug_status = sysmem_->SetDebugClientInfo(
      fuchsia_sysmem2::wire::AllocatorSetDebugClientInfoRequest::Builder(arena)
          .name(fidl::StringView::FromExternal(debug_name))
          .id(current_process_koid)
          .Build());
  if (!set_debug_status.ok()) {
    fdf::error("Failed to set sysmem allocator debug info: {}", set_debug_status.status_string());
    return set_debug_status.status();
  }
  return ZX_OK;
}

zx_status_t DisplayEngine::Initialize() {
  SetFormatSupportCheck([](display::PixelFormat format) { return IsFormatSupported(format); });

  // Set up inspect first, since other components may add inspect children
  // during initialization.
  root_node_ = inspector_.GetRoot().CreateChild("amlogic-display");

  zx_status_t status = GetCommonProtocolsAndResources();
  if (status != ZX_OK) {
    fdf::error("Failed to get common protocols resources from parent devices: {}",
               zx::make_result(status));
    return status;
  }

  status = InitializeVout();
  if (status != ZX_OK) {
    fdf::error("Failed to initalize Vout: {}", zx::make_result(status));
    return status;
  }

  video_input_unit_node_ = root_node_.CreateChild("video_input_unit");
  zx::result<std::unique_ptr<VideoInputUnit>> video_input_unit_create_result =
      VideoInputUnit::Create(pdev_.client_end(), &video_input_unit_node_);
  if (video_input_unit_create_result.is_error()) {
    fdf::error("Failed to create VideoInputUnit instance: {}", video_input_unit_create_result);
    return video_input_unit_create_result.status_value();
  }
  video_input_unit_ = std::move(video_input_unit_create_result).value();

  zx::result<std::unique_ptr<Vpu>> vpu_result = Vpu::Create(pdev_.client_end());
  if (vpu_result.is_error()) {
    fdf::error("Failed to initialize VPU object: {}", vpu_result);
    return vpu_result.status_value();
  }
  vpu_ = std::move(vpu_result).value();

  // If the display engine was previously owned by a different driver, we
  // attempt to complete a seamless takeover. If we previously owned the
  // hardware, our driver must have been unloaded and reloaded.
  // We currently do a full hardware reset in that case.
  const bool performs_full_hardware_reset =
      IsFullHardwareResetRequired(pdev_.client_end()) || !vpu_->CheckAndClaimHardwareOwnership();
  if (performs_full_hardware_reset) {
    fbl::AutoLock lock(&display_mutex_);
    zx::result<> reset_result = ResetDisplayEngine();
    if (!reset_result.is_ok()) {
      fdf::error("Failed to reset the display engine: {}", reset_result);
      return reset_result.status_value();
    }
  } else {
    // It's possible that the AFBC engine is not yet turned on by the
    // previous driver when the driver takes it over so we should ensure it's
    // enabled.
    //
    // TODO(https://fxbug.dev/42082920): Instead of enabling it ad-hoc here, make
    // `Vpu::PowerOn()` idempotent and always call it when initializing the
    // driver.
    vpu_->AfbcPower(true);
  }

  status = InitializeSysmemAllocator();
  if (status != ZX_OK) {
    fdf::error("Failed to initialize sysmem allocator: {}", zx::make_result(status));
    return status;
  }

  {
    zx::result<std::unique_ptr<VsyncReceiver>> vsync_receiver_result =
        VsyncReceiver::Create(pdev_.client_end(), fit::bind_member<&DisplayEngine::OnVsync>(this));
    if (vsync_receiver_result.is_error()) {
      // Create() already logged the error.
      return vsync_receiver_result.error_value();
    }
    vsync_receiver_ = std::move(vsync_receiver_result).value();
  }

  {
    zx::result<std::unique_ptr<Capture>> capture_result = Capture::Create(
        pdev_.client_end(), fit::bind_member<&DisplayEngine::OnCaptureComplete>(this));
    if (capture_result.is_error()) {
      // Create() already logged the error.
      return capture_result.error_value();
    }
    capture_ = std::move(capture_result).value();
  }

  if (vout_->SupportsHotplugDetection()) {
    if (zx_status_t status = SetupHotplugDisplayDetection(); status != ZX_OK) {
      fdf::error("Failed to set up hotplug display: {}", zx::make_result(status));
      return status;
    }
  }

  set_fully_initialized();
  return ZX_OK;
}

DisplayEngine::DisplayEngine(std::shared_ptr<fdf::Namespace> incoming,
                             display::DisplayEngineEventsInterface* engine_events,
                             structured_config::Config structured_config)
    : incoming_(std::move(incoming)),
      engine_events_(*engine_events),
      structured_config_(structured_config) {
  ZX_DEBUG_ASSERT(incoming_ != nullptr);
  ZX_DEBUG_ASSERT(engine_events != nullptr);
}
DisplayEngine::~DisplayEngine() { Deinitialize(); }

// static
zx::result<std::unique_ptr<DisplayEngine>> DisplayEngine::Create(
    std::shared_ptr<fdf::Namespace> incoming, display::DisplayEngineEventsInterface* engine_events,
    structured_config::Config structured_config) {
  ZX_DEBUG_ASSERT(incoming != nullptr);
  ZX_DEBUG_ASSERT(engine_events != nullptr);

  fbl::AllocChecker alloc_checker;
  auto display_engine = fbl::make_unique_checked<DisplayEngine>(&alloc_checker, std::move(incoming),
                                                                engine_events, structured_config);
  if (!alloc_checker.check()) {
    fdf::error("Failed to allocate memory for DisplayEngine");
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  const zx_status_t status = display_engine->Initialize();
  if (status != ZX_OK) {
    fdf::error("Failed to initialize DisplayEngine instance: {}", zx::make_result(status));
    return zx::error(status);
  }
  return zx::ok(std::move(display_engine));
}

}  // namespace amlogic_display
