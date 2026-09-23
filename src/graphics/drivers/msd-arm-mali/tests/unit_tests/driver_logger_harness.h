// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_TESTS_UNIT_TESTS_DRIVER_LOGGER_HARNESS_H_
#define SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_TESTS_UNIT_TESTS_DRIVER_LOGGER_HARNESS_H_

#include <lib/driver/testing/cpp/driver_runtime.h>

#include <memory>

// This class initializes the driver runtime and magma logging, which are necessary for some tests
// to work.
class DriverLoggerHarness {
 public:
  virtual ~DriverLoggerHarness() = 0;

  virtual fdf_testing::DriverRuntime& runtime() = 0;

  static std::unique_ptr<DriverLoggerHarness> Create();
};

#endif  // SRC_GRAPHICS_DRIVERS_MSD_ARM_MALI_TESTS_UNIT_TESTS_DRIVER_LOGGER_HARNESS_H_
