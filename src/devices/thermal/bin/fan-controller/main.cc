// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/syslog/cpp/log_settings.h>
#include <lib/syslog/cpp/macros.h>

#include "fan-controller.h"

// fan-controller is a component that bridges between the drivers implementing fuchsia_hardware_fan
// and the component implementing fuchsia_thermal::ClientStateWatcher.
// The value of `state` returned by fuchsia_thermal::ClientStateWatcher::Watch() must match fan
// levels defined and used by fuchsia_hardware_fan::Device::Set/GetFanLevel().

int main(int argc, const char* argv[], char* envp[]) {
  fuchsia_logging::LogSettingsBuilder builder;
  builder.WithTags({"fan-controller"}).BuildAndInitialize();

  auto result = component::Connect<fuchsia_thermal::ClientStateConnector>();
  if (result.is_error()) {
    FX_LOGS(ERROR) << "Could not connect to fuchsia_thermal::ClientStateConnector "
                   << result.status_string();
    return result.error_value();
  }

  async::Loop loop{&kAsyncLoopConfigNeverAttachToThread};
  zx::result svc_root = component::OpenServiceRoot();
  if (svc_root.is_error()) {
    FX_LOGS(ERROR) << "Could not open /svc " << svc_root.status_string();
    return svc_root.error_value();
  }
  fan_controller::FanController controller(loop.dispatcher(), std::move(*result),
                                           std::move(*svc_root));

  loop.Run();

  return ZX_OK;
}
