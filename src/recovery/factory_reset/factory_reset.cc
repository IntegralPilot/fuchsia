// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "factory_reset.h"

#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <lib/syslog/cpp/macros.h>
#include <zircon/errors.h>
#include <zircon/status.h>

#include "src/recovery/factory_reset/factory_reset_config.h"
#include "src/security/lib/kms-stateless/kms-stateless.h"
namespace factory_reset {

FactoryReset::FactoryReset(async_dispatcher_t* dispatcher,
                           fidl::ClientEnd<fuchsia_io::Directory> dev,
                           fidl::ClientEnd<fuchsia_hardware_power_statecontrol::Admin> admin,
                           fidl::ClientEnd<fuchsia_fshost::Admin> fshost_admin,
                           factory_reset_config::Config config)
    : dev_(std::move(dev)),
      admin_(std::move(admin), dispatcher),
      fshost_admin_(std::move(fshost_admin), dispatcher),
      config_(config) {}

void FactoryReset::Shred(fit::callback<void(zx_status_t)> callback) const {
  // First try and shred the data volume using fshost.
  auto cb = [callback = std::move(callback)](const auto& result) mutable {
    callback([&result]() {
      if (result.ok()) {
        const fit::result response = result.value();
        if (response.is_ok()) {
          FX_LOGS(INFO) << "fshost ShredDataVolume succeeded";
          return ZX_OK;
        }
        return response.error_value();
      } else {
        FX_LOGS(ERROR) << "Failed to call ShredDataVolume: " << result.FormatDescription();
        return result.status();
      }
    }());
  };
  fshost_admin_->ShredDataVolume().ThenExactlyOnce(std::move(cb));
}

void FactoryReset::Reset(fit::callback<void(zx_status_t)> callback) {
  FX_LOGS(INFO) << "Reset called. Starting shred";
  Shred([this, callback = std::move(callback)](zx_status_t status) mutable {
    if (status != ZX_OK) {
      FX_PLOGS(ERROR, status) << "Shred failed";
      callback(status);
      return;
    }
    FX_LOGS(INFO) << "Finished shred";

    uint8_t key_info[kms_stateless::kExpectedKeyInfoSize] = "zxcrypt";
    switch (zx_status_t status = kms_stateless::RotateHardwareDerivedKeyFromService(key_info);
            status) {
      case ZX_OK:
        break;
      case ZX_ERR_NOT_SUPPORTED:
        FX_LOGS(WARNING)
            << "FactoryReset: The device does not support rotatable hardware keys. Ignoring";
        break;
      default:
        FX_PLOGS(ERROR, status) << "FactoryReset: RotateHardwareDerivedKey() failed";
        callback(status);
        return;
    }
    // Reboot to initiate the recovery.
    FX_LOGS(INFO) << "Requesting reboot...";
    auto cb = [callback = std::move(callback)](const auto& result) mutable {
      if (!result.ok()) {
        FX_PLOGS(ERROR, result.status()) << "Reboot call failed";
        callback(result.status());
        return;
      }
      const auto& response = result.value();
      if (response.is_error()) {
        FX_PLOGS(ERROR, response.error_value()) << "Reboot returned error";
        callback(response.error_value());
        return;
      }
      callback(ZX_OK);
    };
    fidl::Arena arena;
    auto builder = fuchsia_hardware_power_statecontrol::wire::ShutdownOptions::Builder(arena);
    builder.action(fuchsia_hardware_power_statecontrol::ShutdownAction::kReboot);
    fuchsia_hardware_power_statecontrol::ShutdownReason reasons[1] = {
        fuchsia_hardware_power_statecontrol::ShutdownReason::kFactoryDataReset};
    auto vector_view =
        fidl::VectorView<fuchsia_hardware_power_statecontrol::ShutdownReason>::FromExternal(
            reasons);
    builder.reasons(vector_view);
    admin_->Shutdown(builder.Build()).ThenExactlyOnce(std::move(cb));
  });
}

void FactoryReset::Reset(ResetCompleter::Sync& completer) {
  Reset([completer = completer.ToAsync()](zx_status_t status) mutable { completer.Reply(status); });
}

}  // namespace factory_reset
