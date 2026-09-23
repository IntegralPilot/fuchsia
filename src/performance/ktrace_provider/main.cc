// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.kernel/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/scheduler/role.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/trace-provider/provider.h>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/performance/ktrace_provider/app.h"

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  if (!fxl::SetLogSettingsFromCommandLine(command_line))
    return 1;

  trace::TraceProviderWithFdio trace_provider(loop.dispatcher(), "ktrace_provider");
  trace_provider.SetGetKnownCategoriesCallback(ktrace_provider::GetKnownCategories);

  auto tracing_client_end = component::Connect<fuchsia_kernel::TracingResource>();
  if (tracing_client_end.is_error()) {
    FX_PLOGS(ERROR, tracing_client_end.error_value())
        << "Failed to get connect to tracing resource";
    return 1;
  }
  auto tracing_result = fidl::SyncClient(std::move(*tracing_client_end))->Get();
  if (!tracing_result.is_ok()) {
    FX_LOGS(ERROR) << tracing_result.error_value() << " Failed to get tracing resource";
    return 1;
  }

  // Apply the scheduler role defined for kernel trace reading.
  std::vector<fuchsia_scheduler::RoleParameter> input_args;
  const zx::result output_args =
      fuchsia_scheduler::SetRoleForThisThread("fuchsia.ktrace.reader", input_args);

  // Default to 2300us every 10ms -- a value somewhat arbitrarily chosen by looking at how long it
  // takes to copy data during a trace with all kernel categories enabled.
  zx::duration max_readout_time = zx::usec(2'300);
  zx::duration poll_period = zx::msec(10);
  if (output_args.is_ok()) {
    for (const auto& [arg_name, arg_val] : *output_args) {
      if (arg_name == "max_readout_time") {
        if (std::holds_alternative<int64_t>(arg_val)) {
          max_readout_time = zx::nsec(std::get<int64_t>(arg_val));
        } else {
          FX_LOGS(ERROR)
              << "Failed to get profile capacity from in 'fuchsia.ktrace.reader'. Poll timings aren't synchronized.";
        }
      } else if (arg_name == "poll_period") {
        if (std::holds_alternative<int64_t>(arg_val)) {
          poll_period = zx::nsec(std::get<int64_t>(arg_val));
        } else {
          FX_LOGS(ERROR)
              << "Failed to get profile capacity from in 'fuchsia.ktrace.reader'. Poll timings aren't synchronized.";
        }
      }
    }
  } else {
    FX_PLOGS(WARNING, output_args.error_value()) << "Failed to apply profile to main thread";
  }

  ktrace_provider::App app(std::move(tracing_result->resource()), command_line, poll_period,
                           max_readout_time);
  loop.Run();
  return 0;
}
