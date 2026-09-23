// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/test.wlan.fidlbridge/cpp/driver/fidl.h>
#include <fidl/test.wlan.fidlbridge/cpp/fidl.h>
#include <lib/async/cpp/task.h>
#include <lib/driver/testing/cpp/scoped_global_logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/fdf/env.h>
#include <lib/sync/cpp/completion.h>

#include <gtest/gtest.h>
#include <wlan/drivers/fidl_bridge.h>

namespace {
using wlan::drivers::fidl_bridge::ForwardResult;

// Test bridge server that forwards requests from a Zircon client to a Driver server.
class Bridge : public fidl::Server<test_wlan_fidlbridge::Zircon> {
 public:
  Bridge(fdf_dispatcher_t* dispatcher, fidl::ServerEnd<test_wlan_fidlbridge::Zircon> server_end,
         fdf::ClientEnd<test_wlan_fidlbridge::Driver> bridge_client_end)
      : binding_(fdf_dispatcher_get_async_dispatcher(dispatcher), std::move(server_end), this,
                 fidl::kIgnoreBindingClosure),
        bridge_client_(std::move(bridge_client_end), dispatcher) {}

  void NoDomainError(NoDomainErrorCompleter::Sync& completer) override {
    bridge_client_->NoDomainError().Then(
        ForwardResult<test_wlan_fidlbridge::Driver::NoDomainError>(completer.ToAsync()));
  }

  void WithDomainError(WithDomainErrorCompleter::Sync& completer) override {
    bridge_client_->WithDomainError().Then(
        ForwardResult<test_wlan_fidlbridge::Driver::WithDomainError>(completer.ToAsync()));
  }

 private:
  fidl::ServerBinding<test_wlan_fidlbridge::Zircon> binding_;
  fdf::Client<test_wlan_fidlbridge::Driver> bridge_client_;
};

// Taken from //sdk/lib/fidl_driver/tests/transport/scoped_fake_driver.h
// This is needed to create fdf Dispatchers without actually using a whole driver.
class ScopedFakeDriver {
 public:
  ScopedFakeDriver() {
    void* driver = reinterpret_cast<void*>(1);
    fdf_env_register_driver_entry(driver);
  }

  ~ScopedFakeDriver() { fdf_env_register_driver_exit(); }
};

class ForwardResultTest : public ::testing::Test {
 protected:
  void SetUp() override {
    auto dispatcher_result = fdf::SynchronizedDispatcher::Create(
        {}, "", [this](fdf_dispatcher_t* dispatcher) { dispatcher_shutdown_.Signal(); });
    ASSERT_TRUE(dispatcher_result.is_ok());
    dispatcher_ = std::move(dispatcher_result.value());

    // Setup bridge server, zircon client, and driver server end
    zx::result driver_endpoints = fdf::CreateEndpoints<test_wlan_fidlbridge::Driver>();
    ASSERT_TRUE(driver_endpoints.is_ok());

    zx::result zircon_endpoints = fidl::CreateEndpoints<test_wlan_fidlbridge::Zircon>();
    ASSERT_TRUE(zircon_endpoints.is_ok());

    libsync::Completion bridge_created;
    async::PostTask(dispatcher_.async_dispatcher(), [&]() {
      bridge_server_ =
          std::make_unique<Bridge>(dispatcher_.get(), std::move(zircon_endpoints->server),
                                   std::move(driver_endpoints->client));
      bridge_created.Signal();
    });
    bridge_created.Wait();

    zircon_client_.Bind(std::move(zircon_endpoints->client), dispatcher_.async_dispatcher());
    driver_server_end_ = std::move(driver_endpoints->server);
  }

  void TearDown() override {
    dispatcher_.ShutdownAsync();
    dispatcher_shutdown_.Wait();
  }

  fidl::SharedClient<test_wlan_fidlbridge::Zircon> zircon_client_;
  fdf::ServerEnd<test_wlan_fidlbridge::Driver> driver_server_end_;
  std::unique_ptr<Bridge> bridge_server_;

 private:
  ScopedFakeDriver fake_driver_;
  fdf_testing::ScopedGlobalLogger log_context;

  fdf::SynchronizedDispatcher dispatcher_;
  libsync::Completion dispatcher_shutdown_;
};

TEST_F(ForwardResultTest, ChannelClosurePropagatesNoDomainError) {
  libsync::Completion call_completion;
  zircon_client_->NoDomainError().ThenExactlyOnce(
      [&call_completion](fidl::Result<test_wlan_fidlbridge::Zircon::NoDomainError>& result) {
        EXPECT_TRUE(result.is_error());
        EXPECT_TRUE(result.error_value().is_peer_closed());
        call_completion.Signal();
      });

  // Drop the driver server end while call is in flight
  driver_server_end_.reset();

  call_completion.Wait();
}

TEST_F(ForwardResultTest, ChannelClosurePropagatesWithDomainError) {
  libsync::Completion call_completion;
  zircon_client_->WithDomainError().ThenExactlyOnce(
      [&call_completion](fidl::Result<test_wlan_fidlbridge::Zircon::WithDomainError>& result) {
        EXPECT_TRUE(result.is_error());
        EXPECT_TRUE(result.error_value().is_framework_error());
        EXPECT_TRUE(result.error_value().framework_error().is_peer_closed());
        call_completion.Signal();
      });

  // Drop the driver server end while call is in flight
  driver_server_end_.reset();

  call_completion.Wait();
}

}  // namespace
