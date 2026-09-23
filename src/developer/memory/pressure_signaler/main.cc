// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>

#include <filesystem>

#include "src/developer/memory/pressure_signaler/debugger.h"
#include "src/developer/memory/pressure_signaler/pressure_notifier.h"

namespace {
const char kNotfiyCrashReporterPath[] = "/config/data/send_critical_pressure_crash_reports";
bool SendCriticalMemoryPressureCrashReports() {
  std::error_code _ignore;
  return std::filesystem::exists(kNotfiyCrashReporterPath, _ignore);
}

}  // namespace

int main(int argc, const char** argv) {
  FX_LOGS(DEBUG) << argv[0] << ": starting";

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();
  trace::TraceProviderWithFdio trace_provider(dispatcher);

  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);
  zx::result result = outgoing.ServeFromStartupInfo();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve the outgoing directory from startup info: "
                   << result.status_string();
    return -1;
  }

  auto client_end = component::Connect<fuchsia_feedback::CrashReporter>();
  if (!client_end.is_ok()) {
    FX_LOGS(ERROR) << "Failed to connect to fuchsia.feedback::CrashReporter: "
                   << client_end.status_string();
    return -1;
  }
  auto crash_reporter =
      fidl::Client<fuchsia_feedback::CrashReporter>(std::move(client_end.value()), dispatcher);
  auto notifier = std::make_unique<pressure_signaler::PressureNotifier>(
      /* watch_for_changes = */ true, SendCriticalMemoryPressureCrashReports(),
      std::move(crash_reporter), dispatcher);
  auto debugger = std::make_unique<pressure_signaler::MemoryDebugger>(notifier.get());

  result = outgoing.AddProtocol<fuchsia_memorypressure::Provider>(std::move(notifier));

  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve the fuchsia.memorypressure::Provider protocol: "
                   << result.status_string();
    return -1;
  }

  result = outgoing.AddProtocol<fuchsia_memory_debug::MemoryPressure>(std::move(debugger));
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Failed to serve the fuchsia.memory.debug::MemoryPressure protocol: "
                   << result.status_string();
    return -1;
  }

  loop.Run();
  FX_LOGS(DEBUG) << argv[0] << ": exiting";

  return 0;
}
