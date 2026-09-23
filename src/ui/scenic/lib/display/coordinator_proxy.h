// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_COORDINATOR_PROXY_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_COORDINATOR_PROXY_H_

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <lib/inspect/cpp/inspect.h>

#include <array>
#include <memory>
#include <span>
#include <unordered_map>
#include <unordered_set>
#include <vector>

#include "src/ui/scenic/lib/display/fidl_id_types.h"
#include "src/ui/scenic/lib/display/fidl_typedefs.h"
#include "src/ui/scenic/lib/display/internal/check_config_cache.h"
#include "src/ui/scenic/lib/display/internal/display_equivalence.h"
#include "src/ui/scenic/lib/display/internal/layer.h"
#include "src/ui/scenic/lib/display/typedefs.h"
#include "src/ui/scenic/lib/types/blend_mode.h"
#include "src/ui/scenic/lib/types/display_mode.h"
#include "src/ui/scenic/lib/types/extent2.h"
#include "src/ui/scenic/lib/types/rectangle.h"
#include "src/ui/scenic/lib/types/rotate_flip.h"

namespace display {

// CoordinatorProxy is a client-side wrapper around the fuchsia.hardware.display.Coordinator FIDL
// service. It optimizes interactions by batching configuration calls and caching CheckConfig
// results.
//
// Key features:
// - Defers most Set*() FIDL calls until ApplyConfig() is called.
// - Diffs the desired state against the last applied state to send only necessary FIDL commands.
// - Caches the results of CheckConfig() to avoid redundant FIDL round-trips for
//   previously-validated configurations.
// - Provides Inspect properties for monitoring API calls and cache behavior.
//
// Thread-safety: This class is thread-unsafe; concurrent access must be externally synchronized.
//
// Key limitations:
// - Although the API appears to support multiple displays, the implementation
//   only supports a single display.  The private method CheckDisplayId() is to
//   guarantee that clients do not try to interact with multiple displays.
class CoordinatorProxy {
 public:
  // Configure which checks are enabled within `HeuristicsSayThatCheckConfigWouldFail()`.
  struct CheckConfigHeuristics {
    // Currently, enabling heuristics is all-or-nothing.
    bool enable_heuristics = false;
  };

  explicit CoordinatorProxy(
      fidl::ClientEnd<fuchsia_hardware_display::Coordinator> coordinator,
      async_dispatcher_t* dispatcher,
      CheckConfigHeuristics check_config_heuristics = {.enable_heuristics = false},
      inspect::Node inspect_node = inspect::Node());

  explicit CoordinatorProxy(
      fidl::WireSharedClient<fuchsia_hardware_display::Coordinator> coordinator,
      CheckConfigHeuristics check_config_heuristics = {.enable_heuristics = false},
      inspect::Node inspect_node = inspect::Node());

  // Corresponds to `fuchsia.hardware.display.Coordinator/ImportImage()`.
  // Two-way FIDL call is sent immediately, and the result is awaited synchronously.
  //
  // Errors are either FIDL transport errors (most commonly ZX_ERR_PEER_CLOSED), or the error status
  // returned by the FIDL method.  The latter are currently not specified by the Coordinator API.
  zx::result<> ImportImage(types::Extent2 image_dimensions, uint32_t image_tiling_type,
                           WireBufferCollectionId buffer_collection_id, uint32_t buffer_index,
                           ImageId image_id);

  // Corresponds to `fuchsia.hardware.display.Coordinator/ReleaseImage()`.
  // One-way FIDL call is sent immediately.
  void ReleaseImage(const ImageId& image_id);

  // Corresponds to `fuchsia.hardware.display.Coordinator/ImportEvent()`.
  // One-way FIDL call is sent immediately.
  //
  // There are two flavors of `ImportEvent()`:
  // - basic: requires the caller to provide both a (consumed) event and an `EventId`
  // - convenient: generates an `EventId` and duplicates the event, then delegates to basic version
  void ImportEvent(zx::event event, const EventId& event_id);
  EventId ImportEvent(const zx::event& event);

  // Corresponds to `fuchsia.hardware.display.Coordinator/ReleaseEvent()`.
  // One-way FIDL call is sent immediately.
  void ReleaseEvent(const EventId& event_id);

  // Corresponds to `fuchsia.hardware.display.Coordinator/CreateLayer()`.
  // Two-way FIDL call is sent immediately, and the result is awaited synchronously.
  //
  // TODO(https://fxbug.dev/430976567): use one-way FIDL call after change to client-managed IDs.
  LayerId CreateLayer();

