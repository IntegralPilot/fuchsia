// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.virtualization.guest.interaction/cpp/fidl.h>
#include <fidl/fuchsia.virtualization/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include "src/virtualization/lib/guest_interaction/interactive_guest/interactive_guest_impl.h"

using InteractiveGuest = fuchsia_virtualization_guest_interaction::InteractiveGuest;

int main() {
  FX_LOGS(INFO) << "Bootstrapping the InteractiveGuest component.";
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);

  // Spin up and connect to the GuestManager
  zx::result result = outgoing.ServeFromStartupInfo();
  FX_CHECK(result.is_ok()) << std::format("Failed to serve outgoing directory with status: {}",
                                          result.status_string());

  // Create and serve the InteractiveGuest
  bool already_bound = false;
  interactive_guest::InteractiveGuestImpl interactive_guest_impl(loop);
  const auto incoming_request_handler = [dispatcher, &interactive_guest_impl, &already_bound](
                                            fidl::ServerEnd<InteractiveGuest> server_end) {
    FX_LOGS(INFO) << "Incoming connection for " << fidl::DiscoverableProtocolName<InteractiveGuest>;

    if (already_bound) {
      FX_LOGS(ERROR)
          << "The interactive guest cannot gracefully handle multiple requests, and is already bound. Closing the incoming request.";
      server_end.Close(ZX_ERR_ALREADY_BOUND);
      return;
    }

    fidl::BindServer(dispatcher, std::move(server_end), &interactive_guest_impl);
    already_bound = true;
  };
  result = outgoing.AddUnmanagedProtocol<InteractiveGuest>(incoming_request_handler);
  FX_CHECK(result.is_ok()) << std::format(
      "Failed to register InteractiveGuest protocol with status: {}", result.status_string());

  FX_LOGS(INFO) << "Running the InteractiveGuest component.";
  return loop.Run();
}
