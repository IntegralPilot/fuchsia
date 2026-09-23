// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_VULKAN_SWAPCHAIN_DISPLAY_COORDINATOR_LISTENER_H_
#define SRC_LIB_VULKAN_SWAPCHAIN_DISPLAY_COORDINATOR_LISTENER_H_

#include <fidl/fuchsia.hardware.display.types/cpp/fidl.h>
#include <fidl/fuchsia.hardware.display/cpp/fidl.h>

#include <functional>

namespace image_pipe_swapchain {

class DisplayCoordinatorListener final
    : public fidl::Server<fuchsia_hardware_display::CoordinatorListener> {
 public:
  using OnDisplaysChangedCallback =
      std::function<void(std::vector<fuchsia_hardware_display::Info> added,
                         std::vector<fuchsia_hardware_display_types::DisplayId> removed)>;
  using OnClientOwnershipChangeCallback = std::function<void(bool has_ownership)>;
  using OnVsyncCallback =
      std::function<void(fuchsia_hardware_display_types::DisplayId display_id, zx::time timestamp,
                         fuchsia_hardware_display::ConfigStamp displayed_config_stamp,
                         fuchsia_hardware_display::VsyncAckCookie cookie)>;

  // `coordinator_listener_server` must be valid.
  DisplayCoordinatorListener(
      fidl::ServerEnd<fuchsia_hardware_display::CoordinatorListener> server_end,
      OnDisplaysChangedCallback on_displays_changed, OnVsyncCallback on_vsync,
      OnClientOwnershipChangeCallback on_client_ownership_change, async_dispatcher_t& dispatcher);

  ~DisplayCoordinatorListener() override;

  DisplayCoordinatorListener(const DisplayCoordinatorListener&) = delete;
  DisplayCoordinatorListener(DisplayCoordinatorListener&&) = delete;
  DisplayCoordinatorListener& operator=(const DisplayCoordinatorListener&) = delete;
  DisplayCoordinatorListener& operator=(DisplayCoordinatorListener&&) = delete;

  // fidl::Server<fuchsia_hardware_display::CoordinatorListener>:
  void OnDisplaysChanged(OnDisplaysChangedRequest& request,
                         OnDisplaysChangedCompleter::Sync& completer) override;
  void OnVsync(OnVsyncRequest& request, OnVsyncCompleter::Sync& completer) override;
  void OnClientOwnershipChange(OnClientOwnershipChangeRequest& request,
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

}  // namespace image_pipe_swapchain

#endif  // SRC_LIB_VULKAN_SWAPCHAIN_DISPLAY_COORDINATOR_LISTENER_H_
