// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.power.system/cpp/fidl.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>

namespace {

class Driver final : public fdf::DriverBase2 {
 public:
  Driver() : fdf::DriverBase2("example_driver") {}

  zx::result<> Start(fdf::DriverContext context) override {
    auto broker = context.incoming().Connect<fuchsia_power_broker::Topology>();
    if (broker.is_error()) {
      fdf::info("Failed to connect to broker");
    }

    auto sag = context.incoming().Connect<fuchsia_power_system::ActivityGovernor>();
    if (sag.is_error()) {
      fdf::info("Failed to connect to sag");
    }

    // Use the GetPowerElements call to see if we are successfully connected to the test realm's.
    fidl::Result power_elements = fidl::Call(*sag)->GetPowerElements();
    if (power_elements.is_error()) {
      fdf::info("Failed to GetPowerElements from SAG: {}",
                power_elements.error_value().FormatDescription());
    } else {
      fdf::info("Successfully did GetPowerElements.");
    }

    auto cpu_element = context.incoming().Connect<fuchsia_power_system::CpuElementManager>();
    if (cpu_element.is_error()) {
      fdf::info("Failed to connect to cpu element manager");
    }

    return zx::ok();
  }
};
}  // namespace

FUCHSIA_DRIVER_EXPORT2(Driver);
