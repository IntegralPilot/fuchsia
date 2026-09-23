// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/drivers/coordinator/client.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <fidl/fuchsia.images2/cpp/wire.h>
#include <lib/async/cpp/task.h>
#include <lib/async/dispatcher.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>
#include <lib/image-format/image_format.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/stdcompat/inplace_vector.h>
#include <lib/sync/completion.h>
#include <lib/sysmem-version/sysmem-version.h>
#include <lib/trace/event.h>
#include <lib/zx/clock.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/vmo.h>
#include <threads.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <algorithm>
#include <cmath>
#include <cstddef>
#include <cstring>
#include <memory>
#include <utility>

#include <fbl/alloc_checker.h>
#include <fbl/auto_lock.h>
#include <fbl/ref_ptr.h>
#include <fbl/string_printf.h>
#include <fbl/vector.h>

#include "src/graphics/display/drivers/coordinator/capture-image.h"
#include "src/graphics/display/drivers/coordinator/client-id.h"
#include "src/graphics/display/drivers/coordinator/client-vsync-queue.h"
#include "src/graphics/display/drivers/coordinator/engine-driver-client.h"
#include "src/graphics/display/drivers/coordinator/fence.h"
#include "src/graphics/display/drivers/coordinator/image.h"
#include "src/graphics/display/lib/api-types/cpp/buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/client-priority.h"
#include "src/graphics/display/lib/api-types/cpp/color-conversion.h"
#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-buffer-collection-id.h"
#include "src/graphics/display/lib/api-types/cpp/driver-capture-image-id.h"
#include "src/graphics/display/lib/api-types/cpp/event-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-buffer-usage.h"
#include "src/graphics/display/lib/api-types/cpp/image-id.h"
#include "src/graphics/display/lib/api-types/cpp/image-metadata.h"
#include "src/graphics/display/lib/api-types/cpp/layer-id.h"
#include "src/graphics/display/lib/api-types/cpp/mode-id.h"
#include "src/graphics/display/lib/api-types/cpp/power-mode.h"
#include "src/graphics/display/lib/api-types/cpp/rectangle.h"
#include "src/graphics/display/lib/api-types/cpp/vsync-ack-cookie.h"

namespace fhd = fuchsia_hardware_display;
namespace fhdt = fuchsia_hardware_display_types;

namespace {

// True iff `inner` is entirely contained within `outer`.
//
// `outer` must be positioned at the coordinate system's origin. Both `inner` and `outer` must be
// non-empty.
constexpr bool OriginRectangleContains(const display::Rectangle& outer,
                                       const display::Rectangle& inner) {
  ZX_DEBUG_ASSERT(outer.x() == 0);
  ZX_DEBUG_ASSERT(outer.y() == 0);
  ZX_DEBUG_ASSERT(outer.width() > 0);
  ZX_DEBUG_ASSERT(outer.height() > 0);
  ZX_DEBUG_ASSERT(inner.width() > 0);
  ZX_DEBUG_ASSERT(inner.height() > 0);

  return inner.x() < outer.width() && inner.y() < outer.height() &&
         inner.x() + inner.width() <= outer.width() && inner.y() + inner.height() <= outer.height();
}

// We allocate some variable sized stack allocations based on the number of
// layers, so we limit the total number of layers to prevent blowing the stack.
constexpr uint64_t kMaxLayers = 65536;

}  // namespace

