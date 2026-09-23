// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/last_reboot/last_reboot_info_provider.h"

#include <lib/syslog/cpp/macros.h>

#include "src/developer/forensics/feedback/reboot_log/final_shutdown_info.h"

namespace forensics {
namespace last_reboot {

LastRebootInfoProvider::LastRebootInfoProvider(
    const feedback::FinalShutdownInfo& final_shutdown_info) {
  if (final_shutdown_info.Uptime().has_value()) {
    last_reboot_.set_uptime(final_shutdown_info.Uptime()->get());
  }

  if (final_shutdown_info.Runtime().has_value()) {
    last_reboot_.set_runtime(final_shutdown_info.Runtime()->get());
  }

  if (const std::optional<bool> graceful = final_shutdown_info.OptionallyGraceful();
      graceful.has_value()) {
    last_reboot_.set_graceful(graceful.value());
  }

  if (const std::optional<bool> planned = final_shutdown_info.OptionallyPlanned();
      planned.has_value()) {
    last_reboot_.set_planned(planned.value());
  }

  if (const std::optional<::fuchsia::feedback::RebootReason> fidl_reboot_reason =
          final_shutdown_info.ToFidlRebootReason();
      fidl_reboot_reason.has_value()) {
    last_reboot_.set_reason(fidl_reboot_reason.value());
  }
}

void LastRebootInfoProvider::Get(GetCallback callback) {
  fuchsia::feedback::LastReboot last_reboot;

  if (const zx_status_t status = last_reboot_.Clone(&last_reboot); status != ZX_OK) {
    FX_PLOGS(ERROR, status) << "Error cloning |last_reboot_|";
  }

  callback(std::move(last_reboot));
}

}  // namespace last_reboot
}  // namespace forensics
