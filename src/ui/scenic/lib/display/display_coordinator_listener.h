// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_DISPLAY_DISPLAY_COORDINATOR_LISTENER_H_
#define SRC_UI_SCENIC_LIB_DISPLAY_DISPLAY_COORDINATOR_LISTENER_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <lib/async/cpp/wait.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/event.h>

#include "src/ui/scenic/lib/display/fidl_typedefs.h"

namespace display {

// Implements a [`fuchsia.hardware.display/CoordinatorListener`] server and
// allows registration for display events callbacks.
class DisplayCoordinatorListener final
    : public fidl::WireServer<fuchsia_hardware_display::CoordinatorListener> {
 public:
  using OnDisplaysChangedCallback = std::function<void(
      fidl::VectorView<WireDisplayInfo> added, fidl::VectorView<display::WireDisplayId> removed)>;
  using OnClientOwnershipChangeCallback = std::function<void(bool has_ownership)>;
  using OnVsyncCallback =
      fit::function<void(display::WireDisplayId display_id, zx::time_monotonic timestamp,
                         WireConfigStamp displayed_config_stamp, WireVsyncAckCookie cookie)>;

  // `coordinator_listener_server` must be valid.
  explicit DisplayCoordinatorListener(
      fidl::ServerEnd<fuchsia_hardware_display::CoordinatorListener> coordinator_listener_server,
      OnDisplaysChangedCallback on_displays_changed, OnVsyncCallback on_vsync,
      OnClientOwnershipChangeCallback on_client_ownership_change);

  ~DisplayCoordinatorListener() override;

  DisplayCoordinatorListener(const DisplayCoordinatorListener&) = delete;
  DisplayCoordinatorListener(DisplayCoordinatorListener&&) = delete;
  DisplayCoordinatorListener& operator=(const DisplayCoordinatorListener&) = delete;
  DisplayCoordinatorListener& operator=(DisplayCoordinatorListener&&) = delete;

  // fidl::Server<fuchsia_hardware_display::CoordinatorListener>:
  void OnDisplaysChanged(OnDisplaysChangedRequestView request,
                         OnDisplaysChangedCompleter::Sync& completer) override;
  void OnVsync(OnVsyncRequestView request, OnVsyncCompleter::Sync& completer) override;
  void OnClientOwnershipChange(OnClientOwnershipChangeRequestView request,
                               OnClientOwnershipChangeCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_display::CoordinatorListener> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  const OnDisplaysChangedCallback on_displays_changed_;
  const OnVsyncCallback on_vsync_;
  const OnClientOwnershipChangeCallback on_client_ownership_change_;

  fidl::ServerBindingRef<fuchsia_hardware_display::CoordinatorListener> binding_;
};

}  // namespace display

#endif  // SRC_UI_SCENIC_LIB_DISPLAY_DISPLAY_COORDINATOR_LISTENER_H_
