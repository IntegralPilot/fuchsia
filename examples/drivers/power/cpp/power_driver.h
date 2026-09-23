// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef EXAMPLES_DRIVERS_POWER_CPP_POWER_DRIVER_H_
#define EXAMPLES_DRIVERS_POWER_CPP_POWER_DRIVER_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/power/cpp/suspend.h>

#include "examples/drivers/power/cpp/component_config.h"

namespace power {

// Power driver that demonstrates how the Suspend() and Resume() functions are registered and
// invoked.
class PowerDriver final : public fdf::DriverBase2, public fdf_power::Suspendable<PowerDriver> {
 public:
  PowerDriver();

  std::optional<fidl::ServerEnd<fuchsia_power_broker::ElementRunner>> take_power_element_runner() {
    return std::nullopt;
  }

  // Only implemented in this example to demonstrate the driver lifecycle. Drivers should avoid
  // implementing the destructor and perform in Start() and PrepareStop().
  ~PowerDriver();

  // Called by the Driver Framework to initialize the driver instance.
  zx::result<> Start(fdf::DriverContext context) override;

  // Called by the Driver Framework before it shutdowns all of the driver's fdf_dispatchers.
  // The driver should use this function to initiate any teardowns on the fdf_dispatchers before
  // they're stopped and deallocated by the Driver Framework.
  void Stop(fdf::StopCompleter completer) override;

  void Suspend(fdf_power::SuspendCompleter cb) override;
  void Resume(fdf_power::ResumeCompleter cb) override;
  bool SuspendEnabled() override;

 private:
  std::optional<component_config::Config> config_;
};

}  // namespace power

#endif  // EXAMPLES_DRIVERS_POWER_CPP_POWER_DRIVER_H_
