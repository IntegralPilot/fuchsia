// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>
#include <stdlib.h>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/performance/trace_manager/trace_manager.h"

namespace {

constexpr char kDefaultConfigFile[] = "/pkg/data/tracing.config";

}  // namespace

int main(int argc, char** argv) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  auto command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  if (!fxl::SetLogSettingsFromCommandLine(command_line)) {
    exit(EXIT_FAILURE);
  }

  auto config_file = command_line.GetOptionValueWithDefault("config", kDefaultConfigFile);

  tracing::Config config;
  if (!config.ReadFrom(config_file)) {
    FX_LOGS(ERROR) << "Failed to read configuration from " << config_file;
    exit(EXIT_FAILURE);
  }

  async::Executor executor{loop.dispatcher()};
  component::OutgoingDirectory outgoing = component::OutgoingDirectory(loop.dispatcher());
  if (zx::result result = outgoing.ServeFromStartupInfo(); result.is_error()) {
    FX_PLOGS(ERROR, result.status_value()) << "Failed to serve outgoing directory";
    return EXIT_FAILURE;
  }
  tracing::TraceManager trace_manager{std::move(config), executor};
  if (zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_tracing_provider::Registry>(
          trace_manager.GetRegistryHandler());
      result.is_error()) {
    FX_PLOGS(ERROR, result.status_value()) << "Failed to add Registry protocol";
    return EXIT_FAILURE;
  }
  if (zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_tracing_controller::Provisioner>(
          trace_manager.GetProvisionerHandler());
      result.is_error()) {
    FX_PLOGS(ERROR, result.status_value()) << "Failed to add Provisioner protocol";
    return EXIT_FAILURE;
  }

  FX_LOGS(DEBUG) << "TraceManager services, now serving";

  loop.Run();
  return EXIT_SUCCESS;
}