  // Corresponds to `fuchsia.hardware.display.Coordinator/DestroyLayer()`.
  // One-way FIDL call is sent immediately.
  void DestroyLayer(const LayerId& layer_id);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetDisplayMode()`.
  // No FIDL call is sent immediately; it is deferred until it is (maybe) sent in `ApplyConfig()`.
  //
  // `display_id` must not be `kInvalidDisplayId`.  Each invocation must use the same ID; multiple
  // displays are not supported.
  void SetDisplayMode(const DisplayId& display_id, const DisplayMode& mode);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetDisplayColorConversion()`.
  // No FIDL call is sent immediately; it is deferred until it is (maybe) sent in `ApplyConfig()`.
  //
  // `display_id` must not be `kInvalidDisplayId`.  Each invocation must use the same ID; multiple
  // displays are not supported.
  void SetDisplayColorConversion(const DisplayId& display_id,
                                 const std::array<float, 3>& preoffsets,
                                 const std::array<float, 9>& coefficients,
                                 const std::array<float, 3>& postoffsets);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetDisplayLayers()`.
  // No FIDL call is sent immediately; it is deferred until it is (maybe) sent in `ApplyConfig()`.
  //
  // `display_id` must not be `kInvalidDisplayId`.  Each invocation must use the same ID; multiple
  // displays are not supported.
  //
  // `layer_ids` must have at most the hardware layer limit elements. Violating this
  // precondition will cause the Display Coordinator to close the FIDL channel.
  void SetDisplayLayers(const DisplayId& display_id, const std::span<const LayerId>& layer_ids);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerPrimaryConfig()`.
  // No FIDL call is sent immediately; it is deferred until it is (maybe) sent in `ApplyConfig()`.
  void SetLayerPrimaryConfig(const LayerId& layer_id, const Extent2& image_dimensions,
                             uint32_t image_tiling_type);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerPrimaryPosition`.
  // No FIDL call is sent immediately; it is deferred until it is (maybe) sent in `ApplyConfig()`.
  void SetLayerPrimaryPosition(const LayerId& layer_id, const RotateFlip& transform,
                               const Rectangle& image_source, const Rectangle& display_destination);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerPrimaryAlpha`.
  // No FIDL call is sent immediately; it is deferred until it is (maybe) sent in `ApplyConfig()`.
  void SetLayerPrimaryAlpha(const LayerId& layer_id, const BlendMode& blend_mode,
                            float alpha_value);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerImage`.
  // No FIDL call is sent immediately; it is deferred until it is (maybe) sent in `ApplyConfig()`.
  void SetLayerImage(const LayerId& layer_id, const ImageId& image_id,
                     const EventId& wait_event_id);

  // Corresponds to `fuchsia.hardware.display.Coordinator/SetLayerColorConfig`.
  // No FIDL call is sent immediately; it is deferred until it is (maybe) sent in `ApplyConfig()`.
  void SetLayerColorConfig(const LayerId& layer_id, const WireColor& color,
                           const Rectangle& display_destination);

  // Augmented version of `fuchsia.hardware.display.Coordinator/ApplyConfig`.
  // Before the FIDL `ApplyConfig()` method is called, the following steps are taken:
  // - check whether the current draft configuration matches one that has already been cached
  //   - exit early if `CheckConfig()` would fail
  // - send any diffs between the draft and last-applied config
  // - call FIDL `CheckConfig()` method, if necessary
  // - call FIDL `ApplyConfig()` method
  //
  // Errors:
  // - ZX_BAD_STATE when `CheckConfig()` fails (or we know it would fail without calling it).
  zx::result<> ApplyConfig(const WireConfigStamp& config_stamp);

  // Currently only used for testing, but OK to use for other purposes.
  // (remove this comment before using outside of tests)
  uint64_t api_calls_received() const { return api_calls_received_; }
  uint64_t api_calls_sent() const { return api_calls_sent_; }
  uint64_t apply_config_calls_received() const { return apply_config_calls_received_; }
  uint64_t apply_config_calls_sent() const { return apply_config_calls_sent_; }
  uint64_t check_config_calls_skipped() const { return check_config_calls_skipped_; }
  uint64_t check_config_calls_sent() const { return check_config_calls_sent_; }

  // TODO(https://fxbug.dev/447416966): Fix call sites to remove the need for this escape hatch, at
  // least in production; maybe some test-only use cases will remain.
  fidl::WireSharedClient<fuchsia_hardware_display::Coordinator>& raw() { return coordinator_; }

  bool is_valid() const { return coordinator_.is_valid(); }

 private:
  // Returns layer after checking that it exists.
  internal::Layer& GetLayer(const LayerId& layer_id);

  // Enforces that only one unique `DisplayId` is ever allowed to be used by a `CoordinatorProxy`
  // instance.  It does this by caching the first `DisplayId` that is seen, and checking all
  // subsequent ones against it.
  void CheckDisplayId(const DisplayId& display_id);

  // Resets all draft state to be identical to the currently-applied state.
  void ResetDraftState();

  // Replaces the accepted state with the draft state.  Afterward, they are identical.
  void AcceptDraftState();

  // Checks for differences between the draft and applied state, and make FIDL calls to that the
  // Coordinator's state matches this proxy.
  void SendDiffsToCoordinator();

  // Computes a `DisplayEquivalence` that is used by `ApplyConfig()` as the key for finding/caching
  // the result of `FidlCheckConfig()` calls.
  //
  // This is the only method allowed to mutate `temp_display_equivalence_`.
  void UpdateDisplayEquivalenceForApplyConfig();

  // One-way call to Coordinator FIDL service.
  void FidlApplyConfig(const WireConfigStamp& config_stamp);

  // One-way call to Coordinator FIDL service.
  void FidlDiscardConfig();

  // Calls `fuchsia.hardware.display.Coordinator/CheckConfig()`.
  // Two-way FIDL call is sent immediately, and the result is awaited synchronously.
  //
  // Assumes that `SendDiffsToCoordinator()` has been sent, so that the local draft state matches
  // the FIDL server draft state.
  WireConfigResult FidlCheckConfig();

  // Caches the result of a CheckConfig call.
  void CacheCheckConfigResult(bool success);

  // Used by `ApplyConfig()` to detect invalid configurations, without invoking `CheckConfig()`.
  //
  // The equivalence classes defined by `DisplayEquivalence` and `LayerEquivalence` are based on
  // guarantees made by the `Coordinator` FIDL protocol.  These equivalence classes MAY NOT make use
  // of additional knowledge that is not guaranteed by the FIDL API (as intended/understood by the
  // display team).
  //
  // For example, at the time of writing, all existing driver implementations require full-screen
  // layers.  A heuristic based on this fact allows us to avoid thrashing the cache with
  // almost-identical configs that we cannot put in the same equivalence class.
  //
  // See the implementation for the specific checks that are used.
  bool HeuristicsSayThatCheckConfigWouldFail();

  // Each of these increments a count variable and the corresponding Inspect property.
  void IncrementApiCallsReceived() { inspect_api_calls_received_.Set(++api_calls_received_); }
  void IncrementApiCallsSent(uint64_t count = 1) {
    api_calls_sent_ += count;
    inspect_api_calls_sent_.Set(api_calls_sent_);
  }
  void IncrementApplyConfigCallsReceived() {
    inspect_apply_config_calls_received_.Set(++apply_config_calls_received_);
  }
  void IncrementApplyConfigCallsSent() {
    inspect_apply_config_calls_sent_.Set(++apply_config_calls_sent_);
  }
  void IncrementCheckConfigCallSkipped() {
    inspect_check_config_calls_skipped_.Set(++check_config_calls_skipped_);
  }
  void IncrementCheckConfigCallsSent() {
    inspect_check_config_calls_sent_.Set(++check_config_calls_sent_);
  }
  void IncrementImportImageCallsSent() {
    inspect_import_image_count_.Set(++import_image_calls_sent_);
  }
  void IncrementImportEventCallsSent() {
    inspect_import_event_count_.Set(++import_event_calls_sent_);
  }

  // Each of these updates an Inspect property to reflect the count of the object type.
  void UpdateCurrentImageCount() { inspect_current_image_count_.Set(images_.size()); }
  void UpdateCurrentEventCount() { inspect_current_event_count_.Set(events_.size()); }

  // Only one display is supported.  The first `DisplayId` that is seen must be the one that is used
  // forever after; this should be asserted by using `CheckDisplayId()`.
  DisplayId display_id_ = kInvalidDisplayId;

  fidl::WireSharedClient<fuchsia_hardware_display::Coordinator> coordinator_;

  std::unordered_map<LayerId, internal::Layer> layers_;
  std::unordered_set<ImageId> images_;
  std::unordered_set<EventId> events_;

  std::vector<LayerId> applied_display_layers_;
  std::vector<LayerId> draft_display_layers_;

  DisplayMode applied_display_mode_;
  DisplayMode draft_display_mode_;

  // Default color conversion values match those in the display coordinator.
  // See `display::ColorConversion::kIdentity` in
  // `//src/graphics/display/lib/api-types/cpp/color-conversion.h`
  std::array<float, 3> applied_color_conversion_preoffsets_ = {0.f, 0.f, 0.f};
  std::array<float, 9> applied_color_conversion_coefficients_ = {1.0f, 0.0f, 0.0f, 0.0f, 1.0f,
                                                                 0.0f, 0.0f, 0.0f, 1.0f};
  std::array<float, 3> applied_color_conversion_postoffsets_ = {0.f, 0.f, 0.f};
  std::array<float, 3> draft_color_conversion_preoffsets_ = {0.f, 0.f, 0.f};
  std::array<float, 9> draft_color_conversion_coefficients_ = {1.0f, 0.0f, 0.0f, 0.0f, 1.0f,
                                                               0.0f, 0.0f, 0.0f, 1.0f};
  std::array<float, 3> draft_color_conversion_postoffsets_ = {0.f, 0.f, 0.f};

