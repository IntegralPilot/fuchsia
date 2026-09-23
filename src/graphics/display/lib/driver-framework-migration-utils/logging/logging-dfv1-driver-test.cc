// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ddk/debug.h>
#include <lib/ddk/driver.h>

#include <gtest/gtest.h>

#include "src/devices/testing/mock-ddk/mock-device.h"
#include "src/graphics/display/lib/driver-framework-migration-utils/logging/testing/dfv1-driver-with-logging.h"
#include "src/lib/testing/predicates/status.h"

namespace display {

namespace {

class DriverLoggingTest : public ::testing::Test {
 public:
  void SetUp() override {
    mock_root_ = MockDevice::FakeRootParent();

    zx_status_t create_result =
        testing::Dfv1DriverWithLogging::Create(/*ctx=*/nullptr, mock_root_.get());
    ASSERT_OK(create_result);

    device_with_logging_ = mock_root_->GetLatestChild();
    ASSERT_NE(device_with_logging_, nullptr);
  }

  void TearDown() override {
    device_async_remove(device_with_logging_);
    mock_ddk::ReleaseFlaggedDevices(mock_root_.get());
  }

  MockDevice* mock_root() const { return mock_root_.get(); }
  MockDevice* device_with_logging() const { return device_with_logging_; }

 private:
  std::shared_ptr<MockDevice> mock_root_;
  MockDevice* device_with_logging_;
};

TEST_F(DriverLoggingTest, MinimumLogLevelTrace) {
  testing::Dfv1DriverWithLogging* driver =
      device_with_logging()->GetDeviceContext<testing::Dfv1DriverWithLogging>();

  mock_ddk::SetMinLogSeverity(DDK_LOG_TRACE);
  EXPECT_TRUE(driver->LogTrace());
  EXPECT_TRUE(driver->LogDebug());
  EXPECT_TRUE(driver->LogInfo());
  EXPECT_TRUE(driver->LogWarning());
  EXPECT_TRUE(driver->LogError());
}

TEST_F(DriverLoggingTest, MinimumLogLevelDebug) {
  testing::Dfv1DriverWithLogging* driver =
      device_with_logging()->GetDeviceContext<testing::Dfv1DriverWithLogging>();

  mock_ddk::SetMinLogSeverity(DDK_LOG_DEBUG);
  EXPECT_FALSE(driver->LogTrace());
  EXPECT_TRUE(driver->LogDebug());
  EXPECT_TRUE(driver->LogInfo());
  EXPECT_TRUE(driver->LogWarning());
  EXPECT_TRUE(driver->LogError());
}

TEST_F(DriverLoggingTest, MinimumLogLevelInfo) {
  testing::Dfv1DriverWithLogging* driver =
      device_with_logging()->GetDeviceContext<testing::Dfv1DriverWithLogging>();

  mock_ddk::SetMinLogSeverity(DDK_LOG_INFO);
  EXPECT_FALSE(driver->LogTrace());
  EXPECT_FALSE(driver->LogDebug());
  EXPECT_TRUE(driver->LogInfo());
  EXPECT_TRUE(driver->LogWarning());
  EXPECT_TRUE(driver->LogError());
}

TEST_F(DriverLoggingTest, MinimumLogLevelWarning) {
  testing::Dfv1DriverWithLogging* driver =
      device_with_logging()->GetDeviceContext<testing::Dfv1DriverWithLogging>();

  mock_ddk::SetMinLogSeverity(DDK_LOG_WARNING);
  EXPECT_FALSE(driver->LogTrace());
  EXPECT_FALSE(driver->LogDebug());
  EXPECT_FALSE(driver->LogInfo());
  EXPECT_TRUE(driver->LogWarning());
  EXPECT_TRUE(driver->LogError());
}

TEST_F(DriverLoggingTest, MinimumLogLevelError) {
  testing::Dfv1DriverWithLogging* driver =
      device_with_logging()->GetDeviceContext<testing::Dfv1DriverWithLogging>();

  mock_ddk::SetMinLogSeverity(DDK_LOG_ERROR);
  EXPECT_FALSE(driver->LogTrace());
  EXPECT_FALSE(driver->LogDebug());
  EXPECT_FALSE(driver->LogInfo());
  EXPECT_FALSE(driver->LogWarning());
  EXPECT_TRUE(driver->LogError());
}

}  // namespace

}  // namespace display
