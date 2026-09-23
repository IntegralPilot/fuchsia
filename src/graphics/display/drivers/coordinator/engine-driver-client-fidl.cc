// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/engine-driver-client-fidl.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/arena.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fit/result.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include "src/graphics/display/drivers/coordinator/fidl-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/power-mode.h"

namespace display_coordinator {

namespace {
constexpr fdf_arena_tag_t kArenaTag = 'DISP';
}  // namespace

EngineDriverClientFidl::EngineDriverClientFidl(
    fdf::ClientEnd<fuchsia_hardware_display_engine::Engine> fidl_engine)
    : fidl_engine_(std::move(fidl_engine)) {
  ZX_DEBUG_ASSERT(fidl_engine_.is_valid());
}

EngineDriverClientFidl::~EngineDriverClientFidl() = default;

void EngineDriverClientFidl::ReleaseImage(display::DriverImageId driver_image_id) {
  fdf::Arena arena(kArenaTag);
  fidl::OneWayStatus fidl_transport_status =
      fidl_engine_.buffer(arena)->ReleaseImage(driver_image_id.ToFidl());
  ZX_ASSERT_MSG(fidl_transport_status.ok(), "FIDL error calling ReleaseImage: %s",
                fidl_transport_status.FormatDescription().c_str());
}

zx::result<> EngineDriverClientFidl::ReleaseCapture(
    display::DriverCaptureImageId driver_capture_image_id) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ReleaseCapture>
      fidl_transport_result =
          fidl_engine_.buffer(arena)->ReleaseCapture(driver_capture_image_id.ToFidl());
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling ReleaseCapture: %s",
                fidl_transport_result.FormatDescription().c_str());

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

display::ConfigCheckResult EngineDriverClientFidl::CheckConfiguration(
    const DriverDisplayConfig& driver_display_config,
    std::span<const display::DriverLayer> layers) {
  fdf::Arena arena(kArenaTag);
  fuchsia_hardware_display_engine::wire::DisplayConfig fidl_config =
      ToFidlDisplayConfig(driver_display_config, layers, arena);

  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::CheckConfiguration>
      fidl_transport_result = fidl_engine_.buffer(arena)->CheckConfiguration(fidl_config);
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling CheckConfiguration: %s",
                fidl_transport_result.FormatDescription().c_str());

  fit::result<fuchsia_hardware_display_types::ConfigResult>& fidl_domain_result =
      fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    return display::ConfigCheckResult(fidl_domain_result.error_value());
  }
  return display::ConfigCheckResult::kOk;
}

void EngineDriverClientFidl::SubmitConfiguration(const DriverDisplayConfig& driver_display_config,
                                                 std::span<const display::DriverLayer> layers,
                                                 display::DriverConfigStamp config_stamp) {
  fdf::Arena arena(kArenaTag);
  fuchsia_hardware_display_engine::wire::DisplayConfig fidl_config =
      ToFidlDisplayConfig(driver_display_config, layers, arena);

  fdf::WireUnownedResult<::fuchsia_hardware_display_engine::Engine::SubmitConfiguration>
      fidl_transport_result =
          fidl_engine_.buffer(arena)->SubmitConfiguration(fidl_config, config_stamp.ToFidl());
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling SubmitConfiguration: %s",
                fidl_transport_result.FormatDescription().c_str());
}

display::EngineInfo EngineDriverClientFidl::CompleteCoordinatorConnection(
    fdf::ClientEnd<fuchsia_hardware_display_engine::EngineListener> fidl_listener_client) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::CompleteCoordinatorConnection>
      fidl_transport_result = fidl_engine_.buffer(arena)->CompleteCoordinatorConnection(
          std::move(fidl_listener_client));
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling CompleteCoordinatorConnection: %s",
                fidl_transport_result.FormatDescription().c_str());

  fuchsia_hardware_display_engine::wire::EngineCompleteCoordinatorConnectionResponse&
      fidl_domain_result = fidl_transport_result.value();
  return display::EngineInfo::From(fidl_domain_result.engine_info);
}

void EngineDriverClientFidl::UnsetListener() {
  fdf::Arena arena(kArenaTag);
  fidl::OneWayStatus fidl_transport_status = fidl_engine_.buffer(arena)->UnsetListener();
  ZX_ASSERT_MSG(fidl_transport_status.ok(), "FIDL error calling UnsetListener: %s",
                fidl_transport_status.FormatDescription().c_str());
}