  // This `DisplayEquivalence` is used as a temporary variable within the `ApplyConfig()` method to
  // hold a representation of the current draft configuration *relevant for `CheckConfig()`
  // equivalency*. This "spec" is a subset of the total display state, containing only the fields
  // that affect the hardware's ability to display the configuration.
  //
  // It is stored as a member variable to avoid repeated heap allocations for its internal vectors,
  // thereby optimizing performance.
  //
  // It is declared const to prevent accidental modifications in other methods.  However, it is
  // mutated *only* within the `UpdateDisplayEquivalenceForApplyConfig()` method, which uses a
  // const_cast for this specific purpose.
  const internal::DisplayEquivalence temp_display_equivalence_ = {};

  // The ID to be assigned to the next layer created by `CreateLayer()`.
  LayerId next_layer_id_ = LayerId(1);

  // Cached `CheckConfig()` results.  If there is no entry in the map, then it means that the
  // config has not been seen before, and a FIDL `CheckConfig()` call is necessary.  Otherwise
  // the stored boolean indicates whether a FIDL `CheckConfig()` call would succeed or fail
  // (true indicates success, and false indicates failure).
  internal::CheckConfigCache check_config_cache_;

  // Specifies which heuristics should be enabled in `HeuristicsSayThatCheckConfigWouldFail()`.
  const CheckConfigHeuristics check_config_heuristics_;

