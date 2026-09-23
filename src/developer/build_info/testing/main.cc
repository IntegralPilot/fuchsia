// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>

#include <memory>

#include "build_info.h"

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);
  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    return -1;
  }

  std::shared_ptr<struct fake_info> info_ref = std::make_shared<struct fake_info>();

  BuildInfoTestControllerImpl test_controller_impl(info_ref);
  result = outgoing.AddUnmanagedProtocol<fuchsia_buildinfo_test::BuildInfoTestController>(
      [&test_controller_impl,
       dispatcher](fidl::ServerEnd<fuchsia_buildinfo_test::BuildInfoTestController> server_end) {
        fidl::BindServer(dispatcher, std::move(server_end), &test_controller_impl);
      });
  if (result.is_error()) {
    return -1;
  }

  FakeProviderImpl provider_impl(info_ref);
  result = outgoing.AddUnmanagedProtocol<fuchsia_buildinfo::Provider>(
      [&provider_impl, dispatcher](fidl::ServerEnd<fuchsia_buildinfo::Provider> server_end) {
        fidl::BindServer(dispatcher, std::move(server_end), &provider_impl);
      });
  if (result.is_error()) {
    return -1;
  }

  return loop.Run();
}
