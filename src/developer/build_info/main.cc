// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include "build_info.h"

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);
  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    return -1;
  }

  ProviderImpl impl;

  result = outgoing.AddUnmanagedProtocol<fuchsia_buildinfo::Provider>(
      [&impl, dispatcher](fidl::ServerEnd<fuchsia_buildinfo::Provider> server_end) {
        fidl::BindServer(dispatcher, std::move(server_end), &impl);
      });
  if (result.is_error()) {
    return -1;
  }

  return loop.Run();
}
