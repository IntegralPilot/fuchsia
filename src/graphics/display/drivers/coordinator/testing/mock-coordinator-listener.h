// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_COORDINATOR_LISTENER_H_
#define SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_COORDINATOR_LISTENER_H_

#include <fidl/fuchsia.hardware.display/cpp/wire.h>
#include <lib/fit/function.h>
#include <lib/zx/time.h>

#include <span>

#include "src/graphics/display/lib/api-types/cpp/config-stamp.h"
#include "src/graphics/display/lib/api-types/cpp/display-id.h"
#include "src/graphics/display/lib/api-types/cpp/vsync-ack-cookie.h"

namespace display_coordinator {

class MockCoordinatorListener
    : public fidl::WireServer<fuchsia_hardware_display::CoordinatorListener> {
 public:
  using OnDisplaysChangedCallback =
      fit::function<void(std::span<const fuchsia_hardware_display::wire::Info> added_displays,
                         std::span<const display::DisplayId> removed_display_ids)>;
  using OnClientOwnershipChangeCallback = fit::function<void(bool has_ownership)>;
  using OnVsyncCallback = fit::function<void(
      display::DisplayId display_id, zx::time_monotonic timestamp,
      display::ConfigStamp displayed_config_stamp, display::VsyncAckCookie cookie)>;

  MockCoordinatorListener(OnDisplaysChangedCallback on_displays_changed_callback,
                          OnVsyncCallback on_vsync_callback,
                          OnClientOwnershipChangeCallback on_client_ownership_change_callback);

  ~MockCoordinatorListener() override;

  MockCoordinatorListener(const MockCoordinatorListener&) = delete;
  MockCoordinatorListener(MockCoordinatorListener&&) = delete;
  MockCoordinatorListener& operator=(const MockCoordinatorListener&) = delete;
  MockCoordinatorListener& operator=(MockCoordinatorListener&&) = delete;

  // [`fuchsia.hardware.display/CoordinatorListener`]:
  void OnDisplaysChanged(OnDisplaysChangedRequestView request,
                         OnDisplaysChangedCompleter::Sync& completer) override;
  void OnVsync(OnVsyncRequestView request, OnVsyncCompleter::Sync& completer) override;
  void OnClientOwnershipChange(OnClientOwnershipChangeRequestView request,
                               OnClientOwnershipChangeCompleter::Sync& completer) override;
  void handle_unknown_method(
      fidl::UnknownMethodMetadata<fuchsia_hardware_display::CoordinatorListener> metadata,
      fidl::UnknownMethodCompleter::Sync& completer) override;

 private:
  const OnDisplaysChangedCallback on_displays_changed_callback_;
  const OnVsyncCallback on_vsync_callback_;
  const OnClientOwnershipChangeCallback on_client_ownership_change_callback_;
};

}  // namespace display_coordinator

#endif  // SRC_GRAPHICS_DISPLAY_DRIVERS_COORDINATOR_TESTING_MOCK_COORDINATOR_LISTENER_H_
