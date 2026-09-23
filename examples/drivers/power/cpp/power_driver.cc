// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "power_driver.h"

#include <fidl/fuchsia.examples/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/component/cpp/node_add_args.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fidl/cpp/client.h>

#include "examples/drivers/power/cpp/component_config.h"
#include "lib/fidl/cpp/channel.h"

namespace power {
namespace fex = fuchsia_examples;

PowerDriver::PowerDriver() : DriverBase2("power_driver") {
  // This constructor is only implemented to demonstrate the driver lifecycle.
  // Drivers are not expected to add implementation in the constructor.
}

PowerDriver::~PowerDriver() {
  fdf::info(
      "PowerDriver destructor invoked. This is called after Stop() is called and "
      "all driver dispatchers are shutdown. Use the destructor to perform any remaining teardowns.");
}

zx::result<> PowerDriver::Start(fdf::DriverContext context) {
  fdf::info(
      "PowerDriver::Start() invoked. In this function, perform the driver "
      "initialization, such as adding children.");
  config_ = context.take_config<component_config::Config>();

  auto result = InitializeSuspend(dispatcher(), context.incoming(), name());
  if (result.is_error()) {
    fdf::error("Failed to initialize suspend: {}", result);
    return result.take_error();
  }

  zx::result echo_protocol = context.incoming().Connect<fex::Echo>();
  if (echo_protocol.is_error()) {
    return echo_protocol.take_error();
  }
  fidl::Call(*echo_protocol)->EchoString({"hello world!"});
  return zx::ok();
}

void PowerDriver::Stop(fdf::StopCompleter completer) {
  fdf::info(
      "PowerDriver::Stop() invoked. This is called before "
      "the driver dispatchers are shutdown. Only implement this function "
      "if you need to manually clean up objects (ex/ unique_ptrs) in the driver dispatchers.");
  completer(zx::ok());
}

void PowerDriver::Suspend(fdf_power::SuspendCompleter cb) {
  fdf::info(
      "PowerDriver::Suspend() invoked. Use this function to perform work required before "
      "going into suspend.");
  cb();
}

void PowerDriver::Resume(fdf_power::ResumeCompleter cb) {
  fdf::info(
      "PowerDriver::Resume() invoked. Use this function to perform any work required "
      "after exiting suspend.");
  cb();
}

bool PowerDriver::SuspendEnabled() { return config_->suspend_enabled(); }

}  // namespace power

FUCHSIA_DRIVER_EXPORT2(power::PowerDriver);
