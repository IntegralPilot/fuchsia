// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.wlan.common/cpp/fidl.h>
#include <fidl/fuchsia.wlan.sme/cpp/fidl.h>
#include <fidl/fuchsia.wlan.softmac/cpp/driver/fidl.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fdf/dispatcher.h>
#include <lib/fidl/cpp/client.h>

#include <bind/fuchsia/cpp/bind.h>
#include <bind/fuchsia/ethernet/cpp/bind.h>
#include <gtest/gtest.h>
#include <src/connectivity/wlan/drivers/wlansoftmac/softmac_driver.h>

#include "custom_environment.h"
#include "fake_wlansoftmac_server.h"

namespace wlan::drivers::wlansoftmac {
namespace {

class FixtureConfig final {
 public:
  using DriverType = SoftmacDriver;
  using EnvironmentType = CustomEnvironment<BasicWlanSoftmacServer>;
};

class GoodSoftmacDriverTest : public ::testing::Test {
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

// Verify a clean startup and shutdown when wlansoftmac does not encounter any errors
// while running.
TEST_F(GoodSoftmacDriverTest, CleanStartupAndShutdown) {}

// Verify wlansoftmac creates a child node for an ethernet driver.
TEST_F(GoodSoftmacDriverTest, VerifyChildNode) {
  driver_test().RunInNodeContext([](fdf_testing::TestNode& node) {
    ASSERT_EQ(1u, node.children().size());
    ASSERT_EQ(node.children().count("wlansoftmac-ethernet"), 1ul);

    auto expected_property =
        fdf::MakeProperty2(bind_fuchsia::PROTOCOL, bind_fuchsia_ethernet::BIND_PROTOCOL_IMPL);
    auto properties = node.children().find("wlansoftmac-ethernet")->second.GetProperties();

    ASSERT_EQ(properties.size(), 1ul);
    ASSERT_EQ(properties[0], expected_property);
  });
}

}  // namespace
}  // namespace wlan::drivers::wlansoftmac
