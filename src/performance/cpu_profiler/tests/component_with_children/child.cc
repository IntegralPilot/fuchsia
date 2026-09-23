// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.examples/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

class EchoImpl : public fidl::Server<fuchsia_examples::Echo> {
 public:
  void EchoString(EchoStringRequest& request, EchoStringCompleter::Sync& completer) override {
    // Call |Reply| to reply synchronously with the request value.
    completer.Reply({{.response = request.value()}});
  }
  void SendString(SendStringRequest& request, SendStringCompleter::Sync& completer) override {}
};

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  // Expose the FIDL server.
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);
  zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_examples::Echo>(
      [dispatcher](fidl::ServerEnd<fuchsia_examples::Echo> server_end) {
        fidl::BindServer(dispatcher, std::move(server_end), std::make_unique<EchoImpl>());
      });
  FX_CHECK(result.is_ok()) << "Failed to expose echo protocol: " << result.status_string();

  result = outgoing.ServeFromStartupInfo();
  FX_CHECK(result.is_ok()) << "Failed to serve outgoing directory: " << result.status_string();
  return loop.Run() == ZX_OK;
}
