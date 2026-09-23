// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tools/create/goldens/my-driver-cpp/my_driver_cpp.h"

#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

namespace my_driver_cpp {

class MyDriverCppTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    return zx::ok();
  }
};

class MyDriverCppTestConfig final {
 public:
  using DriverType = MyDriverCpp;
  using EnvironmentType = MyDriverCppTestEnvironment;
};

class MyDriverCppTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::ForegroundDriverTest<MyDriverCppTestConfig>& driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::ForegroundDriverTest<MyDriverCppTestConfig> driver_test_;
};


TEST_F(MyDriverCppTest, ExampleTest) {
  EXPECT_TRUE(driver_test().driver() != nullptr);
}

}  // namespace my_driver_cpp
