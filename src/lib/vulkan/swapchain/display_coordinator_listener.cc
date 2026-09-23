// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/vulkan/swapchain/display_coordinator_listener.h"

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>

#include <cinttypes>
#include <cstdio>

namespace image_pipe_swapchain {

DisplayCoordinatorListener::DisplayCoordinatorListener(
    fidl::ServerEnd<fuchsia_hardware_display::CoordinatorListener> server_end,
    OnDisplaysChangedCallback on_displays_changed, OnVsyncCallback on_vsync,
    OnClientOwnershipChangeCallback on_client_ownership_change, async_dispatcher_t& dispatcher)
    : on_displays_changed_(std::move(on_displays_changed)),
      on_vsync_(std::move(on_vsync)),
      on_client_ownership_change_(std::move(on_client_ownership_change)),
      binding_(fidl::BindServer(&dispatcher, std::move(server_end), this)) {}

DisplayCoordinatorListener::~DisplayCoordinatorListener() = default;

void DisplayCoordinatorListener::OnDisplaysChanged(OnDisplaysChangedRequest& request,
                                                   OnDisplaysChangedCompleter::Sync& completer) {
  if (on_displays_changed_) {
    on_displays_changed_(std::move(request.added()), std::move(request.removed()));
  }
}

void DisplayCoordinatorListener::OnVsync(OnVsyncRequest& request,
                                         OnVsyncCompleter::Sync& completer) {
  if (on_vsync_) {
    on_vsync_(request.display_id(), zx::time(request.timestamp()), request.displayed_config_stamp(),
              request.cookie());
  }
}

void DisplayCoordinatorListener::OnClientOwnershipChange(
    OnClientOwnershipChangeRequest& request, OnClientOwnershipChangeCompleter::Sync& completer) {
  if (on_client_ownership_change_) {
    on_client_ownership_change_(request.has_ownership());
  }
}

void DisplayCoordinatorListener::handle_unknown_method(
    fidl::UnknownMethodMetadata<fuchsia_hardware_display::CoordinatorListener> metadata,
    fidl::UnknownMethodCompleter::Sync& completer) {
  fprintf(stderr, "Unknown event received: %" PRIu64, metadata.method_ordinal);
}

}  // namespace image_pipe_swapchain
