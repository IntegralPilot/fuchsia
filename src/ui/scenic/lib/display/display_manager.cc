// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/display_manager.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.ui.composition.internal/cpp/fidl.h>
#include <lib/async/default.h>
#include <lib/fit/function.h>
#include <lib/syslog/cpp/macros.h>

#include "src/ui/scenic/lib/utils/logging.h"

namespace display {

namespace {

std::optional<size_t> PickFirstDisplayModeSatisfyingConstraints(
    std::span<const WireDisplayMode> modes, const DisplayModeConstraints& constraints) {
  for (size_t i = 0; i < modes.size(); ++i) {
    if (constraints.ModeSatisfiesConstraints(modes[i])) {
      return std::make_optional(i);
    }
  }
  return std::nullopt;
}

}  // namespace

bool DisplayModeConstraints::ModeSatisfiesConstraints(const WireDisplayMode& mode) const {
  if (!width_px_range.Contains(static_cast<int>(mode.active_area.width))) {
    return false;
  }
  if (!height_px_range.Contains(static_cast<int>(mode.active_area.height))) {
    return false;
  }
  if (!refresh_rate_millihertz_range.Contains(static_cast<int>(mode.refresh_rate_millihertz))) {
    return false;
  }
  return true;
}

DisplayManager::DisplayManager(fit::closure display_available_cb)
    : DisplayManager(std::nullopt, std::nullopt, /*display_mode_constraints=*/{}, inspect::Node(),
                     std::move(display_available_cb)) {}

DisplayManager::DisplayManager(std::optional<WireDisplayId> i_can_haz_display_id,
                               std::optional<size_t> display_mode_index_override,
                               DisplayModeConstraints display_mode_constraints,
                               inspect::Node inspect_node, fit::closure display_available_cb,
                               CoordinatorProxy::CheckConfigHeuristics check_config_heuristics)
    : check_config_heuristics_(check_config_heuristics),
      i_can_haz_display_id_(i_can_haz_display_id),
      display_mode_index_override_(display_mode_index_override),
      display_mode_constraints_(std::move(display_mode_constraints)),
      display_available_cb_(std::move(display_available_cb)),
      inspect_node_(std::move(inspect_node)) {}

void DisplayManager::BindDefaultDisplayCoordinator(
    async_dispatcher_t* dispatcher,
    fidl::ClientEnd<fuchsia_hardware_display::Coordinator> coordinator,
    fidl::ServerEnd<fuchsia_hardware_display::CoordinatorListener> coordinator_listener) {
  FX_DCHECK(!coordinator_proxy_);
  FX_DCHECK(coordinator.is_valid());

  coordinator_proxy_ = std::make_shared<CoordinatorProxy>(
      std::move(coordinator), dispatcher, check_config_heuristics_,
      inspect_node_.CreateChild("Display Coordinator Proxy"));

  display_coordinator_listener_ = std::make_shared<display::DisplayCoordinatorListener>(
      std::move(coordinator_listener), fit::bind_member<&DisplayManager::OnDisplaysChanged>(this),
      fit::bind_member<&DisplayManager::OnVsync>(this),
      fit::bind_member<&DisplayManager::OnClientOwnershipChange>(this));

  inspect_lazy_metrics_ = inspect_node_.CreateLazyValues("Vsync Metrics", [this, dispatcher]() {
    inspect::Inspector inspector;

    const zx::time_monotonic now(async_now(dispatcher));
    const zx::duration duration = now - last_vsync_timestamp_;
    inspector.GetRoot().RecordInt("time_since_last_vsync_ms", duration.to_msecs());

    return fpromise::make_ok_promise(std::move(inspector));
  });
}

void DisplayManager::OnDisplaysChanged(fidl::VectorView<WireDisplayInfo> added,
                                       fidl::VectorView<WireDisplayId> removed) {
  for (WireDisplayInfo& display_info : added) {
    // Ignore display if |i_can_haz_display_id| is set and it doesn't match ID.
    if (i_can_haz_display_id_.has_value() &&
        display_info.id.value != i_can_haz_display_id_->value) {
      FX_LOGS(INFO) << "Ignoring display with id=" << display_info.id.value
                    << " ... waiting for display with id=" << i_can_haz_display_id_->value;
      continue;
    }

    if (default_display_) {
      FX_LOGS(INFO) << "Default display already exists with id="
                    << default_display_->display_id().value()
                    << " ... skipping newly added display id=" << display_info.id.value;
    } else {
      size_t mode_index = 0;

      // Set display mode if requested.
      if (display_mode_index_override_.has_value()) {
        if (*display_mode_index_override_ < display_info.modes.size()) {
          mode_index = *display_mode_index_override_;
        } else {
          FX_LOGS(ERROR) << "Requested display mode=" << *display_mode_index_override_
                         << " doesn't exist for display with id=" << display_info.id.value;
        }
      } else {
        std::optional<size_t> mode_index_satisfying_constraints =
            PickFirstDisplayModeSatisfyingConstraints(display_info.modes,
                                                      display_mode_constraints_);

        // TODO(https://fxbug.dev/42097581): handle this more robustly.
        FX_CHECK(mode_index_satisfying_constraints.has_value())
            << "Failed to find a display mode satisfying all display constraints for "
               "display with id="
            << display_info.id.value;

        mode_index = *mode_index_satisfying_constraints;
      }

      if (mode_index != 0) {
        // TODO(https://fxbug.dev/402804098): `flatland::DisplayCompositor` now handles this, so
        // this is redundant, right?  Verify and delete.
        [[maybe_unused]] fidl::OneWayStatus set_display_mode_result =
            coordinator_proxy_->raw()->SetDisplayMode(display_info.id,
                                                      display_info.modes[mode_index]);
      }

      const WireDisplayMode& mode = display_info.modes[mode_index];
      std::vector<fuchsia_images2::PixelFormat> pixel_formats(display_info.pixel_format.begin(),
                                                              display_info.pixel_format.end());

      default_display_ = std::make_unique<Display>(
          display_info.id, mode, display_info.horizontal_size_mm, display_info.vertical_size_mm,
          display_info.max_layer_count, std::move(pixel_formats));
      OnClientOwnershipChange(owns_display_coordinator_);

      if (display_available_cb_) {
        display_available_cb_();
        display_available_cb_ = nullptr;
      }
      if (display_added_cb_) {
        display_added_cb_(*default_display_);
      }
    }
  }

  for (const WireDisplayId& id : removed) {
    if (default_display_ && default_display_->display_id() == display::DisplayId(id)) {
      // TODO(https://fxbug.dev/42097581): handle this more robustly.
      FX_CHECK(false) << "Display disconnected";
      return;
    }
  }
}

void DisplayManager::SetDisplayAddedCallback(
    fit::function<void(display::Display&)> display_added_cb) {
  display_added_cb_ = std::move(display_added_cb);
  if (default_display_) {
    display_added_cb_(*default_display_);
  }
}

void DisplayManager::OnClientOwnershipChange(bool has_ownership) {
  owns_display_coordinator_ = has_ownership;
  if (default_display_) {
    if (has_ownership) {
      default_display_->ownership_event().signal(
          fuchsia_ui_composition_internal::kSignalDisplayNotOwned,
          fuchsia_ui_composition_internal::kSignalDisplayOwned);
    } else {
      default_display_->ownership_event().signal(
          fuchsia_ui_composition_internal::kSignalDisplayOwned,
          fuchsia_ui_composition_internal::kSignalDisplayNotOwned);
    }
  }
}

void DisplayManager::OnVsync(WireDisplayId display_id, zx::time_monotonic timestamp,
                             WireConfigStamp displayed_config_stamp, WireVsyncAckCookie cookie) {
  if (cookie.value != fuchsia_hardware_display_types::kInvalidDispId) {
    FLATLAND_VERBOSE_LOG << "DisplayManager::OnVsync(): acknowledging vsync display_id="
                         << display_id.value << "  timestamp=" << timestamp.get()
                         << "  displayed_config_stamp=" << displayed_config_stamp.value
                         << "  cookie=" << cookie.value;
    [[maybe_unused]] fidl::OneWayStatus acknowledge_vsync_result =
        coordinator_proxy_->raw()->AcknowledgeVsync(cookie.value);
  } else {
    FLATLAND_VERBOSE_LOG << "DisplayManager::OnVsync(): received vsync display_id="
                         << display_id.value << "  timestamp=" << timestamp.get()
                         << "  displayed_config_stamp=" << displayed_config_stamp.value
                         << "  with no cookie.";
  }

  if (!default_display_) {
    FLATLAND_VERBOSE_LOG << "DisplayManager::OnVsync(): ignoring vsync, no default display";
    return;
  }
  if (default_display_->display_id() != display::DisplayId(display_id)) {
    FLATLAND_VERBOSE_LOG << "DisplayManager::OnVsync(): ignoring vsync, display_id="
                         << display_id.value << "  doesn't match default_display->display_id="
                         << default_display_->display_id().value();
    return;
  }
  default_display_->OnVsync(timestamp, displayed_config_stamp);

  last_vsync_timestamp_ = timestamp;
}

}  // namespace display