namespace display_coordinator {

void Client::ImportImage(ImportImageRequestView request, ImportImageCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::ImportImage");

  const display::ImageId image_id = display::ImageId(request->image_id);
  if (image_id == display::kInvalidImageId) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto images_it = images_.find(image_id);
  if (images_it.IsValid()) {
    completer.ReplyError(ZX_ERR_ALREADY_EXISTS);
    return;
  }
  auto capture_image_it = capture_images_.find(image_id);
  if (capture_image_it.IsValid()) {
    completer.ReplyError(ZX_ERR_ALREADY_EXISTS);
    return;
  }

  if (!display::ImageMetadata::IsValid(request->image_metadata)) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  const display::ImageMetadata image_metadata(request->image_metadata);
  const display::BufferCollectionId buffer_collection_id(request->buffer_collection_id);
  const uint32_t buffer_index = request->buffer_index;

  if (image_metadata.tiling_type() == display::ImageTilingType::kCapture) {
    zx_status_t import_status =
        ImportImageForCapture(image_metadata, buffer_collection_id, buffer_index, image_id);
    if (import_status == ZX_OK) {
      completer.ReplySuccess();
    } else {
      completer.ReplyError(import_status);
    }
    return;
  }

  zx_status_t import_status =
      ImportImageForDisplay(image_metadata, buffer_collection_id, buffer_index, image_id);
  if (import_status == ZX_OK) {
    completer.ReplySuccess();
  } else {
    completer.ReplyError(import_status);
  }
}

zx_status_t Client::ImportImageForDisplay(const display::ImageMetadata& image_metadata,
                                          display::BufferCollectionId buffer_collection_id,
                                          uint32_t buffer_index, display::ImageId image_id) {
  ZX_DEBUG_ASSERT(image_metadata.tiling_type() != display::ImageTilingType::kCapture);
  ZX_DEBUG_ASSERT(!images_.find(image_id).IsValid());
  ZX_DEBUG_ASSERT(!capture_images_.find(image_id).IsValid());

  auto collection_map_it = collection_map_.find(buffer_collection_id);
  if (collection_map_it == collection_map_.end()) {
    return ZX_ERR_INVALID_ARGS;
  }
  const Collections& collections = collection_map_it->second;

  zx::result<display::DriverImageId> result = controller_.engine_driver_client()->ImportImage(
      image_metadata, collections.driver_buffer_collection_id, buffer_index);
  if (result.is_error()) {
    return result.error_value();
  }

  const display::DriverImageId driver_image_id = result.value();
  auto release_image =
      fit::defer([this, driver_image_id]() { controller_.ImageWillBeDestroyed(driver_image_id); });

  fbl::AllocChecker alloc_checker;
  fbl::RefPtr<Image> image = fbl::AdoptRef(new (&alloc_checker) Image(
      &controller_, image_metadata, image_id, driver_image_id, &node(), id_));
  if (!alloc_checker.check()) {
    fdf::debug("Alloc checker failed while constructing Image.\n");
    return ZX_ERR_NO_MEMORY;
  }
  // `dc_image` is now owned by the Image instance.
  release_image.cancel();

  images_.insert(std::move(image));
  return ZX_OK;
}

void Client::ReleaseImage(ReleaseImageRequestView request, ReleaseImageCompleter::Sync&) {
  TRACE_DURATION("gfx", "Display::Client::ReleaseImage");

  const display::ImageId image_id = display::ImageId(request->image_id);
  auto image = images_.find(image_id);
  if (image.IsValid()) {
    if (CleanUpImage(*image)) {
      SubmitConfig();
    }
    return;
  }

  auto capture_image = capture_images_.find(image_id);
  if (capture_image.IsValid()) {
    // Ensure we are not releasing an active capture.
    if (current_capture_image_id_ == image_id) {
      // We have an active capture; release it when capture is completed.
      fdf::warn("Capture is active. Will release after capture is complete");
      pending_release_capture_image_id_ = current_capture_image_id_;
    } else {
      // Release image now.
      capture_images_.erase(capture_image);
    }
    return;
  }

  fdf::error("Invalid Image ID requested for release");
}

void Client::ImportEvent(ImportEventRequestView request, ImportEventCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::ImportEvent");

  const display::EventId event_id = display::EventId(request->id);
  if (event_id == display::kInvalidEventId) {
    fdf::error("Cannot import events with an invalid ID #{}", event_id.value());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  zx::result<> import_result = fences_.ImportEvent(std::move(request->event), event_id);
  if (import_result.is_error()) {
    fdf::error("Failed to import event: {}", import_result);
    completer.Close(import_result.error_value());
    return;
  }
}

void Client::ImportBufferCollection(ImportBufferCollectionRequestView request,
                                    ImportBufferCollectionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::ImportBufferCollection");

  const display::BufferCollectionId buffer_collection_id =
      display::BufferCollectionId(request->buffer_collection_id);
  // TODO: Switch to .contains() when C++20.
  if (collection_map_.count(buffer_collection_id)) {
    completer.ReplyError(ZX_ERR_ALREADY_EXISTS);
    return;
  }

  const display::DriverBufferCollectionId driver_buffer_collection_id =
      controller_.GetNextDriverBufferCollectionId();
  zx::result<> import_result = controller_.engine_driver_client()->ImportBufferCollection(
      driver_buffer_collection_id, std::move(request->buffer_collection_token));
  if (import_result.is_error()) {
    fdf::warn("Cannot import BufferCollection to display driver: {}", import_result);
    completer.ReplyError(ZX_ERR_INTERNAL);
    return;
  }

  collection_map_[buffer_collection_id] = Collections{
      .driver_buffer_collection_id = driver_buffer_collection_id,
  };
  completer.ReplySuccess();
}

void Client::ReleaseBufferCollection(ReleaseBufferCollectionRequestView request,
                                     ReleaseBufferCollectionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::ReleaseBufferCollection");

  const display::BufferCollectionId buffer_collection_id =
      display::BufferCollectionId(request->buffer_collection_id);
  auto it = collection_map_.find(buffer_collection_id);
  if (it == collection_map_.end()) {
    return;
  }

  [[maybe_unused]] zx::result<> result =
      controller_.engine_driver_client()->ReleaseBufferCollection(
          it->second.driver_buffer_collection_id);
  if (result.is_error()) {
    // TODO(https://fxbug.dev/42180237) Consider handling the error instead of ignoring it.
  }

  collection_map_.erase(it);
}

void Client::SetBufferCollectionConstraints(
    SetBufferCollectionConstraintsRequestView request,
    SetBufferCollectionConstraintsCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetBufferCollectionConstraints");

  const display::BufferCollectionId buffer_collection_id =
      display::BufferCollectionId(request->buffer_collection_id);
  auto it = collection_map_.find(buffer_collection_id);
  if (it == collection_map_.end()) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }
  auto& collections = it->second;

  const display::ImageBufferUsage image_buffer_usage(request->buffer_usage);
  zx::result<> result = controller_.engine_driver_client()->SetBufferCollectionConstraints(
      image_buffer_usage, collections.driver_buffer_collection_id);
  if (result.is_error()) {
    fdf::warn(
        "Cannot set BufferCollection constraints using imported buffer collection (id={}) {}.",
        buffer_collection_id.value(), result);
    completer.ReplyError(ZX_ERR_INTERNAL);
  }
  completer.ReplySuccess();
}

void Client::ReleaseEvent(ReleaseEventRequestView request, ReleaseEventCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::ReleaseEvent");

  const display::EventId event_id = display::EventId(request->id);
  // TODO(https://fxbug.dev/42080337): Check if the ID is valid (i.e. imported but not
  // yet released) before calling `ReleaseEvent()`.
  fences_.ReleaseEvent(event_id);
}

void Client::CreateLayer(CreateLayerRequestView request, CreateLayerCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::CreateLayer");

  display::LayerId layer_id = display::LayerId(request->layer_id);
  if (layer_id == display::kInvalidLayerId) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (layers_.find(layer_id).IsValid()) {
    completer.ReplyError(ZX_ERR_ALREADY_EXISTS);
    return;
  }

  if (layers_.size() == kMaxLayers) {
    completer.ReplyError(ZX_ERR_NO_RESOURCES);
    return;
  }

  fbl::AllocChecker alloc_checker;
  auto new_layer = fbl::make_unique_checked<Layer>(&alloc_checker, layer_id);
  if (!alloc_checker.check()) {
    completer.ReplyError(ZX_ERR_NO_MEMORY);
    return;
  }

  layers_.insert(std::move(new_layer));
  completer.ReplySuccess();
}

void Client::DestroyLayer(DestroyLayerRequestView request, DestroyLayerCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::DestroyLayer");

  display::LayerId layer_id = display::LayerId(request->layer_id);

  auto layers_it = layers_.find(layer_id);
  if (!layers_it.IsValid()) {
    fdf::error("Tried to destroy invalid layer {}", layer_id.value());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  Layer& layer = *layers_it;
  if (layer.in_use()) {
    fdf::error("Destroyed layer {} which was in use", layer_id.value());
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  layers_.erase(layers_it);
}

namespace {

// Returns `ModeId` that corresponds to the provided `target_mode` in
// `display_preferred_modes`.
//
// Returns `kInvalidModeId` if the `target_mode` cannot be found.
display::ModeId GetPreferredModeIdForMode(
    std::span<const display::ModeAndId> display_preferred_modes, const display::Mode& target_mode) {
  auto mode_it = std::ranges::find_if(
      display_preferred_modes,
      [&](const display::ModeAndId& mode_and_id) { return mode_and_id.mode() == target_mode; });
  if (mode_it != display_preferred_modes.end()) {
    return mode_it->id();
  }
  return display::kInvalidModeId;
}

}  // namespace

void Client::SetDisplayMode(SetDisplayModeRequestView request,
                            SetDisplayModeCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetDisplayMode");

  const display::DisplayId display_id = display::DisplayId(request->display_id);
  auto display_configs_it = display_configs_.find(display_id);
  if (!display_configs_it.IsValid()) {
    fdf::warn("SetDisplayMode called with unknown display ID: {}", display_id.value());
    return;
  }
  DisplayConfig& display_config = *display_configs_it;

  if (!display::Mode::IsValid(request->mode)) {
    fdf::error("SetDisplayMode called with invalid mode: {}x{} @ {}.{:03} Hz",
               request->mode.active_area.width, request->mode.active_area.height,
               request->mode.refresh_rate_millihertz / 1000,
               request->mode.refresh_rate_millihertz % 1000);
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  display::Mode target_mode = display::Mode::From(request->mode);

  zx::result<std::span<const display::ModeAndId>> display_preferred_modes_result =
      controller_.GetDisplayPreferredModes(display_id);
  if (display_preferred_modes_result.is_error()) {
    fdf::error("Failed to get display preferred modes for display ID {}: {}", display_id.value(),
               display_preferred_modes_result);
    completer.Close(display_preferred_modes_result.status_value());
    return;
  }
  std::span<const display::ModeAndId> display_preferred_modes =
      std::move(display_preferred_modes_result).value();

  display::ModeId mode_id = GetPreferredModeIdForMode(display_preferred_modes, target_mode);
  if (mode_id == display::kInvalidModeId) {
    fdf::error("Failed to find supported mode for: {}", target_mode);
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  fdf::info("Found supported display mode {} in the preferred modes list", target_mode);

  if (display_preferred_modes.size() == 1) {
    // If there is only one mode, the coordinator doesn't need to set
    // the display mode on engine.
    fdf::info("The display has only one mode. Skip setting display mode.");
    return;
  }

  display_config.draft_.mode_id = mode_id;
  display_config.has_draft_nonlayer_config_change_ = true;
  draft_display_config_was_validated_ = false;
}

void Client::SetDisplayColorConversion(SetDisplayColorConversionRequestView request,
                                       SetDisplayColorConversionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetDisplayColorConversion");

  const display::DisplayId display_id = display::DisplayId(request->display_id);
  auto display_configs_it = display_configs_.find(display_id);
  if (!display_configs_it.IsValid()) {
    fdf::warn("SetDisplayColorConversion called with unknown display ID: {}", display_id.value());
    return;
  }
  DisplayConfig& display_config = *display_configs_it;

  fidl::Array<float, 3>& fidl_preoffsets = request->preoffsets;
  fidl::Array<float, 9>& fidl_coefficients = request->coefficients;
  fidl::Array<float, 3>& fidl_postoffsets = request->postoffsets;

  if (std::ranges::any_of(fidl_preoffsets, [](float x) { return !std::isfinite(x); })) {
    fdf::warn("SetDisplayColorConversion called with invalid preoffsets: {}", display_id.value());
    return;
  }
  if (std::ranges::any_of(fidl_coefficients, [](float x) { return !std::isfinite(x); })) {
    fdf::warn("SetDisplayColorConversion called with invalid coefficients: {}", display_id.value());
    return;
  }
  if (std::ranges::any_of(fidl_postoffsets, [](float x) { return !std::isfinite(x); })) {
    fdf::warn("SetDisplayColorConversion called with invalid postoffsets: {}", display_id.value());
    return;
  }

  display::ColorConversion color_conversion({
      .preoffsets = {fidl_preoffsets[0], fidl_preoffsets[1], fidl_preoffsets[2]},
      .coefficients =
          {
              std::array<float, 3>{fidl_coefficients[0], fidl_coefficients[1],
                                   fidl_coefficients[2]},
              std::array<float, 3>{fidl_coefficients[3], fidl_coefficients[4],
                                   fidl_coefficients[5]},
              std::array<float, 3>{fidl_coefficients[6], fidl_coefficients[7],
                                   fidl_coefficients[8]},
          },
      .postoffsets = {fidl_postoffsets[0], fidl_postoffsets[1], fidl_postoffsets[2]},
  });
  display_config.draft_.color_conversion = std::move(color_conversion);
  display_config.has_draft_nonlayer_config_change_ = true;
  draft_display_config_was_validated_ = false;

  // One-way call. No reply required.
}

void Client::SetDisplayLayers(SetDisplayLayersRequestView request,
                              SetDisplayLayersCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetDisplayLayers");

  if (request->layer_ids.empty()) {
    fdf::error("SetDisplayLayers called with an empty layer list");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  const display::DisplayId display_id = display::DisplayId(request->display_id);
  auto display_configs_it = display_configs_.find(display_id);
  if (!display_configs_it.IsValid()) {
    fdf::warn("SetDisplayLayers called with unknown display ID: {}", display_id.value());
    return;
  }
  DisplayConfig& display_config = *display_configs_it;

  // The static_cast does not overflow or underflow (causing UB) because
  // `engine_max_layer_count()` is guaranteed to be positive and at most
  // `display::EngineInfo::kMaxAllowedMaxLayerCount`.
  if (request->layer_ids.size() > static_cast<size_t>(display_config.engine_max_layer_count())) {
    fdf::error(
        "SetDisplayLayers for display {} failed: requested {} layers, but hardware limit is {}. "
        "Terminating client.",
        display_id.value(), request->layer_ids.size(), display_config.engine_max_layer_count());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  display_config.draft_has_layer_list_change_ = true;
  display_config.draft_has_layer_list_change_property_.Set(true);

  display_config.draft_layers_.clear();
  for (fuchsia_hardware_display::wire::LayerId fidl_layer_id : request->layer_ids) {
    display::LayerId layer_id = display::LayerId(fidl_layer_id);

    auto layers_it = layers_.find(layer_id);
    if (!layers_it.IsValid()) {
      fdf::error("SetDisplayLayers called with unknown layer ID: {}", layer_id.value());
      completer.Close(ZX_ERR_INVALID_ARGS);
      return;
    }

    Layer& layer = *layers_it;
    if (!layer.AppendToConfigLayerList(display_config.draft_layers_)) {
      fdf::error("Tried to reuse an in-use layer");
      completer.Close(ZX_ERR_BAD_STATE);
      return;
    }
  }
  display_config.draft_.layer_count = static_cast<int>(request->layer_ids.size());
  draft_display_config_was_validated_ = false;

  // One-way call. No reply required.
}

void Client::SetLayerPrimaryConfig(SetLayerPrimaryConfigRequestView request,
                                   SetLayerPrimaryConfigCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetLayerPrimaryConfig");

  display::LayerId layer_id = display::LayerId(request->layer_id);

  auto layers_it = layers_.find(layer_id);
  if (!layers_it.IsValid()) {
    fdf::error("SetLayerPrimaryConfig called with unknown layer ID: {}", layer_id.value());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  Layer& layer = *layers_it;

  if (!display::ImageMetadata::IsValid(request->image_metadata)) {
    fdf::error("SetLayerPrimaryConfig called with invalid image metadata");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  display::ImageMetadata image_metadata(request->image_metadata);
  layer.SetPrimaryConfig(image_metadata);

  // TODO(https://fxbug.dev/397427767): Check if the layer belongs to the draft
  // config first.
  draft_display_config_was_validated_ = false;

  // One-way call. No reply required.
}

void Client::SetLayerPrimaryPosition(SetLayerPrimaryPositionRequestView request,
                                     SetLayerPrimaryPositionCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetLayerPrimaryPosition");

  display::LayerId layer_id = display::LayerId(request->layer_id);

  auto layers_it = layers_.find(layer_id);
  if (!layers_it.IsValid()) {
    fdf::error("SetLayerPrimaryPosition called with unknown layer ID: {}", layer_id.value());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  Layer& layer = *layers_it;

  if (!display::CoordinateTransformation::IsValid(request->image_source_transformation)) {
    fdf::error("SetLayerPrimaryPosition called with invalid image_source_transformation");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  display::CoordinateTransformation image_source_transformation(
      request->image_source_transformation);

  if (!display::Rectangle::IsValid(request->image_source)) {
    fdf::error("SetLayerPrimaryPosition called with invalid image_source");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  display::Rectangle image_source = display::Rectangle::From(request->image_source);

  if (!display::Rectangle::IsValid(request->display_destination)) {
    fdf::error("SetLayerPrimaryPosition called with invalid display_destination");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  display::Rectangle display_destination = display::Rectangle::From(request->display_destination);

  layer.SetPrimaryPosition(image_source_transformation, image_source, display_destination);

  // TODO(https://fxbug.dev/397427767): Check if the layer belongs to the draft
  // config first.
  draft_display_config_was_validated_ = false;

  // One-way call. No reply required.
}

void Client::SetLayerPrimaryAlpha(SetLayerPrimaryAlphaRequestView request,
                                  SetLayerPrimaryAlphaCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetLayerPrimaryAlpha");

  display::LayerId layer_id = display::LayerId(request->layer_id);

  auto layers_it = layers_.find(layer_id);
  if (!layers_it.IsValid()) {
    fdf::error("SetLayerPrimaryAlpha called with unknown layer ID: {}", layer_id.value());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  Layer& layer = *layers_it;

  if (!display::AlphaMode::IsValid(request->mode)) {
    fdf::error("Invalid alpha mode {}", static_cast<uint8_t>(request->mode));
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  display::AlphaMode alpha_mode(request->mode);

  if ((!isnan(request->val) && (request->val < 0 || request->val > 1))) {
    fdf::error("Invalid alpha value {}", request->val);
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  layer.SetPrimaryAlpha(alpha_mode, /*alpha_coefficient=*/request->val);

  // TODO(https://fxbug.dev/397427767): Check if the layer belongs to the draft
  // config first.
  draft_display_config_was_validated_ = false;

  // One-way call. No reply required.
}

void Client::SetLayerColorConfig(SetLayerColorConfigRequestView request,
                                 SetLayerColorConfigCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetLayerColorConfig");

  display::LayerId layer_id = display::LayerId(request->layer_id);

  auto layers_it = layers_.find(layer_id);
  if (!layers_it.IsValid()) {
    fdf::error("SetLayerColorConfig called with unknown layer ID: {}", layer_id.value());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  Layer& layer = *layers_it;

  if (!display::Color::IsValid(request->color)) {
    fdf::error("SetLayerColorConfig with invalid pixel format");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  display::Color color = display::Color::From(request->color);

  if (!display::Rectangle::IsValid(request->display_destination)) {
    fdf::error("SetLayerColorConfig called with invalid display_destination");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  display::Rectangle display_destination = display::Rectangle::From(request->display_destination);

  layer.SetColorConfig(color, display_destination);

  // TODO(https://fxbug.dev/397427767): Check if the layer belongs to the draft
  // config first.
  draft_display_config_was_validated_ = false;

  // One-way call. No reply required.
}

void Client::SetLayerImage2(SetLayerImage2RequestView request,
                            SetLayerImage2Completer::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetLayerImage2");

  display::LayerId layer_id(request->layer_id);
  display::ImageId image_id(request->image_id);
  display::EventId wait_event_id(request->wait_event_id);

  auto layers_it = layers_.find(layer_id);
  if (!layers_it.IsValid()) {
    fdf::error("SetLayerImage called with unknown layer ID: {}", layer_id.value());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  Layer& layer = *layers_it;

  auto images_it = images_.find(image_id);
  if (!images_it.IsValid()) {
    fdf::error("SetLayerImage called with unknown image ID: {}", image_id.value());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  Image& image = *images_it;

  // TODO(https://fxbug.dev/42076907): Currently this logic only compares size
  // and usage type between the current `Image` and a given `Layer`'s accepted
  // configuration.
  //
  // We don't set the pixel format a `Layer` can accept, and we don't compare the
  // `Image` pixel format against any accepted pixel format, assuming that all
  // image buffers allocated by sysmem can always be used for scanout in any
  // `Layer`. Currently, this assumption works for all our existing display engine
  // drivers. However, switching pixel formats in a `Layer` may cause performance
  // reduction, or might be not supported by new display engines / new display
  // formats.
  //
  // We should figure out a mechanism to indicate pixel format / modifiers
  // support for a `Layer`'s image configuration (as opposed of using image_t),
  // and compare this Image's sysmem buffer collection information against the
  // `Layer`'s format support.
  if (image.metadata() != display::ImageMetadata(layer.draft_image_metadata())) {
    fdf::error("SetLayerImage with mismatching layer and image metadata");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  // TODO(https://fxbug.dev/42080337): Check if the IDs are valid (i.e. imported but not
  // yet released) before calling `SetImage()`.
  layer.SetImage(images_it.CopyPointer(), wait_event_id);

  // One-way call. No reply required.
}

void Client::CheckConfig(CheckConfigCompleter::Sync& completer) {
  display::ConfigCheckResult config_check_result = CheckConfigImpl();
  draft_display_config_was_validated_ = config_check_result == display::ConfigCheckResult::kOk;

  completer.Reply(config_check_result.ToFidl());
}

void Client::DiscardConfig(DiscardConfigCompleter::Sync& completer) { DiscardConfig(); }

void Client::CommitConfig(CommitConfigRequestView request, CommitConfigCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::CommitConfig");

  if (!request->has_stamp()) {
    fdf::error("CommitConfig called without a config stamp");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  const display::ConfigStamp new_config_stamp(request->stamp().value);
  TRACE_FLOW_END("gfx", "Display::CommitConfig", new_config_stamp.value());

  if (layers_.is_empty()) {
    fdf::error("CommitConfig called before SetDisplayLayers");
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  if (!draft_display_config_was_validated_) {
    // TODO(https://fxbug.dev/397427767): completer.Close(ZX_ERR_BAD_STATE) instead of
    // calling CheckConfig() and silently failing.
    draft_display_config_was_validated_ = CheckConfigImpl() == display::ConfigCheckResult::kOk;

    if (!draft_display_config_was_validated_) {
      fdf::info("CommitConfig called with invalid configuration; dropping the request");
      return;
    }
  }

  // Now that we can guarantee that the configuration will be applied, it is
  // safe to update the config stamp.
  if (new_config_stamp <= latest_config_stamp_) {
    fdf::error(
        "CommitConfig config stamp not monotonically increasing; new stamp: {}, previous stamp: {}",
        new_config_stamp.value(), latest_config_stamp_.value());
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }
  latest_config_stamp_ = new_config_stamp;

  // Empty committed layer lists for all displays whose layer lists are changing.
  //
  // This guarantees that layers moved between displays don't end up in two
  // layer lists while each display's committed configuration is updated to match
  // its draft configuration.
  for (DisplayConfig& display_config : display_configs_) {
    if (display_config.draft_has_layer_list_change_) {
      display_config.committed_layers_.clear();
    }
  }

  for (DisplayConfig& display_config : display_configs_) {
    if (display_config.has_draft_nonlayer_config_change_) {
      display_config.committed_ = display_config.draft_;
      display_config.has_draft_nonlayer_config_change_ = false;
    }

    // Update any image layers. This needs to be done before migrating layers, as
    // that needs to know if there are any waiting images.
    for (LayerNode& draft_layer_node : display_config.draft_layers_) {
      if (!draft_layer_node.layer->ResolveDraftLayerProperties()) {
        fdf::error("Failed to resolve draft layer properties for layer {}",
                   draft_layer_node.layer->id().value());
        completer.Close(ZX_ERR_BAD_STATE);
        return;
      }
      if (!draft_layer_node.layer->ResolveDraftImage(&fences_, latest_config_stamp_)) {
        fdf::error("Failed to resolve draft image for layer {}",
                   draft_layer_node.layer->id().value());
        completer.Close(ZX_ERR_BAD_STATE);
        return;
      }
    }

    // Build applied layer lists that were emptied above.
    if (display_config.draft_has_layer_list_change_) {
      // Rebuild the committed layer list from the draft layer list.
      for (LayerNode& draft_layer_node : display_config.draft_layers_) {
        Layer* draft_layer = draft_layer_node.layer;
        display_config.committed_layers_.push_back(
            &draft_layer->committed_display_config_list_node_);
      }

      for (LayerNode& committed_layer_node : display_config.committed_layers_) {
        Layer* committed_layer = committed_layer_node.layer;
        // Don't migrate images between displays if there are pending images. See
        // `Controller::SubmitConfig` for more details.
        if (committed_layer->applied_to_display_id_ != display_config.id() &&
            committed_layer->committed_image_ != nullptr && committed_layer->HasWaitingImages()) {
          committed_layer->committed_image_ = nullptr;

          // This doesn't need to be reset anywhere, since we really care about the last
          // display this layer was shown on. Ignoring the 'null' display could cause
          // unusual layer changes to trigger this unnecessary, but that's not wrong.
          committed_layer->applied_to_display_id_ = display_config.id();
        }
      }
      display_config.draft_has_layer_list_change_ = false;
      display_config.draft_has_layer_list_change_property_.Set(false);
      display_config.pending_commit_layer_change_ = true;
      display_config.pending_commit_layer_change_property_.Set(true);
    }

    // Commit draft configuration changes in committed layers.
    for (LayerNode& committed_layer_node : display_config.committed_layers_) {
      committed_layer_node.layer->CommitChanges();
    }
  }

  SubmitConfig();

  // No reply defined.
}

void Client::GetLatestCommittedConfigStamp(
    GetLatestCommittedConfigStampCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::GetLatestCommittedConfigStamp");
  completer.Reply(latest_config_stamp_.ToFidl());
}

void Client::IsCaptureSupported(IsCaptureSupportedCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::IsCaptureSupported");
  completer.ReplySuccess(controller_.supports_capture());
}

zx_status_t Client::ImportImageForCapture(const display::ImageMetadata& image_metadata,
                                          display::BufferCollectionId buffer_collection_id,
                                          uint32_t buffer_index, display::ImageId image_id) {
  ZX_DEBUG_ASSERT(image_metadata.tiling_type() == display::ImageTilingType::kCapture);
  ZX_DEBUG_ASSERT(!images_.find(image_id).IsValid());
  ZX_DEBUG_ASSERT(!capture_images_.find(image_id).IsValid());

  // Ensure display driver supports/implements capture.
  if (!controller_.supports_capture()) {
    return ZX_ERR_NOT_SUPPORTED;
  }

  // Ensure a previously imported collection id is being used for import.
  auto it = collection_map_.find(buffer_collection_id);
  if (it == collection_map_.end()) {
    return ZX_ERR_INVALID_ARGS;
  }
  const Client::Collections& collections = it->second;
  zx::result<display::DriverCaptureImageId> import_result =
      controller_.engine_driver_client()->ImportImageForCapture(
          collections.driver_buffer_collection_id, buffer_index);
  if (import_result.is_error()) {
    return import_result.error_value();
  }
  const display::DriverCaptureImageId driver_capture_image_id = import_result.value();
  auto release_image = fit::defer([this, driver_capture_image_id]() {
    // TODO(https://fxbug.dev/42180237): Consider handling the error instead of ignoring it.
    [[maybe_unused]] zx::result<> result =
        controller_.engine_driver_client()->ReleaseCapture(driver_capture_image_id);
  });

  fbl::AllocChecker alloc_checker;
  fbl::RefPtr<CaptureImage> capture_image = fbl::AdoptRef(new (&alloc_checker) CaptureImage(
      &controller_, image_id, driver_capture_image_id, &node(), id_));
  if (!alloc_checker.check()) {
    return ZX_ERR_NO_MEMORY;
  }
  // `driver_capture_image_id` is now owned by the CaptureImage instance.
  release_image.cancel();

  capture_images_.insert(std::move(capture_image));
  return ZX_OK;
}

void Client::StartCapture(StartCaptureRequestView request, StartCaptureCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::StartCapture");

  // Ensure display driver supports/implements capture.
  if (!controller_.supports_capture()) {
    completer.ReplyError(ZX_ERR_NOT_SUPPORTED);
    return;
  }

  // Don't start capture if one is in progress.
  if (current_capture_image_id_ != display::kInvalidImageId) {
    completer.ReplyError(ZX_ERR_SHOULD_WAIT);
    return;
  }

  // Ensure we have a capture fence for the request signal event.
  auto signal_fence = fences_.GetFence(display::EventId(request->signal_event_id));
  if (signal_fence == nullptr) {
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  // Ensure we are capturing into a valid image buffer.
  const display::ImageId capture_image_id = display::ImageId(request->image_id);
  auto image = capture_images_.find(capture_image_id);
  if (!image.IsValid()) {
    fdf::error("Invalid Capture Image ID requested for capture");
    completer.ReplyError(ZX_ERR_INVALID_ARGS);
    return;
  }

  capture_fence_id_ = display::EventId(request->signal_event_id);
  zx::result<> result =
      controller_.engine_driver_client()->StartCapture(image->driver_capture_image_id());
  if (result.is_error()) {
    completer.ReplyError(result.error_value());
    return;
  }

  enable_capture_ = true;
  completer.ReplySuccess();

  // Keep track of currently active capture image.
  current_capture_image_id_ = capture_image_id;  // TODO: Is this right?
}

void Client::SetMinimumRgb(SetMinimumRgbRequestView request,
                           SetMinimumRgbCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetMinimumRgb");

  if (!is_owner_) {
    completer.ReplyError(ZX_ERR_NOT_CONNECTED);
    return;
  }
  zx::result<> result = controller_.engine_driver_client()->SetMinimumRgb(request->minimum_rgb);
  if (result.is_error()) {
    completer.ReplyError(result.error_value());
    return;
  }
  client_minimum_rgb_ = request->minimum_rgb;
  completer.ReplySuccess();
}

void Client::SetDisplayPowerMode(SetDisplayPowerModeRequestView request,
                                 SetDisplayPowerModeCompleter::Sync& completer) {
  TRACE_DURATION("gfx", "Display::Client::SetDisplayPowerMode");

  const display::DisplayId display_id = display::DisplayId(request->display_id);
  auto display_configs_it = display_configs_.find(display_id);
  if (!display_configs_it.IsValid()) {
    fdf::warn("SetDisplayPowerMode called with unknown display ID: {}", display_id.value());
    completer.ReplyError(ZX_ERR_NOT_FOUND);
  }

  const display::PowerMode power_mode(request->power_mode);
  zx::result<> result =
      controller_.engine_driver_client()->SetDisplayPowerMode(display_id, power_mode);
  if (result.is_error()) {
    completer.ReplyError(result.error_value());
    return;
  }
  completer.ReplySuccess();
}

display::ConfigCheckResult Client::CheckConfigImpl() {
  TRACE_DURATION("gfx", "Display::Client::CheckConfig");

  for (const DisplayConfig& display_config : display_configs_) {
    if (display_config.draft_layers_.is_empty()) {
      // `SetDisplayLayers()` prevents the client from directly specifying an
      // empty layer list for a display. However, this can still happen if the
      // client put together a display configuration, a new display was added to
      // the system, and the client called CheckConfig() or CommitConfig() before
      // it received the display change event.
      //
      // Skipping over the newly added display is appropriate, because display
      // engine drivers must support operating the hardware between the moment a
      // display is added and the moment it receives its first configuration.
      continue;
    }

    // Required to get display preferred modes.

    zx::result<std::span<const display::ModeAndId>> preferred_modes_result =
        controller_.GetDisplayPreferredModes(display_config.id());
    if (preferred_modes_result.is_error()) {
      fdf::error("Failed to get display preferred modes for display ID {}: {}",
                 display_config.id().value(), preferred_modes_result);
      return display::ConfigCheckResult::kUnsupportedConfig;
    }

    std::span<const display::ModeAndId> preferred_modes = preferred_modes_result.value();
    return CheckConfigForDisplay(display_config, preferred_modes);
  }

  // The client needs to process display changes and prepare a configuration
  // that accounts for the added / removed displays.
  return display::ConfigCheckResult::kEmptyConfig;
}

display::ConfigCheckResult Client::CheckConfigForDisplay(
    const DisplayConfig& display_config, std::span<const display::ModeAndId> preferred_modes) {
  ZX_DEBUG_ASSERT(!display_config.draft_layers_.is_empty());

  // Frame used for checking that each layer's `display_destination` lies
  // entirely within the display output.

  const display::ModeId draft_mode_id(display_config.draft_.mode_id);
  if (draft_mode_id == display::kInvalidModeId) {
    return display::ConfigCheckResult::kInvalidConfig;
  }

  auto mode_it = std::ranges::find_if(preferred_modes, [&](const display::ModeAndId& mode_and_id) {
    return mode_and_id.id() == draft_mode_id;
  });
  if (mode_it == preferred_modes.end()) {
    fdf::error("SetDisplayMode called with unknown mode ID: {}", draft_mode_id.value());
    return display::ConfigCheckResult::kUnsupportedDisplayModes;
  }
  display::Rectangle display_area({});
  display_area = {{
      .x = 0,
      .y = 0,
      .width = mode_it->mode().active_area().width(),
      .height = mode_it->mode().active_area().height(),
  }};

  cpp26::inplace_vector<display::DriverLayer, display::EngineInfo::kMaxAllowedMaxLayerCount>
      driver_layers;

  // The cast will not result in UB because the maximum layer count is
  // guaranteed to be positive.
  const size_t max_layer_count = display_config.engine_max_layer_count();
  ZX_DEBUG_ASSERT_MSG(max_layer_count > 0,
                      "DisplayConfig contract broken: engine_max_layer_count() must be positive");

  // Normalize the display configuration, and perform Coordinator-level
  // checks. The engine drivers API contract does not allow passing
  // configurations that fail these checks.
  for (const LayerNode& draft_layer_node : display_config.draft_layers_) {
    const display::DriverLayer& driver_layer = draft_layer_node.layer->draft_layer_config_;
    driver_layers.push_back(driver_layer);
    ZX_DEBUG_ASSERT(driver_layers.size() <= max_layer_count);

    if (driver_layer.image_source().width() != 0 && driver_layer.image_source().height() != 0) {
      // Frame for checking that the layer's `image_source` lies entirely within
      // the source image.
      const display::Rectangle image_area({
          .x = 0,
          .y = 0,
          .width = driver_layer.image_metadata().dimensions().width(),
          .height = driver_layer.image_metadata().dimensions().height(),
      });
      if (!OriginRectangleContains(image_area, driver_layer.image_source())) {
        return display::ConfigCheckResult::kInvalidConfig;
      }

      // The formats of layer images are negotiated by sysmem between clients
      // and display engine drivers when being imported, so they are always
      // accepted by the display coordinator.
    }
    if (!OriginRectangleContains(display_area, driver_layer.display_destination())) {
      return display::ConfigCheckResult::kInvalidConfig;
    }
  }

  DriverDisplayConfig draft_driver_display_config = display_config.draft_;
  ZX_DEBUG_ASSERT_MSG(
      static_cast<size_t>(draft_driver_display_config.layer_count) == driver_layers.size(),
      "Draft configuration layer count %d does not agree with list size %zu",
      draft_driver_display_config.layer_count, driver_layers.size());

  {
    TRACE_DURATION("gfx", "Display::Client::CheckConfig engine_driver_client");
    return controller_.engine_driver_client()->CheckConfiguration(draft_driver_display_config,
                                                                  driver_layers);
  }
}

void Client::SubmitLastCommittedConfig() {
  if (latest_config_stamp_ != display::kInvalidConfigStamp) {
    SubmitConfig();
  }
}

void Client::SubmitConfig() {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());
  TRACE_DURATION("gfx", "Display::Client::CommitConfig internal");

  ZX_DEBUG_ASSERT_MSG(!layers_.is_empty(), "Empty layers during SubmitConfig");

  bool config_missing_image = false;

  // Layers may have pending images, and it is possible that a layer still
  // uses images from previous configurations. We should take this into account
  // when sending the config_stamp to `Controller`.
  //
  // We keep track of the "current client config stamp" for each image, the
  // value of which is only updated when a configuration uses an image that is
  // ready on application, or when the image's wait fence has been signaled and
  // `ActivateLatestReadyImage()` activates the new image.
  //
  // The final config_stamp sent to `Controller` will be the minimum of all
  // per-layer stamps.
  display::ConfigStamp displayed_config_stamp = latest_config_stamp_;

  for (DisplayConfig& display_config : display_configs_) {
    display_config.committed_.layer_count = 0;

    // Displays with no current layers are filtered out in `Controller::SubmitConfig`,
    // after it updates its own image tracking logic.

    for (LayerNode& applied_layer_node : display_config.committed_layers_) {
      display_config.committed_.layer_count++;
      Layer* applied_layer = applied_layer_node.layer;
      const bool activated = applied_layer->ActivateLatestReadyImage();
      if (activated && applied_layer->committed_image()) {
        display_config.pending_commit_layer_change_ = true;
        display_config.pending_commit_layer_change_property_.Set(true);
      }

      // This is subtle. Compute the config stamp for this config as the *earliest* stamp of any
      // `Image` that appears on a `Layer` in this config. The goal is to satisfy the contract of
      // the `displayed_config_stamp` field of `CoordinatorListener.OnVsync()`, which returns the
      // config stamp of the latest *fully applied* config. For example, a config is not fully
      // applied if one of the images in the config is still waiting on a fence, even if the other
      // images in the config have appeared on-screen.
      //
      // TODO(https://fxbug.dev/449807074): This can cause a Vsync event with the wrong config stamp
      // to be sent. This occurs when the value returned by `GetCurrentClientConfigStamp()` has not
      // been updated to reflect the most recent config where the layer/image is used (the precise
      // details are not yet understood).
      std::optional<display::ConfigStamp> applied_layer_client_config_stamp =
          applied_layer->GetCurrentClientConfigStamp();
      if (applied_layer_client_config_stamp != std::nullopt) {
        displayed_config_stamp =
            std::min(displayed_config_stamp, *applied_layer_client_config_stamp);
      }

      bool is_solid_color_fill =
          applied_layer->committed_layer_config_.image_source().width() == 0 ||
          applied_layer->committed_layer_config_.image_source().height() == 0;
      if (!is_solid_color_fill) {
        if (applied_layer->committed_image() == nullptr) {
          config_missing_image = true;
        }
      }
    }
  }

  if (!config_missing_image && is_owner_) {
    for (DisplayConfig& display_config : display_configs_) {
      controller_.SubmitConfig(display_config, displayed_config_stamp, id_);
    }
  }
}

void Client::SetOwnership(bool is_owner) {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());
  is_owner_property_.Set(is_owner);
  is_owner_ = is_owner;

  NotifyOwnershipChange(/*client_has_ownership=*/is_owner);

  // TODO(costan): The config should only be submitted if the client owns the
  // displays. Land this in a separate behavior-changing CL.
  SubmitLastCommittedConfig();
}

void Client::NotifyDisplayChanges(
    std::span<const fuchsia_hardware_display::wire::Info> added_display_infos,
    std::span<const fuchsia_hardware_display_types::wire::DisplayId> removed_display_ids) {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());

  if (!coordinator_listener_.is_valid()) {
    return;
  }

  // TODO(https://fxbug.dev/42052765): `OnDisplayChanged()` takes `VectorView`s
  // of non-const display `Info` and `display::DisplayId` types though it doesn't modify
  // the vectors. We have to perform a `const_cast` to drop their constness.
  std::span<fuchsia_hardware_display::wire::Info> non_const_added_display_infos(
      const_cast<fuchsia_hardware_display::wire::Info*>(added_display_infos.data()),
      added_display_infos.size());
  std::span<fuchsia_hardware_display_types::wire::DisplayId> non_const_removed_display_ids(
      const_cast<fuchsia_hardware_display_types::wire::DisplayId*>(removed_display_ids.data()),
      removed_display_ids.size());

  fidl::OneWayStatus fidl_transport_status = coordinator_listener_->OnDisplaysChanged(
      fidl::VectorView<fuchsia_hardware_display::wire::Info>::FromExternal(
          non_const_added_display_infos.data(), non_const_added_display_infos.size()),
      fidl::VectorView<fuchsia_hardware_display_types::wire::DisplayId>::FromExternal(
          non_const_removed_display_ids.data(), non_const_removed_display_ids.size()));
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling OnDisplaysChanged: {}", fidl_transport_status.error());
  }
}

void Client::NotifyOwnershipChange(bool client_has_ownership) {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());

  if (!coordinator_listener_.is_valid()) {
    return;
  }

  fidl::OneWayStatus fidl_transport_status =
      coordinator_listener_->OnClientOwnershipChange(client_has_ownership);
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling OnClientOwnershipChange: {}", fidl_transport_status.error());
  }
}

void Client::NotifyVsync(display::DisplayId display_id, zx::time_monotonic timestamp,
                         display::ConfigStamp config_stamp,
                         display::VsyncAckCookie vsync_ack_cookie) {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());

  if (!coordinator_listener_.is_valid()) {
    return;
  }

  fidl::OneWayStatus fidl_transport_status = coordinator_listener_->OnVsync(
      display_id.ToFidl(), timestamp, config_stamp.ToFidl(), vsync_ack_cookie.ToFidl());
  if (!fidl_transport_status.ok()) {
    fdf::error("FIDL error calling OnVsync: {}", fidl_transport_status.error());
  }
}

void Client::OnDisplaysChanged(std::span<const display::DisplayId> added_display_ids,
                               std::span<const display::DisplayId> removed_display_ids) {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());
  for (display::DisplayId added_display_id : added_display_ids) {
    zx::result get_supported_pixel_formats_result =
        controller_.GetSupportedPixelFormats(added_display_id);
    if (get_supported_pixel_formats_result.is_error()) {
      fdf::warn("Failed to get pixel formats when processing hotplug: {}",
                get_supported_pixel_formats_result);
      continue;
    }

    fbl::AllocChecker alloc_checker;
    auto display_config = fbl::make_unique_checked<DisplayConfig>(
        &alloc_checker, added_display_id, std::move(get_supported_pixel_formats_result).value(),
        controller_.engine_info().max_layer_count());
    if (!alloc_checker.check()) {
      fdf::warn("Out of memory when processing hotplug");
      continue;
    }

    zx::result<std::span<const display::ModeAndId>> display_preferred_modes_result =
        controller_.GetDisplayPreferredModes(display_config->id());
    if (display_preferred_modes_result.is_error()) {
      fdf::warn("Failed to get display preferred modes when processing hotplug: {}",
                display_preferred_modes_result);
      continue;
    }
    std::span<const display::ModeAndId> display_preferred_modes =
        std::move(display_preferred_modes_result).value();
    ZX_DEBUG_ASSERT(!display_preferred_modes.empty());

    const display::ModeAndId preferred_mode_and_id = display_preferred_modes[0];
    ZX_DEBUG_ASSERT(preferred_mode_and_id.id() != display::kInvalidModeId);

    display_config->committed_ = DriverDisplayConfig{
        .display_id = display_config->id(),
        .mode_id = preferred_mode_and_id.id(),
        .color_conversion = display::ColorConversion::kIdentity,
        .layer_count = 0,
    };
    display_config->draft_ = display_config->committed_;

    display_config->InitializeInspect(&node());

    display_configs_.insert(std::move(display_config));
  }

  // We need 2 loops, since we need to make sure we allocate the
  // correct size array in the FIDL response.
  std::vector<fhd::wire::Info> coded_configs;
  coded_configs.reserve(added_display_ids.size());

  // Hang on to modes values until we send the message.
  std::vector<std::vector<fuchsia_hardware_display_types::wire::Mode>> modes_vector;

  fidl::Arena arena;
  for (display::DisplayId added_display_id : added_display_ids) {
    auto display_configs_it = display_configs_.find(added_display_id);
    if (!display_configs_it.IsValid()) {
      // The display got removed before the display addition was processed and
      // reported to the client.
      continue;
    }
    const DisplayConfig& display_config = *display_configs_it;

    fhd::wire::Info fidl_display_info;
    fidl_display_info.id = display_config.id().ToFidl();
    fidl_display_info.max_layer_count = display_config.engine_max_layer_count();

    zx::result<std::span<const display::ModeAndId>> display_preferred_modes_result =
        controller_.GetDisplayPreferredModes(display_config.id());
    ZX_DEBUG_ASSERT(display_preferred_modes_result.is_ok());

    std::span<const display::ModeAndId> display_preferred_modes =
        std::move(display_preferred_modes_result).value();
    ZX_DEBUG_ASSERT(!display_preferred_modes.empty());

    std::vector<fuchsia_hardware_display_types::wire::Mode> fidl_modes;
    fidl_modes.reserve(display_preferred_modes.size());
    for (const display::ModeAndId& mode_and_id : display_preferred_modes) {
      fidl_modes.emplace_back(mode_and_id.mode().ToFidl());
    }
    modes_vector.emplace_back(std::move(fidl_modes));
    fidl_display_info.modes =
        fidl::VectorView<fuchsia_hardware_display_types::wire::Mode>::FromExternal(
            modes_vector.back());

    fidl_display_info.pixel_format = fidl::VectorView<fuchsia_images2::wire::PixelFormat>(
        arena, display_config.pixel_formats_.size());
    for (size_t pixel_format_index = 0; pixel_format_index < fidl_display_info.pixel_format.size();
         ++pixel_format_index) {
      fidl_display_info.pixel_format[pixel_format_index] =
          display_config.pixel_formats_[pixel_format_index].ToFidl();
    }

    const bool found_display_info =
        controller_.FindDisplayInfo(added_display_id, [&](const DisplayInfo& display_info) {
          fidl_display_info.manufacturer_name =
              fidl::StringView::FromExternal(display_info.GetManufacturerName());
          fidl_display_info.monitor_name = fidl::StringView(arena, display_info.GetMonitorName());
          fidl_display_info.monitor_serial =
              fidl::StringView(arena, display_info.GetMonitorSerial());

          // The return value of `GetHorizontalSizeMm()` is guaranteed to be `0 <= value < 2^16`,
          // so it can be safely cast to `uint32_t`.
          fidl_display_info.horizontal_size_mm =
              static_cast<uint32_t>(display_info.GetHorizontalSizeMm());

          // The return value of `GetVerticalSizeMm()` is guaranteed to be `0 <= value < 2^16`,
          // so it can be safely cast to uint32_t.
          fidl_display_info.vertical_size_mm =
              static_cast<uint32_t>(display_info.GetVerticalSizeMm());
        });
    if (!found_display_info) {
      fdf::error("Failed to get DisplayInfo for display {}", added_display_id.value());
      ZX_DEBUG_ASSERT(false);
    }

    fidl_display_info.using_fallback_size = false;
    if (fidl_display_info.horizontal_size_mm == 0 || fidl_display_info.vertical_size_mm == 0) {
      fidl_display_info.horizontal_size_mm = controller_.fallback_horizontal_size_mm();
      fidl_display_info.vertical_size_mm = controller_.fallback_vertical_size_mm();
      fidl_display_info.using_fallback_size = true;
    }

    coded_configs.push_back(fidl_display_info);
  }

  std::vector<fhdt::wire::DisplayId> fidl_removed_display_ids;
  fidl_removed_display_ids.reserve(removed_display_ids.size());

  for (display::DisplayId removed_display_id : removed_display_ids) {
    std::unique_ptr<DisplayConfig> display_config = display_configs_.erase(removed_display_id);
    if (display_config != nullptr) {
      display_config->draft_layers_.clear();
      display_config->committed_layers_.clear();
      fidl_removed_display_ids.push_back(display_config->id().ToFidl());
    }
  }

  if (!coded_configs.empty() || !fidl_removed_display_ids.empty()) {
    NotifyDisplayChanges(coded_configs, fidl_removed_display_ids);
  }
}

void Client::OnFenceSignaled(Fence& fence) {
  bool new_image_ready = false;
  for (auto& layer : layers_) {
    new_image_ready |= layer.MarkFenceReady(fence);
  }
  if (new_image_ready) {
    SubmitConfig();
  }
}

void Client::CaptureCompleted() {
  auto signal_fence = fences_.GetFence(capture_fence_id_);
  if (signal_fence != nullptr) {
    signal_fence->Signal();
  }

  // Release the pending capture image, if there is one.
  if (pending_release_capture_image_id_ != display::kInvalidImageId) {
    auto image = capture_images_.find(pending_release_capture_image_id_);
    if (image.IsValid()) {
      capture_images_.erase(image);
    }
    pending_release_capture_image_id_ = display::kInvalidImageId;
  }
  current_capture_image_id_ = display::kInvalidImageId;
}

void Client::CloseFidlConnection(zx_status_t epitaph) {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());

  TRACE_DURATION("gfx", "Display::Client::CloseFidlConnection");
  fdf::info("Closing FIDL connection for client: priority {}, ID {}", priority_, id_.value());

  // See `fuchsia.hardware.display/Coordinator` protocol documentation in `coordinator.fidl`,
  // which describes the epitaph values that will be set when the channel closes.
  switch (epitaph) {
    case ZX_ERR_INVALID_ARGS:
    case ZX_ERR_BAD_STATE:
    case ZX_ERR_NO_MEMORY:
    case ZX_OK:
      break;
    default:
      fdf::info(
          "CloseFidlConnection() called with epitaph {}; using catchall ZX_ERR_INTERNAL instead",
          zx::make_result(epitaph));
      epitaph = ZX_ERR_INTERNAL;
  }

  binding_.Close(epitaph);
  coordinator_listener_ = fidl::WireSyncClient<fuchsia_hardware_display::CoordinatorListener>();
}

void Client::ReleaseResources() {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());
  ZX_DEBUG_ASSERT_MSG(!release_resources_called_, "ReleaseResources() already called");

  TRACE_DURATION("gfx", "Display::Client::ReleaseResources");
  fdf::info("Releasing resources for client: priority {}, ID {}", priority_, id_.value());

  release_resources_called_ = true;
  draft_display_config_was_validated_ = false;

  CleanUpAllImages();
  fdf::info("Releasing {} capture images cur={}, pending={}", capture_images_.size(),
            current_capture_image_id_.value(), pending_release_capture_image_id_.value());
  current_capture_image_id_ = pending_release_capture_image_id_ = display::kInvalidImageId;
  capture_images_.clear();

  fences_.Clear();

  for (DisplayConfig& display_config : display_configs_) {
    display_config.draft_layers_.clear();
    display_config.committed_layers_.clear();
  }

  // The layer's images have already been handled in `CleanUpImageLayerState`.
  layers_.clear();

  // Release all imported buffer collections on display drivers.
  for (const auto& [k, v] : collection_map_) {
    zx::result<> result =
        controller_.engine_driver_client()->ReleaseBufferCollection(v.driver_buffer_collection_id);
    if (result.is_error()) {
      fdf::warn("Failed to release buffer collection {} on driver: {}",
                v.driver_buffer_collection_id.value(), result.status_string());
    }
  }
  collection_map_.clear();
}

bool Client::CleanUpAllImages() {
  // Clean up any layer state associated with the images.
  bool current_config_changed = [&] {
    // We need to clean up images for all layers and thus should not
    // short-circuit here.
    bool any_layer_changed = false;
    for (Layer& layer : layers_) {
      any_layer_changed |= layer.CleanUpAllImages();
    }
    return any_layer_changed;
  }();

  images_.clear();
  return current_config_changed;
}

bool Client::CleanUpImage(Image& image) {
  // Clean up any layer state associated with the images.
  bool current_config_changed = [&] {
    // We need to clean up images for all layers and thus should not
    // short-circuit here.
    bool any_layer_changed = false;
    for (Layer& layer : layers_) {
      any_layer_changed |= layer.CleanUpImage(image);
    }
    return any_layer_changed;
  }();

  images_.erase(image);
  return current_config_changed;
}

void Client::CleanUpCaptureImage(display::ImageId id) {
  if (id == display::kInvalidImageId) {
    return;
  }
  // If the image is currently active, the underlying driver will retain a
  // handle to it until the hardware can be reprogrammed.
  auto image = capture_images_.find(id);
  if (image.IsValid()) {
    capture_images_.erase(image);
  }
}

void Client::SetAllConfigDraftLayersToCommittedLayers() {
  // Layers may have been moved between displays, so we must be extra careful
  // to avoid inserting a Layer in a display's draft list while it's
  // already moved to another Display's draft list.
  //
  // We side-step this problem by clearing all draft lists before inserting
  // any Layer in them, so that we can guarantee that for every Layer, its
  // `draft_node_` is not in any Display's draft list.
  for (DisplayConfig& display_config : display_configs_) {
    display_config.draft_layers_.clear();
  }
  for (DisplayConfig& display_config : display_configs_) {
    // Rebuild the draft layers list from the committed layers list.
    for (LayerNode& layer_node : display_config.committed_layers_) {
      display_config.draft_layers_.push_back(&layer_node.layer->draft_display_config_list_node_);
    }
  }
}

void Client::DiscardConfig() {
  TRACE_DURATION("gfx", "Display::Client::DiscardConfig");

  // Go through layers and release any resources claimed by draft configs.
  for (Layer& layer : layers_) {
    layer.DiscardChanges();
  }

  // Discard layer list changes.
  SetAllConfigDraftLayersToCommittedLayers();

  // Discard the rest of the Display changes.
  for (DisplayConfig& display_config : display_configs_) {
    display_config.DiscardNonLayerDraftConfig();
  }
  draft_display_config_was_validated_ = true;
}

void Client::AcknowledgeVsync(AcknowledgeVsyncRequestView request,
                              AcknowledgeVsyncCompleter::Sync& completer) {
  display::VsyncAckCookie ack_cookie = display::VsyncAckCookie(request->cookie);
  if (ack_cookie == display::kInvalidVsyncAckCookie) {
    fdf::error("AcknowledgeVsync() called with invalid cookie");
    completer.Close(ZX_ERR_INVALID_ARGS);
    return;
  }

  if (!vsync_queue_.Acknowledge(ack_cookie)) {
    fdf::error("Client passed incorrect VSync ack cookie: {}", ack_cookie.value());
  }
  DrainVsyncQueue();
  fdf::trace("Cookie {} Acked\n", ack_cookie.value());
}

Client::Client(
    Controller* controller, display::ClientPriority priority, ClientId client_id,
    fidl::ServerEnd<fuchsia_hardware_display::Coordinator> coordinator_server_end,
    fidl::ClientEnd<fuchsia_hardware_display::CoordinatorListener> coordinator_listener_client_end)
    : controller_(*controller),
      priority_(priority),
      id_(client_id),
      fences_(this, controller->driver_dispatcher()->borrow()),
      binding_(controller_.driver_dispatcher()->async_dispatcher(),
               std::move(coordinator_server_end), this,
               std::mem_fn(&Client::OnClientFidlBindingClosed)),
      coordinator_listener_(std::move(coordinator_listener_client_end)) {
  ZX_DEBUG_ASSERT(controller != nullptr);
  ZX_DEBUG_ASSERT(priority != display::ClientPriority::kInvalid);
  ZX_DEBUG_ASSERT(client_id != kInvalidClientId);
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());
  ZX_DEBUG_ASSERT(coordinator_listener_.is_valid());
}

Client::~Client() {
  ZX_DEBUG_ASSERT(release_resources_called_);
  ZX_DEBUG_ASSERT(attach_inspect_node_called_);

  ZX_DEBUG_ASSERT(layers_.size() == 0);
}

void Client::SubmitSpecialConfigs() {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());

  zx::result<> result = controller_.engine_driver_client()->SetMinimumRgb(GetMinimumRgb());
  if (!result.is_ok()) {
    fdf::warn("Failed to submit minimum RGB value: {}", result);
  }
}

void Client::OnCaptureComplete() {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());

  if (enable_capture_) {
    CaptureCompleted();
  }
  enable_capture_ = false;
}

void Client::OnDisplayVsync(display::DisplayId display_id, zx_instant_mono_t timestamp,
                            display::DriverConfigStamp driver_config_stamp) {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());

  display::ConfigStamp client_stamp = {};
  auto it =
      std::find_if(pending_displayed_config_stamps_.begin(), pending_displayed_config_stamps_.end(),
                   [driver_config_stamp](const ConfigStampPair& stamp) {
                     return stamp.driver_stamp >= driver_config_stamp;
                   });

  if (it == pending_displayed_config_stamps_.end() || it->driver_stamp != driver_config_stamp) {
    client_stamp = display::kInvalidConfigStamp;
  } else {
    client_stamp = it->client_stamp;
    pending_displayed_config_stamps_.erase(pending_displayed_config_stamps_.begin(), it);
  }

  vsync_queue_.Push(ClientVsyncQueue::Message{.display_id = display_id,
                                              .timestamp = zx::time_monotonic(timestamp),
                                              .config_stamp = client_stamp});
  DrainVsyncQueue();
}

void Client::DrainVsyncQueue() {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());

  vsync_queue_.DrainUntilThrottled(
      [&](const ClientVsyncQueue::Message& message, display::VsyncAckCookie ack_cookie) {
        NotifyVsync(message.display_id, message.timestamp, message.config_stamp, ack_cookie);
      });
}

void Client::OnClientFidlBindingClosed(fidl::UnbindInfo unbind_info) {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());

  // Ultimately results in deleting `this`.
  controller_.OnClientDisconnected(this);
}

void Client::UpdateConfigStampMapping(ConfigStampPair stamps) {
  ZX_DEBUG_ASSERT(controller_.IsRunningOnDriverDispatcher());
  ZX_DEBUG_ASSERT(pending_displayed_config_stamps_.empty() ||
                  pending_displayed_config_stamps_.back().driver_stamp < stamps.driver_stamp);

  pending_displayed_config_stamps_.push_back({
      .driver_stamp = stamps.driver_stamp,
      .client_stamp = stamps.client_stamp,
  });
}

void Client::AttachInspectNode(inspect::Node client_node) {
  ZX_DEBUG_ASSERT_MSG(!attach_inspect_node_called_, "AttachInspectNode() already called");

  attach_inspect_node_called_ = true;
  node_ = std::move(client_node);
  node_.RecordUint("priority", priority().ValueForLogging());
  is_owner_property_ = node_.CreateBool("is_owner", false);
}

}  // namespace display_coordinator
