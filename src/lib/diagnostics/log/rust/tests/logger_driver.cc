// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.framework/cpp/natural_types.h>
#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/logging/cpp/logger.h>

#include <string_view>

namespace logger_driver {

class LoggerDriver : public fdf::DriverBase2 {
 public:
  LoggerDriver() : fdf::DriverBase2("logger_driver") {}

  zx::result<> Start(fdf::DriverContext context) override {
    fdf::info("Hello, Archivist!");
    return zx::ok();
  }
};

}  // namespace logger_driver

FUCHSIA_DRIVER_EXPORT2(logger_driver::LoggerDriver);
