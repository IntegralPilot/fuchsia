// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <fidl/fuchsia.boot/cpp/wire.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/fidl/cpp/interface_request.h>
#include <lib/fidl/cpp/wire/client.h>
#include <lib/fidl/cpp/wire/connect_service.h>
#include <lib/fidl/cpp/wire/internal/transport_channel.h>
#include <lib/sys/cpp/component_context.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/vfs/cpp/pseudo_dir.h>
#include <lib/zx/channel.h>
#include <string.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include <cerrno>
#include <memory>

#include <fbl/unique_fd.h>

#include "src/lib/fxl/command_line.h"
#include "src/lib/fxl/log_settings_command_line.h"
#include "src/sys/early_boot_instrumentation/coverage_source.h"

int main(int argc, char** argv) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  static const std::string static_dir_name(early_boot_instrumentation::kStaticDir);
  static const std::string dynamic_dir_name(early_boot_instrumentation::kDynamicDir);
  const fxl::CommandLine command_line = fxl::CommandLineFromArgcArgv(argc, argv);
  fxl::SetLogSettingsFromCommandLine(command_line, {"early-boot-instrumentation"},
                                     loop.dispatcher());

  early_boot_instrumentation::SinkDirMap sink_map;
  [&sink_map]() {
    zx::result client_end = component::Connect<fuchsia_boot::SvcStashProvider>();
    if (client_end.is_error()) {
      FX_PLOGS(ERROR, client_end.status_value())
          << "Failed to connect to "
          << fidl::DiscoverableProtocolDefaultPath<fuchsia_boot::SvcStashProvider>;
      return;
    }
    fidl::WireSyncClient client(std::move(client_end.value()));
    const fidl::WireResult result = client->Get();
    if (!result.ok()) {
      // TODO(https://fxbug.dev/42075300): s/WARNING/ERROR/.
      FX_PLOGS(WARNING, result.status()) << "Failed to call fuchsia.boot/SvcStashProvider.Get";
      return;
    }
    const fit::result response = result.value();
    if (response.is_error()) {
      FX_PLOGS(ERROR, response.error_value())
          << "Error response from fuchsia.boot/SvcStashProvider.Get";
      return;
    }
    sink_map = early_boot_instrumentation::ExtractDebugData(std::move(response->resource));
  }();

  auto out_logs = std::make_unique<vfs::PseudoDir>();
  [&sink_map, &out_logs]() {
    zx::result client_end = component::Connect<fuchsia_boot::Items>();
    if (client_end.is_error()) {
      FX_PLOGS(ERROR, client_end.status_value())
          << "Failed to connect to "
          << fidl::DiscoverableProtocolDefaultPath<fuchsia_boot::SvcStashProvider>;
      return;
    }
    if (auto res = early_boot_instrumentation::ExposeDebugDataZbiItem(std::move(client_end).value(),
                                                                      *out_logs, sink_map);
        res.is_error()) {
      FX_PLOGS(ERROR, res.status_value()) << "Failed to expose ZBI_ITEM_TYPE_DEBUGDATA";
      return;
    }
  }();

  // Even if we fail to populate from the sources, we expose empty directories,
  // such that the contract remains.
  fbl::unique_fd boot_instrumentation_data_dir(open("/boot/kernel/i", O_RDONLY));
  if (!boot_instrumentation_data_dir) {
    if (errno != ENOENT) {
      const char* err = strerror(errno);
      FX_LOGS(ERROR)
          << "Could not obtain handle to '/boot/kernel/i'(boot instrumentation data directory). "
          << err;
    }
  } else {
    fbl::unique_fd boot_logs(openat(boot_instrumentation_data_dir.get(), "logs", O_RDONLY));

    if (auto res = early_boot_instrumentation::ExposeBootDebugdata(boot_instrumentation_data_dir,
                                                                   sink_map);
        res.is_error()) {
      FX_LOGS(ERROR) << "Could not expose kernel profile data. " << res.status_value();
    }

    if (!boot_logs) {
      FX_LOGS(ERROR) << "Failed to obtain logs directory handle." << strerror(errno);
    }
    if (auto res = early_boot_instrumentation::ExposeLogs(boot_logs, *out_logs); res.is_error()) {
      FX_LOGS(ERROR) << "Failed to expose logs from logs directory." << res.status_value();
    }
  }

  auto debug_data = std::make_unique<vfs::PseudoDir>();
  for (auto& [sink, root] : sink_map) {
    debug_data->AddEntry(sink, std::move(root));
  }
  sink_map.clear();

  std::unique_ptr context = sys::ComponentContext::Create();
  context->outgoing()->root_dir()->AddEntry("debugdata", std::move(debug_data));
  context->outgoing()->root_dir()->AddEntry("logs", std::move(out_logs));
  if (zx_status_t status = context->outgoing()->ServeFromStartupInfo(loop.dispatcher());
      status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Could not serve outgoing directory";
  }
  if (zx_status_t status = loop.Run(); status != ZX_OK) {
    FX_PLOGS(FATAL, status) << "Could not run async loop";
  };
  return 0;
}
