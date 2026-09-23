// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/coordinator-getter/client.h"

#include <fidl/fuchsia.hardware.display/cpp/fidl.h>
#include <fidl/fuchsia.io/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/async/default.h>
#include <lib/fdio/directory.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace/event.h>
#include <lib/zx/result.h>
#include <zircon/status.h>

#include <memory>

namespace display {

fpromise::promise<CoordinatorClientChannels, zx_status_t> GetCoordinator(
    fidl::ClientEnd<fuchsia_hardware_display::Provider> provider, async_dispatcher_t* dispatcher) {
  auto [coordinator_client, coordinator_server] =
      fidl::Endpoints<fuchsia_hardware_display::Coordinator>::Create();
  auto [coordinator_listener_client, coordinator_listener_server] =
      fidl::Endpoints<fuchsia_hardware_display::CoordinatorListener>::Create();

  fpromise::bridge<void, zx_status_t> bridge;
  std::shared_ptr completer =
      std::make_shared<decltype(bridge.completer)>(std::move(bridge.completer));

  // fidl::Client requires that it must be bound on the dispatcher thread.
  // So this has to be dispatched as an async task running on `dispatcher`.
  async::PostTask(
      dispatcher, [completer, dispatcher, provider = std::move(provider),
                   coordinator_server = std::move(coordinator_server),
                   coordinator_listener_client = std::move(coordinator_listener_client)]() mutable {
        fidl::Client<fuchsia_hardware_display::Provider> client(std::move(provider), dispatcher);
        // The FIDL Client is retained in the `Then` handler, to keep the
        // connection open until the response is received.
        client
            ->OpenCoordinator(
                {{.coordinator = std::move(coordinator_server),
                  .coordinator_listener = std::move(coordinator_listener_client),
                  .priority = fuchsia_hardware_display::ClientPriority(
                      {.value = fuchsia_hardware_display::kCompositorClientPriorityValue})}})
            .Then([completer, client = std::move(client)](
                      fidl::Result<fuchsia_hardware_display::Provider::OpenCoordinator>& result) {
              if (result.is_error()) {
                auto& error_value = result.error_value();
                FX_LOGS(ERROR) << "Failed to open coordinator: " << error_value.FormatDescription();
                zx_status_t status = (error_value.is_domain_error())
                                         ? error_value.domain_error()
                                         : error_value.framework_error().status();
                completer->complete_error(status);
                return;
              }
              completer->complete_ok();
            });
      });

  CoordinatorClientChannels coordinator_channels = {
      .coordinator_client_end = std::move(coordinator_client),
      .coordinator_listener_server_end = std::move(coordinator_listener_server),
  };
  return bridge.consumer.promise().and_then(
      [coordinator_channels = std::move(coordinator_channels)]() mutable {
        return fpromise::ok(std::move(coordinator_channels));
      });
}

fpromise::promise<CoordinatorClientChannels, zx_status_t> GetCoordinator(
    fidl::ClientEnd<fuchsia_hardware_display::Provider> provider) {
  return GetCoordinator(std::move(provider), async_get_default_dispatcher());
}

}  // namespace display
