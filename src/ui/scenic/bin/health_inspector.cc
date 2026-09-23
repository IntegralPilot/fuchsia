// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/bin/health_inspector.h"

#include <lib/fpromise/promise.h>

namespace scenic_impl {

HealthInspector::HealthInspector(
    const std::optional<display::DisplayManager>& display_manager,
    const std::optional<display::DisplayPowerManager>& display_power_manager,
    inspect::Node& parent_node)
    : display_manager_(display_manager), display_power_manager_(display_power_manager) {
  InitializeInspectObjects(parent_node);
}

void HealthInspector::InitializeInspectObjects(inspect::Node& parent_node) {
  // The user prefers "health" to be at the top, potentially as a peer to "subsystems".
  health_node_ = parent_node.CreateChild("health");
  inspect_lazy_node_ = health_node_.CreateLazyValues("metrics", [this] {
    inspect::Inspector inspector;
    auto& root = inspector.GetRoot();

    const zx::time_monotonic now(zx_clock_get_monotonic());
    const bool healthy = CheckReceivingVsyncsWhenDisplayIsOn(root, now, inspector);
    root.CreateString("overall_health", healthy ? "NOT_UNHEALTHY" : "UNHEALTHY", &inspector);

    return fpromise::make_ok_promise(std::move(inspector));
  });
}

bool HealthInspector::CheckReceivingVsyncsWhenDisplayIsOn(inspect::Node& node,
                                                          zx::time_monotonic now,
                                                          inspect::Inspector& inspector) {
  if (!display_power_manager_ || !display_manager_) {
    return true;  // Can't check, assume OK.
  }

  const auto power_mode = display_power_manager_->current_power_mode();
  if (power_mode != fuchsia_ui_display_singleton::PowerMode::kOn) {
    return true;  // Display is not on, so we don't expect Vsync events.
  }

  auto default_display = display_manager_->default_display();
  if (!default_display) {
    return true;  // No display, assume OK.
  }

  // Vsyncs are expected at 60fps, but we use a longer grace period to avoid false positives.
  constexpr zx::duration kGracePeriod = zx::sec(2);

  const zx::time_monotonic last_power_change = display_power_manager_->last_power_change_time();
  if (now - last_power_change < kGracePeriod) {
    return true;  // Still within grace period since display was powered on.
  }

  auto vsync_timing = default_display->vsync_timing();
  if (!vsync_timing) {
    return true;  // No vsync timing, assume OK.
  }
  const zx::time_monotonic last_vsync = vsync_timing->last_vsync_time();
  const zx::duration delta = now - last_vsync;

  const bool healthy = delta < kGracePeriod;
  if (!healthy) {
    // Record details in a specific error child node.
    inspect::Node error_node = node.CreateChild("ERROR_not_receiving_vsyncs_when_display_is_on");
    error_node.CreateInt("time_since_last_vsync_ms", delta.to_msecs(), &inspector);
    inspector.emplace(std::move(error_node));
  }

  return healthy;
}

}  // namespace scenic_impl
