// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.test/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/driver_test_realm/src/boot_items.h>
#include <lib/driver_test_realm/src/root_job.h>
#include <lib/driver_test_realm/src/system_state.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include <sdk/lib/driver_test_realm/dtr_support_config.h>

int main(int argc, const char** argv) {
  async::Loop loop(&kAsyncLoopConfigNeverAttachToThread);
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithDispatcher(loop.dispatcher()).BuildAndInitialize();

  async_dispatcher_t* dispatcher = loop.dispatcher();

  component::OutgoingDirectory outgoing(dispatcher);

  driver_test_realm::BootItems boot_items;
  driver_test_realm::SystemStateTransition system_state;
  driver_test_realm::RootJob root_job;

  auto config = dtr_support_config::Config::TakeFromStartupHandle();

  boot_items.SetBoardName(config.board_name());
  if (config.platform_vid() != "") {
    char* end = nullptr;
    uint64_t vid = (std::strtoul(config.platform_vid().c_str(), &end, 10));
    if (errno != ERANGE && end && *end == '\0') {
      if (vid < std::numeric_limits<uint32_t>::max()) {
        boot_items.SetVid(static_cast<uint32_t>(vid));
      } else {
        FX_LOGS(ERROR) << "Platform VID number over unsigned 32 bit max: " << vid;
      }
    } else {
      FX_LOGS(ERROR) << "Failed to parse vid";
      errno = 0;
    }
  }
  if (config.platform_pid() != "") {
    char* end = nullptr;
    uint64_t pid = std::strtoul(config.platform_pid().c_str(), &end, 10);
    if (errno != ERANGE && end && *end == '\0') {
      if (pid < std::numeric_limits<uint32_t>::max()) {
        boot_items.SetPid(static_cast<uint32_t>((pid)));
      } else {
        FX_LOGS(ERROR) << "Platform PID number over unsigned 32 bit max: " << pid;
      }

    } else {
      FX_LOGS(ERROR) << "Failed to parse pid.";
      errno = 0;
    }
  }

  auto boot_result = component::Connect<fuchsia_driver_test::ResourceProvider>();
  fidl::SyncClient<fuchsia_driver_test::ResourceProvider> client(std::move(boot_result.value()));
  auto dt_result = client->GetDeviceTree();
  if (dt_result.is_ok()) {
    boot_items.SetDeviceTree(std::move(dt_result->devicetree()));
  }

  zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_boot::Items>(
      [&boot_items, dispatcher, tunnel_boot_items = config.tunnel_boot_items()](
          fidl::ServerEnd<fuchsia_boot::Items> server_end) {
        auto result = boot_items.Serve(dispatcher, std::move(server_end), tunnel_boot_items);
        if (result.is_error()) {
          FX_LOGS(ERROR) << "Failed to tunnel fuchsia_boot::Items" << result.status_string();
        }
      });
  ZX_ASSERT(result.is_ok());

  result = outgoing.AddUnmanagedProtocol<fuchsia_system_state::SystemStateTransition>(
      system_state.CreateHandler(dispatcher));
  ZX_ASSERT(result.is_ok());

  result =
      outgoing.AddUnmanagedProtocol<fuchsia_kernel::RootJob>(root_job.CreateHandler(dispatcher));
  ZX_ASSERT(result.is_ok());

  result = outgoing.ServeFromStartupInfo();
  ZX_ASSERT(result.is_ok());

  loop.Run();
  return 0;
}
