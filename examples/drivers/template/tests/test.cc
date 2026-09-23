// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "examples/drivers/template/template_driver.h"

namespace testing {

class SimpleDriverTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    // Perform any additional initialization here, such as setting up compat device servers
    // and FIDL servers.
    return zx::ok();
  }
};

class FixtureConfig final {
 public:
  using DriverType = template_driver::TemplateDriver;
  using EnvironmentType = SimpleDriverTestEnvironment;
};

class SimpleDriverTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(SimpleDriverTest, VerifyChildNode) {
  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    EXPECT_EQ(1u, node.children().size());
    EXPECT_TRUE(node.children().count("example_child"));
  });
}

}  // namespace testing
