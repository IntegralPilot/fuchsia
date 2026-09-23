// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_DISPLAY_MANAGER_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_DISPLAY_MANAGER_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/inspect.h>

#include <optional>

#include "src/lib/fxl/macros.h"
#include "src/ui/scenic/lib/display/coordinator_proxy.h"
#include "src/ui/scenic/lib/display/display.h"
#include "src/ui/scenic/lib/display/display_coordinator_listener.h"
#include "src/ui/scenic/lib/display/fidl_typedefs.h"
#include "src/ui/scenic/lib/utils/range_inclusive.h"

namespace display {

struct DisplayModeConstraints {
  utils::RangeInclusive<int> width_px_range;
  utils::RangeInclusive<int> height_px_range;
  utils::RangeInclusive<int> refresh_rate_millihertz_range;

  bool ModeSatisfiesConstraints(const WireDisplayMode& mode) const;
};

// Discovers and owns the default display coordinator, and waits for and exposes the default
// display.
class DisplayManager {
 public:
  // |display_available_cb| is a one-shot callback that is triggered when the first display is
  // observed, and cleared immediately afterward.
  explicit DisplayManager(fit::closure display_available_cb);
  DisplayManager(std::optional<WireDisplayId> i_can_haz_display_id,
                 std::optional<size_t> display_mode_index_override,
                 DisplayModeConstraints display_mode_constraints, inspect::Node inspect_node,
                 fit::closure display_available_cb,
                 CoordinatorProxy::CheckConfigHeuristics check_config_heuristics = {
                     .enable_heuristics = false});
  ~DisplayManager() = default;

  void BindDefaultDisplayCoordinator(
      async_dispatcher_t* dispatcher,
      fidl::ClientEnd<fuchsia_hardware_display::Coordinator> coordinator,
      fidl::ServerEnd<fuchsia_hardware_display::CoordinatorListener> coordinator_listener);

  void SetDisplayAddedCallback(fit::function<void(display::Display&)> display_added_cb);

  // Gets information about the default display.
  // May return null if there isn't one.
  Display* default_display() const { return default_display_.get(); }

  // Only use this during Scenic initialization to pass a reference to FrameScheduler.
  std::shared_ptr<Display> default_display_shared() const { return default_display_; }

  const std::shared_ptr<CoordinatorProxy>& coordinator_proxy() const { return coordinator_proxy_; }

  std::shared_ptr<display::DisplayCoordinatorListener> display_coordinator_listener() {
    return display_coordinator_listener_;
  }

  // For testing.
  void SetDefaultDisplayForTests(std::shared_ptr<Display> display) {
    default_display_ = std::move(display);
  }

 private:
  void OnDisplaysChanged(fidl::VectorView<WireDisplayInfo> added,
                         fidl::VectorView<WireDisplayId> removed);
  void OnClientOwnershipChange(bool has_ownership);
  void OnVsync(WireDisplayId display_id, zx::time_monotonic timestamp,
               WireConfigStamp displayed_config_stamp, WireVsyncAckCookie cookie);

  std::shared_ptr<CoordinatorProxy> coordinator_proxy_;

  const CoordinatorProxy::CheckConfigHeuristics check_config_heuristics_;

  std::shared_ptr<display::DisplayCoordinatorListener> display_coordinator_listener_;

  std::shared_ptr<Display> default_display_;

  // When new displays are detected, ignore all displays which don't match this ID.
  // TODO(https://fxbug.dev/42156949): Remove this when we have proper multi-display support.
  const std::optional<WireDisplayId> i_can_haz_display_id_;

  // When a new display is picked, use display mode with this index.
  // TODO(https://fxbug.dev/42156949): Remove this when we have proper multi-display support.
  const std::optional<size_t> display_mode_index_override_;

  const DisplayModeConstraints display_mode_constraints_;

  fit::closure display_available_cb_;
  // A boolean indicating whether or not we have ownership of the display
  // coordinator (not just individual displays). The default is no.
  bool owns_display_coordinator_ = false;

  fit::function<void(display::Display&)> display_added_cb_;

  zx::time_monotonic last_vsync_timestamp_;

  inspect::Node inspect_node_;
  inspect::LazyNode inspect_lazy_metrics_;

  FXL_DISALLOW_COPY_AND_ASSIGN(DisplayManager);
};

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_DISPLAY_MANAGER_H_
