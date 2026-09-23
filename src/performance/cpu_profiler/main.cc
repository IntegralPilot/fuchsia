// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/dispatcher.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>
#include <stdlib.h>
#include <unistd.h>

#include "profiler_controller_impl.h"

int main() {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  async_dispatcher_t* dispatcher = loop.dispatcher();

  trace::TraceProviderWithFdio trace_provider(dispatcher, "cpu_profiler");

  // We request an event stream for debug_started. We need to handle this stream for the duration of
  // the program so we don't delay component start up for other components by not acking the stream
  // messages.
  profiler::ComponentWatcher event_stream{dispatcher};
  zx::result watch_result = event_stream.Watch();
  FX_CHECK(watch_result.is_ok()) << "Failed to service event_stream: "
                                 << watch_result.status_string();

  bool active_session = false;

  // Expose the FIDL server.
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(dispatcher);
  zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_cpu_profiler::Session>(
      [dispatcher, &event_stream,
       &active_session](fidl::ServerEnd<fuchsia_cpu_profiler::Session> server_end) {
        // We currently only support a single session at a time. Deny additional session if one is
        // currently in progress.
        if (active_session) {
          server_end.Close(ZX_ERR_ALREADY_EXISTS);
          return;
        }
        active_session = true;
        event_stream.Clear();
        fidl::BindServer(
            dispatcher, std::move(server_end),
            std::make_unique<profiler::ProfilerControllerImpl>(dispatcher, event_stream),
            [&active_session](profiler::ProfilerControllerImpl*, fidl::UnbindInfo,
                              fidl::ServerEnd<fuchsia_cpu_profiler::Session>) {
              active_session = false;
            });
      });
  FX_CHECK(result.is_ok()) << "Failed to expose ProfilingController protocol: "
                           << result.status_string();

  result = outgoing.ServeFromStartupInfo();
  FX_CHECK(result.is_ok()) << "Failed to serve outgoing directory: " << result.status_string();
  return loop.Run() == ZX_OK;
}
