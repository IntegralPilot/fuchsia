// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.crash/cpp/fidl.h>
#include <fidl/fuchsia.sys2/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include "src/developer/forensics/testing/fakes/crash_introspect.h"

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"forensics", "test"}).BuildAndInitialize();

  FX_LOGS(INFO) << "Starting FakeCrashIntrospect";

  component::OutgoingDirectory outgoing(loop.dispatcher());

  if (const zx::result result = outgoing.ServeFromStartupInfo(); result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve outgoing directory: " << result.status_string();
    return EXIT_FAILURE;
  }

  if (const zx::result result = outgoing.AddProtocol<fuchsia_sys2::CrashIntrospect>(
          std::make_unique<forensics::fakes::CrashIntrospect>());
      result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add CrashIntrospect protocol: " << result.status_string();
    return EXIT_FAILURE;
  }

  if (const zx::result result = outgoing.AddProtocol<fuchsia_driver_crash::CrashIntrospect>(
          std::make_unique<forensics::fakes::CrashIntrospect>());
      result.is_error()) {
    FX_LOGS(ERROR) << "Failed to add CrashIntrospect protocol: " << result.status_string();
    return EXIT_FAILURE;
  }

  loop.Run();
  return EXIT_SUCCESS;
}
