// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/syslog/structured_backend/fuchsia_syslog.h>

#include <vector>

#include <gtest/gtest.h>

#include "src/graphics/display/lib/driver-framework-migration-utils/logging/testing/dfv2-driver-with-logging.h"
#include "src/lib/diagnostics/fake-log-sink/cpp/fake_log_sink.h"
#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

using ::testing::Values;

class DriverLoggingTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    zx::result<> remove_result =
        to_driver_vfs.component().RemoveProtocol<fuchsia_logger::LogSink>();
    if (remove_result.is_error() && remove_result.error_value() != ZX_ERR_NOT_FOUND) {
      fdf::error("Failed to remove LogSink protocol: {}", remove_result.status_string());
    }
    return to_driver_vfs.component().AddUnmanagedProtocol<fuchsia_logger::LogSink>(
        [this](fidl::ServerEnd<fuchsia_logger::LogSink> server_end) {
          log_sinks_.emplace_back(severity_, std::move(server_end));
        });
  }

  void SetSeverity(fuchsia_logging::RawLogSeverity severity) { severity_ = severity; }

 private:
  fuchsia_logging::RawLogSeverity severity_ = FUCHSIA_LOG_INFO;
  std::vector<fuchsia_logging::FakeLogSink> log_sinks_;
};

class TestConfig final {
 public:
  using DriverType = testing::Dfv2DriverWithLogging;
  using EnvironmentType = DriverLoggingTestEnvironment;
};

class DriverLoggingTest : public ::testing::TestWithParam<fuchsia_logging::RawLogSeverity> {
 public:
  void SetUp() override {
    driver_test().RunInEnvironmentTypeContext(
        [severity = GetParam()](DriverLoggingTestEnvironment& env) { env.SetSeverity(severity); });

    zx::result<> start_result = driver_test().StartDriver();
    EXPECT_OK(start_result);
  }

  void TearDown() override {
    zx::result<> stop_result = driver_test().StopDriver();
    EXPECT_OK(stop_result);
  }

  testing::Dfv2DriverWithLogging* driver() { return driver_test().driver(); }

  fdf_testing::ForegroundDriverTest<TestConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<TestConfig> driver_test_;
};

TEST_P(DriverLoggingTest, MinimumLogLevel) {
  EXPECT_EQ(driver()->LogTrace(), GetParam() <= FUCHSIA_LOG_TRACE);
  EXPECT_EQ(driver()->LogDebug(), GetParam() <= FUCHSIA_LOG_DEBUG);
  EXPECT_EQ(driver()->LogInfo(), GetParam() <= FUCHSIA_LOG_INFO);
  EXPECT_EQ(driver()->LogWarning(), GetParam() <= FUCHSIA_LOG_WARNING);
  EXPECT_EQ(driver()->LogError(), GetParam() <= FUCHSIA_LOG_ERROR);
}

INSTANTIATE_TEST_SUITE_P(, DriverLoggingTest,
                         Values(FUCHSIA_LOG_TRACE, FUCHSIA_LOG_DEBUG, FUCHSIA_LOG_INFO,
                                FUCHSIA_LOG_WARNING, FUCHSIA_LOG_ERROR));

}  // namespace

}  // namespace display
