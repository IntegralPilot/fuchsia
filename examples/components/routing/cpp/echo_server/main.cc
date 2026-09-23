// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// [START imports]
#include <fidl/fidl.examples.routing.echo/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/fidl/cpp/binding.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
// [END imports]

// [START handler]
// Handler for incoming service requests
class EchoImplementation : public fidl::Server<fidl_examples_routing_echo::Echo> {
 public:
  void EchoString(EchoStringRequest& request, EchoStringCompleter::Sync& completer) override {
    completer.Reply({{.response = request.value()}});
  }
};
// [END handler]

// [START main_body]
int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  // Initialize inspect
  inspect::ComponentInspector inspector(loop.dispatcher(), inspect::PublishOptions{});
  inspector.Health().StartingUp();

  component::OutgoingDirectory outgoing_directory = component::OutgoingDirectory(dispatcher);
  zx::result result = outgoing_directory.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }

  result = outgoing_directory.AddUnmanagedProtocol<fidl_examples_routing_echo::Echo>(
      [dispatcher](fidl::ServerEnd<fidl_examples_routing_echo::Echo> server_end) {
        fidl::BindServer(dispatcher, std::move(server_end), std::make_unique<EchoImplementation>());
      });
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Echo protocol: " << result.status_string();
    return -1;
  }

  // Component is serving and ready to handle incoming requests
  inspector.Health().Ok();

  return loop.Run();
}
// [END main_body]
