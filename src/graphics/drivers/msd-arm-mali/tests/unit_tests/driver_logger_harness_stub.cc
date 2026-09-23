// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/testing/cpp/driver_runtime.h>

#include "driver_logger_harness.h"

DriverLoggerHarness::~DriverLoggerHarness() {}

class DriverLoggerHarnessStub : public DriverLoggerHarness {
  fdf_testing::DriverRuntime& runtime() override { return runtime_; }
  fdf_testing::DriverRuntime runtime_;
  fdf::UnownedSynchronizedDispatcher test_env_dispatcher_ = runtime_.StartBackgroundDispatcher();
};

// static
std::unique_ptr<DriverLoggerHarness> DriverLoggerHarness::Create() {
  return std::make_unique<DriverLoggerHarnessStub>();
}
