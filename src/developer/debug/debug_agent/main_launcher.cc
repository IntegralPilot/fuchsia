// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.component.decl/cpp/fidl.h>
#include <fidl/fuchsia.component/cpp/fidl.h>
#include <fidl/fuchsia.debugger/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/incoming/cpp/service.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/fdio/spawn.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/channel.h>
#include <lib/zx/process.h>
#include <zircon/errors.h>
#include <zircon/processargs.h>

#include "src/developer/debug/debug_agent/launcher.h"

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.BuildAndInitialize();

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(loop.dispatcher());

  zx::result res =
      outgoing.AddProtocol<fuchsia_debugger::Launcher>(std::make_unique<DebugAgentLauncher>());
  if (res.is_error()) {
    FX_LOGS(ERROR) << "Failed to add Launcher protocol: " << res.status_string();
    return -1;
  }

  res = outgoing.ServeFromStartupInfo();
  if (res.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << res.status_string();
    return -1;
  }

  FX_LOGS(INFO) << "Start listening on FIDL fuchsia::debugger::Launcher.";
  return loop.Run();
}
