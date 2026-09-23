// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.device/cpp/test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include "src/power/testing/fake-suspend/device_server.h"

int main() {
  FX_LOGS(INFO) << "Starting fake suspend...";

  async::Loop loop{&kAsyncLoopConfigAttachToCurrentThread};
  component::OutgoingDirectory outgoing(loop.dispatcher());

  auto device_server = std::make_shared<fake_suspend::DeviceServer>();

  fuchsia_hardware_power_suspend::SuspendService::InstanceHandler handler({
      .suspender =
          [device_server](fidl::ServerEnd<fuchsia_hardware_power_suspend::Suspender> server_end) {
            device_server->Serve(async_get_default_dispatcher(), std::move(server_end));
          },
  });

  auto result =
      outgoing.AddService<fuchsia_hardware_power_suspend::SuspendService>(std::move(handler));
  if (result.is_error()) {
    return -1;
  }

  result = outgoing.AddUnmanagedProtocol<test_suspendcontrol::Device>(
      [device_server](fidl::ServerEnd<test_suspendcontrol::Device> server_end) {
        device_server->Serve(async_get_default_dispatcher(), std::move(server_end));
      });
  if (result.is_error()) {
    return -1;
  }

  result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    return -1;
  }

  loop.Run();
  return 0;
}
