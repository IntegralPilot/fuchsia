// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.debugger/cpp/fidl.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <zircon/processargs.h>

#include <memory>

#include "lib/stdcompat/functional.h"
#include "src/developer/debug/debug_agent/debug_agent.h"
#include "src/developer/debug/debug_agent/debug_agent_config.h"
#include "src/developer/debug/debug_agent/debug_agent_server.h"
#include "src/developer/debug/debug_agent/zircon_system_interface.h"
#include "src/developer/debug/shared/platform_message_loop.h"

int main(int argc, const char* argv[]) {
  debug::PlatformMessageLoop message_loop;
  std::string init_error_message;
  if (!message_loop.Init(&init_error_message)) {
    LOGS(Error) << init_error_message;
    return 1;
  }

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(message_loop.dispatcher());

  // The scope ensures the objects are destroyed before calling Cleanup on the MessageLoop.
  {
    // Take the server_end of the DebugAgent protocol we are handed from the launcher.
    zx::channel server_end(zx_take_startup_handle(PA_HND(PA_USER0, 0)));
    FX_CHECK(server_end.is_valid());

    auto config = debug_agent_config::Config::TakeFromStartupHandle();
    auto zircon_system_interface = std::make_unique<debug_agent::ZirconSystemInterface>();
    debug_agent::DebugAgent debug_agent(
        std::move(zircon_system_interface),
        debug_agent::DebugAgentOptions{.monitor_root_job = config.monitor_root_job()});

    auto res = outgoing.AddUnmanagedProtocol<fuchsia_debugger::DebugAgent>(
        [&](fidl::ServerEnd<fuchsia_debugger::DebugAgent> server_end) {
          debug_agent::DebugAgentServer::BindServer(
              message_loop.dispatcher(), std::move(server_end), debug_agent.GetWeakPtr());
        });
    FX_CHECK(res.is_ok()) << res.error_value();

    res = outgoing.ServeFromStartupInfo();
    FX_CHECK(res.is_ok()) << res.error_value();

    // Now explicitly bind to the given server_end from the startup handles.
    debug_agent::DebugAgentServer::BindServer(
        message_loop.dispatcher(),
        fidl::ServerEnd<fuchsia_debugger::DebugAgent>(std::move(server_end)),
        debug_agent.GetWeakPtr());

    // Run the loop.
    message_loop.Run();
  }

  message_loop.Cleanup();

  // It's very useful to have a simple message that informs the debug_agent exited successfully.
  LOGS(Info) << "See you, Space Cowboy...";
  return 0;
}
