// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_RECOVERY_FACTORY_RESET_FACTORY_RESET_H_
#define SRC_RECOVERY_FACTORY_RESET_FACTORY_RESET_H_

#include <fidl/fuchsia.fshost/cpp/wire.h>
#include <fidl/fuchsia.hardware.power.statecontrol/cpp/wire.h>
#include <fidl/fuchsia.recovery/cpp/wire.h>
#include <zircon/types.h>

#include "src/recovery/factory_reset/factory_reset_config.h"

namespace factory_reset {

// Implements a simple version of Factory Reset that shreds zxcrypt and then
// reboots.
class FactoryReset : public fidl::WireServer<fuchsia_recovery::FactoryReset> {
 public:
  FactoryReset(async_dispatcher_t* dispatcher, fidl::ClientEnd<fuchsia_io::Directory> dev,
               fidl::ClientEnd<fuchsia_hardware_power_statecontrol::Admin> admin,
               fidl::ClientEnd<fuchsia_fshost::Admin> fshost_admin,
               factory_reset_config::Config config);
  // Performs the factory reset.
  void Reset(fit::callback<void(zx_status_t)> callback);
  void Reset(ResetCompleter::Sync& completer) override;

 private:
  // Finds the zxcrypt partition, then overwrites its superblocks with random
  // data, causing them to be unusable.
  void Shred(fit::callback<void(zx_status_t)> callback) const;

  fidl::ClientEnd<fuchsia_io::Directory> dev_;
  fidl::WireClient<fuchsia_hardware_power_statecontrol::Admin> admin_;
  fidl::WireClient<fuchsia_fshost::Admin> fshost_admin_;
  factory_reset_config::Config config_;
};

}  // namespace factory_reset

#endif  // SRC_RECOVERY_FACTORY_RESET_FACTORY_RESET_H_
