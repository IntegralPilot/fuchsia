// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>

// [START include]
#include "examples/drivers/config/cpp/example_config_driver_config.h"
// [END include]

namespace {
class Driver : public fdf::DriverBase2 {
 public:
  Driver() : fdf::DriverBase2("config-driver") {}

  zx::result<> Start(fdf::DriverContext context) override {
    // [START use]
    auto config = context.take_config<example_config_driver_config::Config>();
    fdf::info("My config value is: {}", config.suspend_enabled());
    // [END use]
    return zx::ok();
  }
};
}  // namespace

FUCHSIA_DRIVER_EXPORT2(Driver);
