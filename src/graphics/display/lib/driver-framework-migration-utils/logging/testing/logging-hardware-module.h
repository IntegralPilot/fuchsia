// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_TESTING_LOGGING_HARDWARE_MODULE_H_
#define SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_TESTING_LOGGING_HARDWARE_MODULE_H_

namespace display::testing {

// An example class representation of a hardware module that logs to the
// driver logger.
//
// LoggingHardwareModule is designed to be linkable to both DFv1 and DFv2
// drivers.
class LoggingHardwareModule {
 public:
  LoggingHardwareModule() = default;
  ~LoggingHardwareModule() = default;

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
};

}  // namespace display::testing

#endif  // SRC_GRAPHICS_DISPLAY_LIB_DRIVER_FRAMEWORK_MIGRATION_UTILS_LOGGING_TESTING_LOGGING_HARDWARE_MODULE_H_
