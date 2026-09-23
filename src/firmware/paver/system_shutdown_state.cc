// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/firmware/paver/system_shutdown_state.h"

#include <fidl/fuchsia.system.state/cpp/fidl.h>

#include <sdk/lib/component/incoming/cpp/protocol.h>

#include "src/firmware/paver/pave-logging.h"

using fuchsia_system_state::SystemPowerState;
using fuchsia_system_state::SystemStateTransition;

namespace paver {

SystemPowerState GetShutdownSystemState(fidl::UnownedClientEnd<fuchsia_io::Directory> svc_dir) {
  constexpr SystemPowerState kDefaultShutdownSystemState = SystemPowerState::kFullyOn;

  zx::result client = component::ConnectAt<SystemStateTransition>(svc_dir);
  if (client.is_error()) {
    ERROR("Failed to connect to fuchsia.system.state/SystemStateTransition: %s",
          client.status_string());
    return kDefaultShutdownSystemState;
  }

  fidl::WireResult result = fidl::WireCall(client.value())->GetTerminationSystemState();
  if (!result.ok()) {
    ERROR("Failed to get termination state: %s", result.status_string());
    return kDefaultShutdownSystemState;
  }

  return result->state;
}

}  // namespace paver
