// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/singleton_display_service.h"

#include <fidl/fuchsia.math/cpp/fidl.h>
#include <lib/async/default.h>

namespace display {

SingletonDisplayService::SingletonDisplayService(std::shared_ptr<display::Display> display)
    : display_(std::move(display)) {}

void SingletonDisplayService::GetMetrics(GetMetricsCompleter::Sync& completer) {
  GetMetrics([completer = completer.ToAsync()](auto result) mutable {
    completer.Reply(std::move(result));
  });
}

void SingletonDisplayService::GetMetrics(
    fit::function<void(fuchsia_ui_display_singleton::InfoGetMetricsResponse)> callback) {
  const glm::vec2 dpr = display_->device_pixel_ratio();
  if (dpr.x != dpr.y) {
    FX_LOGS(WARNING) << "SingletonDisplayService::GetMetrics(): x/y display pixel ratio mismatch ("
                     << dpr.x << " vs. " << dpr.y << ")";
  }

  auto metrics = fuchsia_ui_display_singleton::Metrics();
  metrics.extent_in_px(fuchsia_math::SizeU{display_->width_in_px(), display_->height_in_px()});
  metrics.extent_in_mm(fuchsia_math::SizeU{display_->width_in_mm(), display_->height_in_mm()});
  metrics.recommended_device_pixel_ratio(fuchsia_math::VecF{dpr.x, dpr.y});
  metrics.maximum_refresh_rate_in_millihertz(display_->maximum_refresh_rate_in_millihertz());

  callback(std::move(metrics));
}

void SingletonDisplayService::GetEvent(GetEventCompleter::Sync& completer) {
  GetEvent([completer = completer.ToAsync()](auto result) mutable {
    completer.Reply(std::move(result));
  });
}

void SingletonDisplayService::GetEvent(
    fit::function<void(fuchsia_ui_composition_internal::DisplayOwnershipGetEventResponse)>
        callback) {
  // These constants are defined as raw hex in the FIDL file, so we confirm here that they are the
  // same values as the expected constants in the ZX headers.
  static_assert(fuchsia_ui_composition_internal::kSignalDisplayNotOwned == ZX_USER_SIGNAL_0,
                "Bad constant");
  static_assert(fuchsia_ui_composition_internal::kSignalDisplayOwned == ZX_USER_SIGNAL_1,
                "Bad constant");

  zx::event dup;
  if (display_->ownership_event().duplicate(ZX_RIGHTS_BASIC, &dup) != ZX_OK) {
    FX_LOGS(ERROR) << "Display ownership event duplication error.";
    callback(zx::event());
  } else {
    callback(std::move(dup));
  }
}

void SingletonDisplayService::AddPublicService(sys::OutgoingDirectory* outgoing_directory) {
  FX_DCHECK(outgoing_directory);
  outgoing_directory->AddProtocol<fuchsia_ui_display_singleton::Info>(info_bindings_.CreateHandler(
      this, async_get_default_dispatcher(), fidl::kIgnoreBindingClosure));
  outgoing_directory->AddProtocol<fuchsia_ui_composition_internal::DisplayOwnership>(
      ownership_bindings_.CreateHandler(this, async_get_default_dispatcher(),
                                        fidl::kIgnoreBindingClosure));
}

}  // namespace display
