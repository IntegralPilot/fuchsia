// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/display/display_coordinator_listener.h"

#include <lib/async/default.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/types.h>

namespace display {

DisplayCoordinatorListener::DisplayCoordinatorListener(
    fidl::ServerEnd<fuchsia_hardware_display::CoordinatorListener> coordinator_listener_server,
    OnDisplaysChangedCallback on_displays_changed, OnVsyncCallback on_vsync,
    OnClientOwnershipChangeCallback on_client_ownership_change)
    : on_displays_changed_(std::move(on_displays_changed)),
      on_vsync_(std::move(on_vsync)),
      on_client_ownership_change_(std::move(on_client_ownership_change)),
      binding_(fidl::BindServer(async_get_default_dispatcher(),
                                std::move(coordinator_listener_server), this)) {}

DisplayCoordinatorListener::~DisplayCoordinatorListener() {}

void DisplayCoordinatorListener::OnDisplaysChanged(OnDisplaysChangedRequestView request,
                                                   OnDisplaysChangedCompleter::Sync& completer) {
  if (on_displays_changed_) {
    on_displays_changed_(request->added, request->removed);
  }
}

void DisplayCoordinatorListener::OnVsync(OnVsyncRequestView request,
                                         OnVsyncCompleter::Sync& completer) {
  if (on_vsync_) {
    on_vsync_(request->display_id, zx::time_monotonic(request->timestamp),
              request->displayed_config_stamp, request->cookie);
  }
}

void DisplayCoordinatorListener::OnClientOwnershipChange(
    OnClientOwnershipChangeRequestView request, OnClientOwnershipChangeCompleter::Sync& completer) {
  if (on_client_ownership_change_) {
    on_client_ownership_change_(request->has_ownership);
  }
}

void DisplayCoordinatorListener::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_display::CoordinatorListener> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  FX_LOGS(WARNING) << "Unknown event received: # " << metadata.method_ordinal;
}

}  // namespace display
