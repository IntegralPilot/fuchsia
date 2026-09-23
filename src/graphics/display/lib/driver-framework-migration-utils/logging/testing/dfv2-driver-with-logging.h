// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_TESTING_DFV2_DRIVER_WITH_LOGGING_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_TESTING_DFV2_DRIVER_WITH_LOGGING_H_

#include <lib/driver/component/cpp/driver_base2.h>
#include <lib/zx/result.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/testing/logging-hardware-module.h"

namespace display::testing {

class Dfv2DriverWithLogging : public fdf::DriverBase2 {
 public:
  Dfv2DriverWithLogging();
  ~Dfv2DriverWithLogging() override;

  // Implements `fdf::DriverBase`.
  zx::result<> Start(fdf::DriverContext context) override;

  // Logs a TRACE level message and returns true if TRACE log level is enabled.
  // Returns false otherwise.
  bool LogTrace() const;

  // Logs a DEBUG level message and returns true if DEBUG log level is enabled.
  // Returns false otherwise.
  bool LogDebug() const;

  // Logs an INFO level message and returns true if INFO log level is enabled.
  // Returns false otherwise.
  bool LogInfo() const;

  // Logs a WARNING level message and returns true if WARNING log level is
  // enabled. Returns false otherwise.
  bool LogWarning() const;

  // Logs an ERROR level message and returns true if ERROR log level is enabled.
  // Returns false otherwise.
  bool LogError() const;

 private:
  testing::LoggingHardwareModule logging_hardware_module_;
};

}  // namespace display::testing

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_TESTING_DFV2_DRIVER_WITH_LOGGING_H_