  // Track the number of state-setting methods called on `CoordinatorProxy`, and the number of
  // corresponding FIDL methods sent to the `fuchsia.hardware.display/Coordinator` service.
  // Calls to `ApplyConfig()` are tracked separately.
  uint64_t api_calls_received_ = 0;
  uint64_t api_calls_sent_ = 0;
  uint64_t apply_config_calls_received_ = 0;
  uint64_t apply_config_calls_sent_ = 0;
  // Track the number of `CheckConfig()` calls made to the FIDL service, and the number of calls
  // that are skipped because a cached result was found.
  uint64_t check_config_calls_skipped_ = 0;
  uint64_t check_config_calls_sent_ = 0;
  // Track the number of images and events that were imported (the total number, not current count).
  uint64_t import_image_calls_sent_ = 0;
  uint64_t import_event_calls_sent_ = 0;

  // Inspect properties reflect the counts above.
  inspect::Node inspect_node_;
  inspect::UintProperty inspect_api_calls_received_;
  inspect::UintProperty inspect_api_calls_sent_;
  inspect::UintProperty inspect_apply_config_calls_received_;
  inspect::UintProperty inspect_apply_config_calls_sent_;
  inspect::UintProperty inspect_check_config_calls_skipped_;
  inspect::UintProperty inspect_check_config_calls_sent_;
  inspect::UintProperty inspect_check_config_cache_size_;
  inspect::UintProperty inspect_import_image_count_;
  inspect::UintProperty inspect_current_image_count_;
  inspect::UintProperty inspect_import_event_count_;
  inspect::UintProperty inspect_current_event_count_;
#ifndef NDEBUG
  inspect::LazyNode inspect_config_cache_dump_;
#endif
};

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_COORDINATOR_PROXY_H_
