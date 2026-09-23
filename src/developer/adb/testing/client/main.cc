// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>

#include "src/developer/adb/testing/client/adb_client.h"

int main(int argc, const char** argv) {
  FX_LOGS(INFO) << "adb-test-client started.";
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(loop.dispatcher());
  AdbClientImpl impl(loop.dispatcher());

  auto result = outgoing.AddUnmanagedProtocol<fuchsia_testing_adb::Client>(
      impl.bind_handler(loop.dispatcher()));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add protocol: " << result.status_string();
    return -1;
  }

  result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return -1;
  }

  loop.Run();
  return 0;
}
