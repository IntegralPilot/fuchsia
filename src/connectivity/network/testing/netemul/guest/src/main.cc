// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.netemul.guest/cpp/fidl.h>
#include <fidl/fuchsia.virtualization/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/directory.h>
#include <lib/syslog/cpp/macros.h>

#include "src/connectivity/network/testing/netemul/guest/src/controller.h"

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);

  zx::result result = outgoing.ServeFromStartupInfo();
  ZX_ASSERT_MSG(result.is_ok(), "failed to serve outgoing directory with status: %s",
                result.status_string());

  zx::result guest_manager_client_end =
      component::Connect<fuchsia_virtualization::DebianGuestManager>();
  ZX_ASSERT_MSG(guest_manager_client_end.is_ok(),
                "failed to connect to the GuestManager protocol with status: %s",
                guest_manager_client_end.status_string());

  fidl::SyncClient<fuchsia_virtualization::DebianGuestManager> guest_manager =
      fidl::SyncClient<fuchsia_virtualization::DebianGuestManager>(
          {std::move(*guest_manager_client_end)});

  ControllerImpl controller(loop, std::move(guest_manager));

  result = outgoing.AddUnmanagedProtocol<fuchsia_netemul_guest::Controller>(
      [dispatcher, &controller](fidl::ServerEnd<fuchsia_netemul_guest::Controller> server_end) {
        FX_LOGS(INFO) << "Incoming connection for "
                      << fidl::DiscoverableProtocolName<fuchsia_netemul_guest::Controller>;
        fidl::BindServer(dispatcher, std::move(server_end), &controller);
      });
  ZX_ASSERT_MSG(result.is_ok(),
                "failed to add the fuchsia_netemul_guest::Controller protocol with status: %s",
                result.status_string());

  result = outgoing.AddUnmanagedProtocol<fuchsia_net_virtualization::Control>(
      [dispatcher, &controller](fidl::ServerEnd<fuchsia_net_virtualization::Control> server_end) {
        FX_LOGS(INFO) << "Incoming connection for "
                      << fidl::DiscoverableProtocolName<fuchsia_net_virtualization::Control>;
        fidl::BindServer(dispatcher, std::move(server_end), &controller);
      });
  ZX_ASSERT_MSG(result.is_ok(),
                "failed to add the fuchsia_net_virtualization::Control protocol with status: %s",
                result.status_string());

  FX_LOGS(INFO) << "Running Netemul Guest server";
  return loop.Run();
}
