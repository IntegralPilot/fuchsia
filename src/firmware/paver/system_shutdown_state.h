// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_FIRMWARE_PAVER_SYSTEM_SHUTDOWN_STATE_H_
#define SRC_FIRMWARE_PAVER_SYSTEM_SHUTDOWN_STATE_H_

#include <fidl/fuchsia.io/cpp/wire.h>
#include <fidl/fuchsia.system.state/cpp/common_types.h>

namespace paver {

/// Retrieve current System Power State from `fuchsia.system.state.SystemStateTransition`.
///
/// `fuchsia.system.state.SystemPowerState.kFullyOn` is returned if state can't be retrieved.
fuchsia_system_state::SystemPowerState GetShutdownSystemState(
    fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir);

}  // namespace paver

#endif  // SRC_FIRMWARE_PAVER_SYSTEM_SHUTDOWN_STATE_H_
