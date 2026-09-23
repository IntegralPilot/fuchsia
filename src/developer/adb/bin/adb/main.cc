// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/component/outgoing/cpp/outgoing_directory.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include "adb.h"
#include "src/developer/adb/bin/adb/adb_config.h"
#include "src/developer/adb/bin/adb/state-controller.h"
#include "src/developer/adb/third_party/adb/adb-protocol.h"

int main(int argc, char** argv) {
  fuchsia_logging::LogSettingsBuilder log_builder;
  log_builder.WithTags({"adb"}).BuildAndInitialize();

  auto config = adb_config::Config::TakeFromStartupHandle();
  if (config.is_recovery()) {
    set_system_type(kCsRecovery);
  }

  async::Loop adb_loop{&kAsyncLoopConfigNeverAttachToThread};
  auto status = adb_loop.StartThread("adb-thread");
  if (status != ZX_OK) {
    FX_LOGS(ERROR) << "Could not start adb_loop";
    return status;
  }

  async::Loop main_loop{&kAsyncLoopConfigNeverAttachToThread};
  component::OutgoingDirectory outgoing(main_loop.dispatcher());
  adb::StateControllerServer state_controller;
  if (zx::result result = outgoing.AddUnmanagedProtocol<fuchsia_hardware_adb::StateController>(
          state_controller.bind_handler(main_loop.dispatcher()));
      result.is_error()) {
    FX_LOGS(ERROR) << "Could not add StateController protocol: " << result.status_string();
    return result.error_value();
  }

  if (zx::result result = outgoing.ServeFromStartupInfo(); result.is_error()) {
    FX_LOGS(ERROR) << "Could not serve outgoing directory: " << result.status_string();
    return result.error_value();
  }

  auto adb = adb::Adb::Create(adb_loop.dispatcher());
  if (adb.is_error()) {
    FX_LOGS(ERROR) << "Could not create adb " << adb.error_value();
    return adb.error_value();
  }

  state_controller.set_reset_callback([&]() {
    if (adb.is_ok()) {
      adb.value()->Reset();
    }
  });

  main_loop.Run();
  return ZX_OK;
}
