// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_LAYER_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_LAYER_H_

#include "src/ui/scenic/lib/display/fidl_id_types.h"
#include "src/ui/scenic/lib/display/internal/layer_equivalence.h"

namespace display::internal {

// Manages the state of a single display layer within the `CoordinatorProxy`.
// This class maintains a "draft" state, representing pending changes, and an "applied" state,
// representing the configuration last known to be successfully applied on the display hardware via
// the FIDL `ApplyConfig()` method.
//
// The `Layer` also holds a `LayerEquivalence`, which is a projection of the layer's state used by
// `CoordinatorProxy` to determine `CheckConfig()` equivalency.
//
// Workflow:
// 1.  Calls to `Set*()` methods update the `draft_equiv_` and other draft fields
//     (e.g., `draft_image_`).
// 2.  `CoordinatorProxy` calls `SendDiffsToCoordinator()`. This method compares the current
//     `draft_equiv_` and `draft_image_` against the `applied_equiv_` and `applied_image_`.  It
//     sends the minimal set of FIDL commands to the `fuchsia.hardware.display.Coordinator` to make
//     the coordinator's layer state match this `Layer`'s draft state.
// 3a. *After* a successful `CheckConfig()` and `ApplyConfig()` sequence in `CoordinatorProxy`, the
//     proxy calls this `Layer`'s `AcceptDraftState()` method to promote the draft state to the
//     applied state.  Or:
// 3b. If `CheckConfig()` fails, `CoordinatorProxy` calls `ResetDraftState()` to
//     revert the draft state back to the last known good applied state.
class Layer {
 public:
  // Values that are not directly held in a `LayerEquivalence`.  For example, the specific alpha
  // value is irrelevant from the POV of a `LayerEquivalence`, which only cares if the value is one,
  // zero, or between the two.
  struct ConfigValues {
    float alpha_value = 1.f;
  };

  Layer() = default;

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerPrimaryConfig()`.
  // Modifies the layer's draft state; no FIDL methods are invoked.
  //
  // Note: calling this clears the image, as done by the coordinator impl.
  void SetPrimaryConfig(const Extent2& image_dimensions, uint32_t image_tiling_type);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerPrimaryPosition()`.
  // Modifies the layer's draft state; no FIDL methods are invoked.
  void SetPrimaryPosition(const RotateFlip& transform, const Rectangle& src, const Rectangle& dst);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerPrimaryAlpha()`.
  // Modifies the layer's draft state; no FIDL methods are invoked.
  void SetPrimaryAlpha(const BlendMode& blend_mode, float alpha_value);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerPrimaryImage()`.
  // Modifies the layer's draft state; no FIDL methods are invoked.
  void SetLayerImage(const ImageId& image_id, const EventId& wait_event_id);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerColorConfig()`.
  // Modifies the layer's draft state; no FIDL methods are invoked.
  void SetColorConfig(const WireColor& color, const Rectangle& display_destination);

  // If either the draft or applied state includes `image_id`, replace with `kInvalidImageId`.
  void UnsetImage(const ImageId& image_id);

  // Compute the diffs between the draft and applied states, and make only the necessary FIDL calls
  // to make the Coordinator's draft state match this layer's draft state.  Returns the number of
  // FIDL API calls that were sent.
  size_t SendDiffsToCoordinator(
      const LayerId& layer_id,
      fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& coordinator);

  // Reset all draft state to be identical to the currently-applied state.
  void ResetDraftState();

  // Replace the accepted state with the draft state.  Afterward, they are identical.
  void AcceptDraftState();

  const LayerEquivalence& draft_equiv() const { return draft_equiv_; }

 private:
  // Helper for `SendDiffsToCoordinator()`.
  size_t SendImageLayerDiffsToCoordinator(
      const LayerId& layer_id,
      fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& coordinator);

  // Helper for `SendDiffsToCoordinator()`.
  size_t SendColorLayerDiffsToCoordinator(
      const LayerId& layer_id,
      fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& coordinator);

  LayerEquivalence applied_equiv_;
  ConfigValues applied_values_;
  ImageId applied_image_ = kInvalidImageId;
  EventId applied_wait_event_ = kInvalidEventId;

  LayerEquivalence draft_equiv_;
  ConfigValues draft_values_;
  ImageId draft_image_ = kInvalidImageId;
  EventId draft_wait_event_ = kInvalidEventId;
};

}  // namespace display::internal

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_INTERNAL_LAYER_H_
