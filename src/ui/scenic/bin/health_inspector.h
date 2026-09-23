// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_BIN_HEALTH_INSPECTOR_H_
#define SRC_UI_SCENIC_BIN_HEALTH_INSPECTOR_H_

#include <lib/inspect/cpp/inspect.h>

#include <optional>

#include "src/ui/scenic/lib/display/display_manager.h"
#include "src/ui/scenic/lib/display/display_power_manager.h"

namespace scenic_impl {

// Handles the creation of the top-level "health" Inspect node and evaluates
// health checks.  Under this node is an "overall_health" string that is either
// "UNHEALTHY" or "NOT_UNHEALTHY" (note: not "HEALTHY" because the checks are currently
// far from comprehensive).  If the overall status is "UNHEALTHY", there will be
// additional information about specific failures.
//
// Lifetime requirements: The `DisplayManager` and `DisplayPowerManager`
// references passed to the constructor must outlive this instance. This is
// guaranteed by instantiating it as the last member in `App`.
class HealthInspector {
 public:
  HealthInspector(const std::optional<display::DisplayManager>& display_manager,
                  const std::optional<display::DisplayPowerManager>& display_power_manager,
                  inspect::Node& parent_node);

 private:
  void InitializeInspectObjects(inspect::Node& parent_node);

  // Scenic expects to be receiving vsyncs when the display is powered on.  Otherwise there is
  // probably a bug in the display driver.
  bool CheckReceivingVsyncsWhenDisplayIsOn(inspect::Node& node, zx::time_monotonic now,
                                           inspect::Inspector& inspector);

  const std::optional<display::DisplayManager>& display_manager_;
  const std::optional<display::DisplayPowerManager>& display_power_manager_;

  inspect::Node health_node_;
  inspect::LazyNode inspect_lazy_node_;
};

}  // namespace scenic_impl

#endif  // SRC_UI_SCENIC_BIN_HEALTH_INSPECTOR_H_
