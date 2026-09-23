// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef LIB_DRIVER_TESTING_CPP_SCOPED_GLOBAL_LOGGER_H_
#define LIB_DRIVER_TESTING_CPP_SCOPED_GLOBAL_LOGGER_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/logging/cpp/logger.h>

#include <memory>

namespace fdf_testing {

// This is for unit tests that want to use FDF_LOG macros. It does this by wrapping the logic
// to create an fdf::Logger instance and setting the global instance of the logger to it.
//
// In DFv2, drivers normally set a global logging instance to be used by the driver logging macros.
// In unit tests that only test functions internal to the driver (and do not instantiate the whole
// driver with a `DriverBase::Start` call), tests must manually create an fdf::Logger to set as the
// global logger.
class ScopedGlobalLogger {
 public:
  explicit ScopedGlobalLogger(FuchsiaLogSeverity min_severity = FUCHSIA_LOG_INFO);
  ~ScopedGlobalLogger();

  fdf::Logger& logger() { return *logger_.get(); }

 private:
  // Background loop running the logger.
  async::Loop loop_;
  // The logger instance. Must be created and destroyed on the loop_'s dispatcher.
  std::unique_ptr<fdf::Logger> logger_;
};

}  // namespace fdf_testing

#endif  // LIB_DRIVER_TESTING_CPP_SCOPED_GLOBAL_LOGGER_H_
