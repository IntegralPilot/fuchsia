// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/internal/layer.h"

#include "src/ui/scenic/lib/utils/logging.h"

// Allows us to manually change this to enable logging without *all* Flatland verbose logging.
#define CP_VERBOSE_LOG FLATLAND_VERBOSE_LOG

namespace display::internal {

void Layer::SetPrimaryConfig(const Extent2& image_dimensions, uint32_t image_tiling_type) {
  if (!std::holds_alternative<ImageLayerEquivalence>(draft_equiv_.config)) {
    draft_equiv_ = ImageLayerEquivalence{};
  }
  auto& equiv = std::get<ImageLayerEquivalence>(draft_equiv_.config);
  equiv.image_dimensions = image_dimensions;
  equiv.image_tiling_type = image_tiling_type;

  draft_image_ = kInvalidImageId;
  draft_wait_event_ = kInvalidEventId;
}

void Layer::SetPrimaryPosition(const RotateFlip& transform, const Rectangle& src,
                               const Rectangle& dst) {
  FX_DCHECK(std::holds_alternative<ImageLayerEquivalence>(draft_equiv_.config));
  auto& equiv = std::get<ImageLayerEquivalence>(draft_equiv_.config);
  equiv.image_source_transformation = transform;
  equiv.display_destination = dst;
  equiv.image_source = src;
}

void Layer::SetPrimaryAlpha(const BlendMode& blend_mode, float alpha_value) {
  FX_DCHECK(std::holds_alternative<ImageLayerEquivalence>(draft_equiv_.config));
  auto& equiv = std::get<ImageLayerEquivalence>(draft_equiv_.config);
  equiv.blend_mode = blend_mode;
  equiv.alpha_range = ImageLayerEquivalence::MakeAlphaRange(alpha_value);

  draft_values_.alpha_value = alpha_value;
}

void Layer::SetLayerImage(const ImageId& image_id, const EventId& wait_event_id) {
  FX_DCHECK(image_id != kInvalidImageId);
  draft_image_ = image_id;
  draft_wait_event_ = wait_event_id;
}

void Layer::SetColorConfig(const WireColor& color, const Rectangle& display_destination) {
  draft_equiv_ = ColorLayerEquivalence{
      .color = color,
      .display_destination = display_destination,
  };
  draft_image_ = kInvalidImageId;
  draft_wait_event_ = kInvalidEventId;
}

void Layer::UnsetImage(const ImageId& image_id) {
  if (applied_image_ == image_id) {
    applied_image_ = kInvalidImageId;
  }
  if (draft_image_ == image_id) {
    draft_image_ = kInvalidImageId;
  }
}

size_t Layer::SendDiffsToCoordinator(
    const LayerId& layer_id,
    fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& shared_coordinator) {
  if (std::holds_alternative<ImageLayerEquivalence>(draft_equiv_.config)) {
    return SendImageLayerDiffsToCoordinator(layer_id, shared_coordinator);
  }
  if (std::holds_alternative<ColorLayerEquivalence>(draft_equiv_.config)) {
    return SendColorLayerDiffsToCoordinator(layer_id, shared_coordinator);
  }
  // Uninitialized layer, no diffs to send.
  FX_DCHECK(std::holds_alternative<UninitializedLayerEquivalence>(draft_equiv_.config));
  return 0;
}

void Layer::ResetDraftState() {
  draft_equiv_ = applied_equiv_;
  draft_image_ = applied_image_;
  draft_wait_event_ = applied_wait_event_;
}

void Layer::AcceptDraftState() {
  applied_equiv_ = draft_equiv_;
  applied_image_ = draft_image_;
  applied_wait_event_ = draft_wait_event_;
}

size_t Layer::SendImageLayerDiffsToCoordinator(
    const LayerId& layer_id,
    fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& shared_coordinator) {
  FX_DCHECK(std::holds_alternative<ImageLayerEquivalence>(draft_equiv_.config));
  const auto& draft_equiv = std::get<ImageLayerEquivalence>(draft_equiv_.config);
  const bool is_applying_image_config =
      std::holds_alternative<ImageLayerEquivalence>(applied_equiv_.config);

  const WireLayerId wire_layer_id = layer_id.ToFidl();

  size_t api_calls_sent = 0;

  bool must_set_config = false;
  bool must_set_position = false;
  bool must_set_alpha = false;
  bool must_set_image = false;

  // Determine which values must be set by calling Coordinator methods.
  if (!is_applying_image_config) {
    // Transitioning to image config from some other type of config (either color or uninitialized),
    // therefore must set all draft values.
    must_set_config = true;
    must_set_position = true;
    must_set_alpha = true;
    must_set_image = true;
  } else {
    const auto& applied_equiv = std::get<ImageLayerEquivalence>(applied_equiv_.config);

    must_set_config = draft_equiv.image_dimensions != applied_equiv.image_dimensions ||
                      draft_equiv.image_tiling_type != applied_equiv.image_tiling_type;
    must_set_position =
        draft_equiv.image_source_transformation != applied_equiv.image_source_transformation ||
        draft_equiv.image_source != applied_equiv.image_source ||
        draft_equiv.display_destination != applied_equiv.display_destination;
    must_set_alpha = draft_equiv.blend_mode != applied_equiv.blend_mode ||
                     draft_equiv.alpha_range != applied_equiv.alpha_range;
    // Setting the config clears the image in the Coordinator impl, so `must_set_config == true`
    // means that we must set the image even if it matches the already-applied image.
    must_set_image =
        (draft_image_ != kInvalidImageId) && (must_set_config || draft_image_ != applied_image_ ||
                                              draft_wait_event_ != applied_wait_event_);
  }

  if (must_set_config) {
    CP_VERBOSE_LOG << "Layer::SendImageLayerDiffsToCoordinator()... setting config";
    ++api_calls_sent;
    const fidl::OneWayStatus status = shared_coordinator->SetLayerPrimaryConfig(
        wire_layer_id, WireImageMetadata{.dimensions = draft_equiv.image_dimensions.ToWire(),
                                         .tiling_type = draft_equiv.image_tiling_type});
    FX_DCHECK(status.ok()) << "Failed to call FIDL SetLayerPrimaryConfig method: "
                           << status.status_string();
  }

  if (must_set_position) {
    CP_VERBOSE_LOG << "Layer::SendImageLayerDiffsToCoordinator()... setting position";
    ++api_calls_sent;
    const fidl::OneWayStatus status = shared_coordinator->SetLayerPrimaryPosition(
        wire_layer_id, draft_equiv.image_source_transformation.ToDisplayCoordinateTransformation(),
        draft_equiv.image_source.ToWireRectU(), draft_equiv.display_destination.ToWireRectU());

    FX_DCHECK(status.ok()) << "Failed to call FIDL SetLayerPrimaryPosition method: "
                           << status.status_string();
  }

  if (must_set_alpha) {
    CP_VERBOSE_LOG << "Layer::SendImageLayerDiffsToCoordinator()... setting alpha";
    ++api_calls_sent;
    const fidl::OneWayStatus status = shared_coordinator->SetLayerPrimaryAlpha(
        wire_layer_id, draft_equiv.blend_mode.ToDisplayAlphaMode(), draft_values_.alpha_value);

    FX_DCHECK(status.ok()) << "Failed to call FIDL SetLayerPrimaryAlpha method: "
                           << status.status_string();
  }

  // TODO(https://fxbug.dev/449807074): we must always send `SetLayerImage2` even if the image
  // hasn't changed, since otherwise the received Vsync config stamps can be wrong.
  const bool force_send_layer_image = draft_image_ != display::kInvalidImageId;
  if (force_send_layer_image || must_set_image) {
    CP_VERBOSE_LOG << "Layer::SendImageLayerDiffsToCoordinator()... setting image";
    ++api_calls_sent;
    const fidl::OneWayStatus status = shared_coordinator->SetLayerImage2(
        wire_layer_id, draft_image_.ToFidl(), draft_wait_event_.ToFidl());

    FX_DCHECK(status.ok()) << "Failed to call FIDL SetLayerImage2 method: "
                           << status.status_string();
  }

  return api_calls_sent;
}

size_t Layer::SendColorLayerDiffsToCoordinator(
    const LayerId& layer_id,
    fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& shared_coordinator) {
  FX_DCHECK(std::holds_alternative<ColorLayerEquivalence>(draft_equiv_.config));

  if (draft_equiv_ == applied_equiv_) {
    return 0;  // zero FIDL calls sent
  }
  CP_VERBOSE_LOG << "Layer::SendColorLayerDiffsToCoordinator()... setting color config";

  const auto& draft_equiv = std::get<ColorLayerEquivalence>(draft_equiv_.config);
  const fidl::OneWayStatus status = shared_coordinator->SetLayerColorConfig(
      layer_id.ToFidl(), draft_equiv.color, draft_equiv.display_destination.ToWireRectU());
  FX_DCHECK(status.ok()) << "Failed to call FIDL SetLayerImage2 method: " << status.status_string();
  return 1;  // one FIDL call sent
}

}  // namespace display::internal
