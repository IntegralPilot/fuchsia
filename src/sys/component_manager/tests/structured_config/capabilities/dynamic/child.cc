// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.config/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <zircon/process.h>
#include <zircon/processargs.h>

class Server : public fidl::Server<test_config::Config> {
  void Get(GetCompleter::Sync& completer) override {
    zx::vmo vmo(zx_take_startup_handle(PA_VMO_COMPONENT_CONFIG));
    completer.Reply({test_config::ConfigGetResponse(std::move(vmo))});
  }
};

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(loop.dispatcher());
  Server server;
  if (zx::result result = outgoing.AddUnmanagedProtocol<test_config::Config>(
          server.bind_handler(loop.dispatcher()));
      result.is_error()) {
    return 1;
  }

  if (zx::result result = outgoing.ServeFromStartupInfo(); result.is_error()) {
    return 1;
  }
  loop.Run();
  return 0;
}
