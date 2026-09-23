// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "examples/drivers/simple/dfv2/simple_driver.h"

namespace testing {

// This example demonstrates a unit test for simple driver wrapped by BackgroundDriverTest,
// which executes the simple driver on a background driver dispatcher and allows tests to use
// sync FIDL clients directly from the main test thread. It's recommended to use
// BackgroundDriverTest if the unit test heavily exercises the driver-under-test through its exposed
// FIDL services, rather than its public methods.

// This class that provides custom environment configurations for the driver running
// under the test. Typically this is used to set up services that the driver interacts
// with.
class SimpleDriverTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    // Perform any additional initialization here, such as setting up compat device servers
    // and FIDL servers.
    return zx::ok();
  }
};

// Configuration class that provides information for ForegroundDriverTest. The class
// must match the following format and provide the driver class and the environment
// class.
class TestConfig final {
 public:
  using DriverType = simple::SimpleDriver;
  using EnvironmentType = SimpleDriverTestEnvironment;
};

class SimpleDriverBackgroundTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test.StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test.StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

 protected:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test;
};

TEST_F(SimpleDriverBackgroundTest, VerifyNameAndChildNode) {
  // Access the driver and verify the name.
  driver_test.RunInDriverContext(
      [](simple::SimpleDriver& driver) { EXPECT_EQ("simple_driver", driver.GetName()); });

  // Access the driver's bound node and check that it's parenting one child node.
  driver_test.RunInNodeContext([](fdf_testing::TestNode& node) {
    EXPECT_EQ(2u, node.children().size());
    EXPECT_TRUE(node.children().count("simple_child"));
    EXPECT_TRUE(node.children().count("owned_child"));
  });
}

}  // namespace testing