zx::result<display::DriverImageId> EngineDriverClientFidl::ImportImage(
    const display::ImageMetadata& image_metadata, display::DriverBufferCollectionId collection_id,
    uint32_t index) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportImage>
      fidl_transport_result = fidl_engine_.buffer(arena)->ImportImage(
          image_metadata.ToFidl(), collection_id.ToFidl(), index);
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling ImportImage: %s",
                fidl_transport_result.FormatDescription().c_str());

  fit::result<zx_status_t, fuchsia_hardware_display_engine::wire::EngineImportImageResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok(display::DriverImageId(fidl_domain_result.value()->image_id.value));
}

zx::result<display::DriverCaptureImageId> EngineDriverClientFidl::ImportImageForCapture(
    display::DriverBufferCollectionId collection_id, uint32_t index) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportImageForCapture>
      fidl_transport_result =
          fidl_engine_.buffer(arena)->ImportImageForCapture(collection_id.ToFidl(), index);
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling ImportImageForCapture: %s",
                fidl_transport_result.FormatDescription().c_str());

  fit::result<zx_status_t,
              fuchsia_hardware_display_engine::wire::EngineImportImageForCaptureResponse*>&
      fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    return zx::error(fidl_domain_result.error_value());
  }
  fuchsia_hardware_display_engine::wire::ImageId image_id =
      fidl_domain_result.value()->capture_image_id;
  return zx::ok(display::DriverCaptureImageId(image_id.value));
}

zx::result<> EngineDriverClientFidl::ImportBufferCollection(
    display::DriverBufferCollectionId collection_id,
    fidl::ClientEnd<fuchsia_sysmem2::BufferCollectionToken> collection_token) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ImportBufferCollection>
      fidl_transport_result = fidl_engine_.buffer(arena)->ImportBufferCollection(
          collection_id.ToFidl(), std::move(collection_token));
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling ImportBufferCollection: %s",
                fidl_transport_result.FormatDescription().c_str());

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

zx::result<> EngineDriverClientFidl::ReleaseBufferCollection(
    display::DriverBufferCollectionId collection_id) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::ReleaseBufferCollection>
      fidl_transport_result =
          fidl_engine_.buffer(arena)->ReleaseBufferCollection(collection_id.ToFidl());
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling ReleaseBufferCollection: %s",
                fidl_transport_result.FormatDescription().c_str());

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

zx::result<> EngineDriverClientFidl::SetBufferCollectionConstraints(
    const display::ImageBufferUsage& usage, display::DriverBufferCollectionId collection_id) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetBufferCollectionConstraints>
      fidl_transport_result = fidl_engine_.buffer(arena)->SetBufferCollectionConstraints(
          usage.ToFidl(), collection_id.ToFidl());
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling SetBufferCollectionConstraints: %s",
                fidl_transport_result.FormatDescription().c_str());

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

zx::result<> EngineDriverClientFidl::StartCapture(
    display::DriverCaptureImageId driver_capture_image_id) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::StartCapture>
      fidl_transport_result =
          fidl_engine_.buffer(arena)->StartCapture(driver_capture_image_id.ToFidl());
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling StartCapture: %s",
                fidl_transport_result.FormatDescription().c_str());

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

zx::result<> EngineDriverClientFidl::SetDisplayPowerMode(display::DisplayId display_id,
                                                         display::PowerMode power_mode) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetDisplayPowerMode>
      set_display_power_mode_result =
          fidl_engine_.buffer(arena)->SetDisplayPowerMode(display_id.ToFidl(), power_mode.ToFidl());
  ZX_ASSERT_MSG(set_display_power_mode_result.ok(), "FIDL error calling SetDisplayPowerMode: %s",
                set_display_power_mode_result.FormatDescription().c_str());

  fit::result<zx_status_t>& fidl_domain_result = set_display_power_mode_result.value();
  if (fidl_domain_result.is_ok()) {
    return zx::ok();
  }
  return zx::error(fidl_domain_result.error_value());
}

zx::result<> EngineDriverClientFidl::SetMinimumRgb(uint8_t minimum_rgb) {
  fdf::Arena arena(kArenaTag);
  fdf::WireUnownedResult<fuchsia_hardware_display_engine::Engine::SetMinimumRgb>
      fidl_transport_result = fidl_engine_.buffer(arena)->SetMinimumRgb(minimum_rgb);
  ZX_ASSERT_MSG(fidl_transport_result.ok(), "FIDL error calling SetMinimumRgb: %s",
                fidl_transport_result.FormatDescription().c_str());

  fit::result<zx_status_t>& fidl_domain_result = fidl_transport_result.value();
  if (fidl_domain_result.is_error()) {
    return zx::error(fidl_domain_result.error_value());
  }
  return zx::ok();
}

}  // namespace display_coordinator
