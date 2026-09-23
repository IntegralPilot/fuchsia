// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "adb-reboot.h"

#include <fidl/fuchsia.hardware.adb/cpp/wire.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire.h>
#include <fidl/fuchsia.storage.block/cpp/markers.h>
#include <lib/async/cpp/task.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/macros.h>
#include <stdio.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include "src/storage/lib/block_client/cpp/reader_writer.h"
#include "src/storage/lib/block_client/cpp/remote_block_device.h"

namespace adb_reboot {

// From bootable/recovery/bootloader_message/bootloader_message.cpp
bool update_bootloader_message_in_struct(bootloader_message* boot,
                                         const std::vector<std::string>& options) {
  if (!boot)
    return false;
  // Replace the command & recovery fields.
  memset(boot->command, 0, sizeof(boot->command));
  memset(boot->recovery, 0, sizeof(boot->recovery));

  strlcpy(boot->command, "boot-recovery", sizeof(boot->command));

  std::string recovery = "recovery\n";
  for (const auto& s : options) {
    recovery += s;
    if (s.back() != '\n') {
      recovery += '\n';
    }
  }
  strlcpy(boot->recovery, recovery.c_str(), sizeof(boot->recovery));
  return true;
}

zx_status_t WriteBootloaderMessage(std::string arg) {
  std::string path =
      std::string("/misc/") + fidl::DiscoverableProtocolName<fuchsia_storage_block::Block>;
  zx::result<fidl::ClientEnd<fuchsia_storage_block::Block>> client_end =
      component::Connect<fuchsia_storage_block::Block>(path);
  if (client_end.is_error()) {
    FX_LOGS(ERROR) << "Failed to connect to block volume: " << client_end.status_string();
    return client_end.status_value();
  }

  zx::result device_result = block_client::RemoteBlockDevice::Create(std::move(client_end.value()));
  if (device_result.is_error()) {
    FX_LOGS(ERROR) << "Failed to create remote block device: " << device_result.status_string();
    return device_result.status_value();
  }
  std::unique_ptr<block_client::RemoteBlockDevice> device = std::move(device_result.value());
  block_client::ReaderWriter reader_writer(*device);

  bootloader_message message = {};
  update_bootloader_message_in_struct(&message, {"--" + arg});

  return reader_writer.Write(0, sizeof(message), &message);
}

void AdbReboot::ConnectToService(ConnectToServiceRequestView request,
                                 ConnectToServiceCompleter::Sync& completer) {
  auto status = Reboot({std::string(request->args.get())});
  if (status != ZX_OK) {
    completer.ReplyError(status);
    return;
  }
  completer.ReplySuccess();
}

zx_status_t AdbReboot::Reboot(std::optional<std::string> args) {
  zx_status_t status = ZX_OK;

  FX_LOGS(INFO) << "adb reboot " << args.value_or("");
  if (args->empty() || args.value().empty()) {
    status = ScheduleReboot(fuchsia_hardware_power_statecontrol::ShutdownAction::kReboot);
  } else if (args.value() == "bootloader") {
    status =
        ScheduleReboot(fuchsia_hardware_power_statecontrol::ShutdownAction::kRebootToBootloader);
  } else if (args.value() == "recovery") {
    status = ScheduleReboot(fuchsia_hardware_power_statecontrol::ShutdownAction::kRebootToRecovery);
  } else if (args.value() == "fastboot" || args.value() == "sideload" ||
             args.value() == "sideload-auto-reboot") {
    // Reference implementation in system/core/init/reboot.cpp
    status = WriteBootloaderMessage(args.value() == "sideload-auto-reboot" ? "sideload_auto_reboot"
                                                                           : args.value());
    if (status != ZX_OK) {
      FX_LOGS(ERROR) << "Failed to write bootloader message: " << zx_status_get_string(status);
      return status;
    }
    status = ScheduleReboot(fuchsia_hardware_power_statecontrol::ShutdownAction::kRebootToRecovery);
  } else {
    FX_LOGS(ERROR) << "Error: Invalid args for adb reboot: " << args.value();
    status = ZX_ERR_INVALID_ARGS;
  }

  return status;
}

zx_status_t AdbReboot::ScheduleReboot(
    const fuchsia_hardware_power_statecontrol::ShutdownAction action) {
  auto client_end = component::ConnectAt<fuchsia_hardware_power_statecontrol::Admin>(svc_);
  if (client_end.is_error()) {
    FX_LOGS(ERROR) << "Could not connect to hardware power state control: "
                   << client_end.status_string();
    return client_end.status_value();
  }

  fidl::WireSyncClient<fuchsia_hardware_power_statecontrol::Admin> reboot_client(
      std::move(*client_end));

  // Post task on dispatcher so that reboot occurs after replying to adb command.
  return async::PostTask(dispatcher_, [action, client = std::move(reboot_client)]() mutable {
    fuchsia_hardware_power_statecontrol::ShutdownReason reasons[1] = {
        fuchsia_hardware_power_statecontrol::ShutdownReason::kDeveloperRequest};
    auto reasons_view =
        fidl::VectorView<fuchsia_hardware_power_statecontrol::ShutdownReason>::FromExternal(
            reasons);

    fidl::Arena arena;
    auto builder = fuchsia_hardware_power_statecontrol::wire::ShutdownOptions::Builder(arena)
                       .action(action)
                       .reasons(reasons_view);

    auto response = client->Shutdown(builder.Build());
    if (response.status() != ZX_OK) {
      // Note: This warning might be triggered in tests when the realm builder shuts down power
      // statecontrol instance before the adb-reboot instance, which is fine.
      FX_LOGS(WARNING) << "Reboot: " << response.status_string();
    } else if (response->is_error()) {
      FX_LOGS(WARNING) << "Reboot: " << response->error_value();
    }
  });
}

}  // namespace adb_reboot
