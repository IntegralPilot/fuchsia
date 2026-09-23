// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.wlan.sme/cpp/fidl.h>
#include <fidl/fuchsia.wlan.softmac/cpp/driver/fidl.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fidl/cpp/client.h>

#include <chrono>
#include <thread>

#include <gtest/gtest.h>
#include <src/connectivity/wlan/drivers/wlansoftmac/softmac_driver.h>

#include "custom_environment.h"
#include "fake_wlansoftmac_server.h"

namespace wlan::drivers::wlansoftmac {
namespace {

template <typename Environment>
class FixtureConfig final {
 public:
  using DriverType = SoftmacDriver;
  using EnvironmentType = Environment;
};

class BadStartWlanSoftmacServer : public UnimplementedWlanSoftmacServer {
 public:
  using UnimplementedWlanSoftmacServer::UnimplementedWlanSoftmacServer;

  void Start(StartRequest& request, StartCompleter::Sync& completer) final {
    completer.Reply(fit::error(ZX_ERR_ADDRESS_UNREACHABLE));
  }
};

class BadStartSoftmacDriverTest : public ::testing::Test {
 public:
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig<CustomEnvironment<BadStartWlanSoftmacServer>>>&
  driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig<CustomEnvironment<BadStartWlanSoftmacServer>>>
      driver_test_;
};

// Verify that a WlanSoftmac.Start failure during Start causes Start to fail.
TEST_F(BadStartSoftmacDriverTest, StartFails) {
  auto result = driver_test().StartDriver();
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_ADDRESS_UNREACHABLE);
}

class BadQueryWlanSoftmacServer : public BasicWlanSoftmacServer {
 public:
  using BasicWlanSoftmacServer::BasicWlanSoftmacServer;

  void Query(QueryCompleter::Sync& completer) override {
    completer.Reply(fit::error(ZX_ERR_ACCESS_DENIED));
  }
};

class BadQuerySoftmacDriverTest : public ::testing::Test {
 public:
  fdf_testing::BackgroundDriverTest<FixtureConfig<CustomEnvironment<BadQueryWlanSoftmacServer>>>&
  driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig<CustomEnvironment<BadQueryWlanSoftmacServer>>>
      driver_test_;
};

// Verify that a WlanSoftmac.Query failure during Start causes Start to fail.
TEST_F(BadQuerySoftmacDriverTest, StartFails) {
  auto result = driver_test().StartDriver();
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_ACCESS_DENIED);
}

class BadBootstrapWlanSoftmacServer : public BasicWlanSoftmacServer {
 public:
  using BasicWlanSoftmacServer::BasicWlanSoftmacServer;

  void Start(StartRequest& request, StartCompleter::Sync& completer) override {
    // Acquire/Construct the WlanSoftmacIfc, UsmeBootstrap, and GenericSme endpoints.
    softmac_ifc_client_endpoint_ = std::move(request.ifc());
    auto usme_bootstrap_endpoints = fidl::CreateEndpoints<fuchsia_wlan_sme::UsmeBootstrap>();
    auto generic_sme_endpoints = fidl::CreateEndpoints<fuchsia_wlan_sme::GenericSme>();
    generic_sme_client_endpoint_ = std::move(generic_sme_endpoints.value().client);

    // Drop UsmeBootstrap client endpoint without calling UsmeBootstrap.Start.

    completer.Reply(fit::ok(fuchsia_wlan_softmac::WlanSoftmacStartResponse(
        usme_bootstrap_endpoints.value().server.TakeChannel())));
  }

 private:
  std::optional<fdf::ClientEnd<::fuchsia_wlan_softmac::WlanSoftmacIfc>>
      softmac_ifc_client_endpoint_;
  std::optional<fidl::ClientEnd<fuchsia_wlan_sme::GenericSme>> generic_sme_client_endpoint_;
  std::optional<zx::vmo> inspect_vmo_;
};

class BadBootstrapSoftmacDriverTest : public ::testing::Test {
 public:
  fdf_testing::BackgroundDriverTest<
      FixtureConfig<CustomEnvironment<BadBootstrapWlanSoftmacServer>>>&
  driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig<CustomEnvironment<BadBootstrapWlanSoftmacServer>>>
      driver_test_;
};

// Verify that a failure to bootstrap to GenericSme during Start causes Start to fail.
TEST_F(BadBootstrapSoftmacDriverTest, StartFails) {
  auto result = driver_test().StartDriver();
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_BAD_STATE);
}

class FragileSoftmacDriverEnvironment : public CustomEnvironment<BasicWlanSoftmacServer> {
 public:
  using CustomEnvironment<BasicWlanSoftmacServer>::CustomEnvironment;

  void DropWlanSoftmacIfcClient() { this->GetServer().DropWlanSoftmacIfcClient(); }

  void DropGenericSmeClient() { this->GetServer().DropGenericSmeClient(); }
};

class FragileSoftmacDriverTest : public ::testing::Test {
 public:
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig<FragileSoftmacDriverEnvironment>>& driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig<FragileSoftmacDriverEnvironment>> driver_test_;
};

// Verify dropping the WlanSoftmacIfc client end causes wlansoftmac to exit.
TEST_F(FragileSoftmacDriverTest, WlanSoftmacIfcClientDropped) {
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  driver_test().RunInNodeContext([](auto& node) { ASSERT_EQ(node.children().size(), 1ul); });

  driver_test().RunInEnvironmentTypeContext(
      [](auto& environment) { environment.DropWlanSoftmacIfcClient(); });

  while (driver_test().RunInNodeContext<size_t>([](auto& node) { return node.children().size(); }) >
         0) {
    std::this_thread::sleep_for(std::chrono::milliseconds(5));
  }
  driver_test().RunInNodeContext([](auto& node) { ASSERT_EQ(node.children().size(), 0ul); });
}

// Verify dropping the GenericSme client end causes wlansoftmac to exit.
TEST_F(FragileSoftmacDriverTest, GenericSmeClientDropped) {
  ASSERT_TRUE(driver_test().StartDriver().is_ok());

  driver_test().RunInNodeContext([](auto& node) { ASSERT_EQ(node.children().size(), 1ul); });

  driver_test().RunInEnvironmentTypeContext(
      [](auto& environment) { environment.DropGenericSmeClient(); });

  while (driver_test().RunInNodeContext<size_t>([](auto& node) { return node.children().size(); }) >
         0) {
    std::this_thread::sleep_for(std::chrono::milliseconds(5));
  }
  driver_test().RunInNodeContext([](auto& node) { ASSERT_EQ(node.children().size(), 0ul); });
}

}  // namespace
}  // namespace wlan::drivers::wlansoftmac
