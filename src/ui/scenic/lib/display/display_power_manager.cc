// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/display_power_manager.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <lib/zx/clock.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include "src/ui/scenic/lib/display/display_manager.h"

namespace display {

namespace {

using PowerMode = fuchsia_ui_display_singleton::PowerMode;

constexpr char kDisplayPowerEvents[] = "display_power_events";
constexpr uint64_t kInspectHistorySize = 64;

std::string ToString(const PowerMode& power_mode) {
  switch (power_mode) {
    case PowerMode::kOff:
      return "off";
    case PowerMode::kOn:
      return "on";
    case PowerMode::kDoze:
      return "doze";
    case PowerMode::kDozeSuspend:
      return "doze_suspend";
    default:
      return "unknown";
  }
}

fuchsia_hardware_display_types::PowerMode ToDisplayPowerMode(const PowerMode& power_mode) {
  switch (power_mode) {
    case PowerMode::kOff:
      return fuchsia_hardware_display_types::PowerMode::kOff;
    case PowerMode::kOn:
      return fuchsia_hardware_display_types::PowerMode::kOn;
    case PowerMode::kDoze:
      return fuchsia_hardware_display_types::PowerMode::kDoze;
    case PowerMode::kDozeSuspend:
      return fuchsia_hardware_display_types::PowerMode::kDozeSuspend;
    default:
      FX_LOGS(ERROR) << "Unexpected power mode: " << ToString(power_mode) << "; defaulting to ON";
      return fuchsia_hardware_display_types::PowerMode::kOn;
  }
}

}  // namespace

DisplayPowerManager::DisplayPowerManager(DisplayManager& display_manager,
                                         inspect::Node& parent_node)
    : display_manager_(display_manager),
      inspect_display_power_events_(parent_node.CreateChild(kDisplayPowerEvents),
                                    kInspectHistorySize) {}

void DisplayPowerManager::SetPowerMode(SetPowerModeRequest& request,
                                       SetPowerModeCompleter::Sync& completer) {
  SetPowerMode(request.power_mode(),
               [completer = completer.ToAsync()](auto result) mutable { completer.Reply(result); });
}

void DisplayPowerManager::SetPowerMode(PowerMode power_mode,
                                       fit::function<void(fit::result<zx_status_t>)> completer) {
  // No display
  if (!display_manager_.default_display()) {
    completer(fit::error(ZX_ERR_NOT_FOUND));
    return;
  }

  // TODO(https://fxbug.dev/42177175): Since currently Scenic only supports one display,
  // the DisplayPowerManager will only control power of the default display.
  // Once Scenic and DisplayManager supports multiple displays, this needs to
  // be updated to control power of all available displays.
  auto& coordinator_proxy = display_manager_.coordinator_proxy();
  FX_DCHECK(coordinator_proxy);
  display::DisplayId id = display_manager_.default_display()->display_id();

  auto set_display_power_mode_result = coordinator_proxy->raw().sync()->SetDisplayPowerMode(
      id.ToFidl(), ToDisplayPowerMode(power_mode));
  if (!set_display_power_mode_result.ok()) {
    FX_LOGS(ERROR) << "DisplayPowerManager.SetPowerMode() FAILED to set value: "
                   << ToString(power_mode)
                   << " transport error: " << set_display_power_mode_result.status_string();
    // NOTE: is ZX_ERR_INTERNAL the best value?
    AddSetPowerModeInspectValues(power_mode, ZX_ERR_INTERNAL);
    completer(fit::error(ZX_ERR_INTERNAL));
    return;
  }

  if (set_display_power_mode_result->is_error()) {
    FX_LOGS(ERROR) << "DisplayPowerManager.SetPowerMode() FAILED to set value: "
                   << ToString(power_mode) << " display error: "
                   << zx_status_get_string(set_display_power_mode_result->error_value());
    // NOTE: is ZX_ERR_NOT_SUPPORTED the best value?
    AddSetPowerModeInspectValues(power_mode, ZX_ERR_NOT_SUPPORTED);
    completer(fit::error(ZX_ERR_NOT_SUPPORTED));
    return;
  }

  FX_LOGS(INFO) << "Successfully set display power mode: " << ToString(power_mode);
  current_power_mode_ = power_mode;
  last_power_change_time_ = zx::clock::get_monotonic();

  AddSetPowerModeInspectValues(power_mode, ZX_OK);
  completer(fit::ok());
}

void DisplayPowerManager::AddSetPowerModeInspectValues(PowerMode power_mode, zx_status_t status) {
  const auto boot_now = zx::clock::get_boot();
  const auto mono_now = zx::clock::get_monotonic();

  inspect_display_power_events_.CreateEntry(
      [power_mode, status, boot_now, mono_now](inspect::Node& n) {
        std::string power_mode_str = ToString(power_mode);
        if (status != ZX_OK) {
          power_mode_str = power_mode_str + "_ERROR_" + zx_status_get_string(status);
        }

        // TODO(b/475953032): Remove unsuffixed `power_mode_str` when it is no longer used directly.
        n.RecordInt(power_mode_str, mono_now.get());
        // This is used in system health analyses. Without it, the fallback is `mono_now` above,
        // which is wrong.
        n.RecordInt(std::format("{}_boot_ns", power_mode_str), boot_now.get());
        n.RecordInt(std::format("{}_mono_ns", power_mode_str), mono_now.get());

        // Detect potential inaccuracy in the monotonic clock reading.  This instructs the
        // consumer to take this timestamp with a grain of salt.
        const zx::duration boot_diff = zx::clock::get_boot() - boot_now;
        constexpr auto kBootDiffThreshold = zx::usec(100);
        if (boot_diff >= kBootDiffThreshold) {
          n.RecordInt("timestamp_inaccuracy_range_us", boot_diff.to_usecs());
        }
      });
}

}  // namespace display
