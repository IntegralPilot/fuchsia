// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/magma/platform/zircon/zircon_platform_logger_dfv2.h>

#include <gtest/gtest.h>

#include "driver_logger_harness.h"

DriverLoggerHarness::~DriverLoggerHarness() {}

namespace {

class DriverLoggerHarnessDFv2 : public DriverLoggerHarness {
 public:
  ~DriverLoggerHarnessDFv2() override = default;

  void Initialize();
  fdf_testing::DriverRuntime& runtime() override { return runtime_; }

 private:
  fdf_testing::DriverRuntime runtime_;
  fdf_testing::ScopedGlobalLogger logger_;

  fit::deferred_callback logger_callback_;
};

void DriverLoggerHarnessDFv2::Initialize() {
  logger_callback_ = magma::InitializePlatformLoggerForDFv2(&logger_.logger(), "mali");
}

}  // namespace

// static
std::unique_ptr<DriverLoggerHarness> DriverLoggerHarness::Create() {
  auto harness = std::make_unique<DriverLoggerHarnessDFv2>();
  harness->Initialize();
  return std::move(harness);
}
