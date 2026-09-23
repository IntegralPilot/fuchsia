// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/component/cpp/driver_export2.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/driver/testing/cpp/minimal_compat_environment.h>
#include <lib/magma_service/mock/mock_msd.h>
#include <lib/magma_service/sys_driver/magma_driver_base.h>
#include <lib/zx/result.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace msd {

class FakeTestDriver : public MagmaDriverBase {
 public:
  FakeTestDriver() : MagmaDriverBase("fake_test_driver") {}
  zx::result<> MagmaStart(fdf::DriverContext& context) override {
    std::lock_guard lock(magma_mutex());

    set_magma_driver(msd::Driver::MsdCreate());
    if (!magma_driver()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
    test_server_.set_unit_test_status(ZX_OK);
    zx::result result = CreateTestService(test_server_);
    if (result.is_error()) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    set_magma_system_device(
        MagmaSystemDevice::Create(magma_driver(), magma_driver()->MsdCreateDevice(nullptr)));
    if (!magma_system_device()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::ok();
  }

 private:
  msd::MagmaTestServer test_server_;
};

namespace {

class FakeDriver : public MagmaDriverBase {
 public:
  FakeDriver() : MagmaDriverBase("fake_driver") {}
  zx::result<> MagmaStart(fdf::DriverContext& context) override {
    std::lock_guard lock(magma_mutex());

    set_magma_driver(msd::Driver::MsdCreate());
    if (!magma_driver()) {
      return zx::error(ZX_ERR_INTERNAL);
    }

    set_magma_system_device(
        MagmaSystemDevice::Create(magma_driver(), magma_driver()->MsdCreateDevice(nullptr)));
    if (!magma_system_device()) {
      return zx::error(ZX_ERR_INTERNAL);
    }
    return zx::ok();
  }
};

// Instantiating drivers involves a synchronous connection to create a logger, so we must serve that
// as a minimum.
void SetUpIncomingDirectory(fdf::OutgoingDirectory& dir,
                            fidl::ServerEnd<fuchsia_io::Directory> server_end) {
  EXPECT_EQ(
      dir.component()
          .AddUnmanagedProtocol<fuchsia_logger::LogSink>(
              [](fidl::ServerEnd<fuchsia_logger::LogSink> server_end) {
                ZX_ASSERT(
                    component::Connect<fuchsia_logger::LogSink>(std::move(server_end)).is_ok());
              })
          .status_value(),
      ZX_OK);
  EXPECT_EQ(dir.Serve(std::move(server_end)).status_value(), ZX_OK);
}

// Check that the test driver class can be instantiated (not started).
TEST(MagmaDriver, CreateTestDriver) {
  fdf_testing::DriverRuntime runtime;
  fdf_testing::TestNode node_server("root");
  zx::result start_args = node_server.CreateStartArgsAndServe();
  EXPECT_EQ(start_args.status_value(), ZX_OK);
  async_patterns::TestDispatcherBound<fdf::OutgoingDirectory> env(
      runtime.StartBackgroundDispatcher()->async_dispatcher());
  env.emplace();
  env.SyncCall([&](fdf::OutgoingDirectory* env) {
    SetUpIncomingDirectory(*env, std::move(start_args->incoming_directory_server));
  });
  FakeTestDriver driver{};
}

// Check that the driver class can be instantiated (not started).
TEST(MagmaDriver, CreateDriver) {
  fdf_testing::DriverRuntime runtime;
  fdf_testing::TestNode node_server("root");
  zx::result start_args = node_server.CreateStartArgsAndServe();
  EXPECT_EQ(start_args.status_value(), ZX_OK);
  async_patterns::TestDispatcherBound<fdf::OutgoingDirectory> env(
      runtime.StartBackgroundDispatcher()->async_dispatcher());
  env.emplace();
  env.SyncCall([&](fdf::OutgoingDirectory* env) {
    SetUpIncomingDirectory(*env, std::move(start_args->incoming_directory_server));
  });
  FakeDriver driver{};
}

class FixtureConfig final {
 public:
  using DriverType = FakeTestDriver;
  using EnvironmentType = fdf_testing::MinimalCompatEnvironment;
};

class MagmaDriverStarted : public testing::Test {
 public:
  void SetUp() override { ASSERT_OK(driver_test_.StartDriver()); }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(MagmaDriverStarted, TestDriver) {}

TEST_F(MagmaDriverStarted, Query) {
  zx::result client_end = driver_test().Connect<fuchsia_gpu_magma::Service::Device>();
  ASSERT_OK(client_end);
  fidl::WireSyncClient client{std::move(client_end.value())};

  auto result = client->Query(fuchsia_gpu_magma::wire::QueryId::kDeviceId);
  ASSERT_EQ(ZX_OK, result.status());
  ASSERT_TRUE(result->is_ok()) << result->error_value();
  ASSERT_TRUE(result->value()->is_simple_result());
  EXPECT_EQ(0u, result->value()->simple_result());
}

TEST_F(MagmaDriverStarted, PerformanceCounters) {
  zx::result client_end =
      driver_test().Connect<fuchsia_gpu_magma::PerformanceCounterService::Access>();
  ASSERT_OK(client_end);
  fidl::WireSyncClient<fuchsia_gpu_magma::PerformanceCounterAccess> client{
      std::move(client_end.value())};

  auto result = client->GetPerformanceCountToken();

  ASSERT_EQ(ZX_OK, result.status());

  zx_info_handle_basic_t handle_info{};
  ASSERT_EQ(result->access_token.get_info(ZX_INFO_HANDLE_BASIC, &handle_info, sizeof(handle_info),
                                          nullptr, nullptr),
            ZX_OK);
  EXPECT_EQ(ZX_OBJ_TYPE_EVENT, handle_info.type);
}

class MemoryPressureProviderServer : public fidl::WireServer<fuchsia_memorypressure::Provider> {
 public:
  void RegisterWatcher(fuchsia_memorypressure::wire::ProviderRegisterWatcherRequest* request,
                       RegisterWatcherCompleter::Sync& completer) override {
    auto client = fidl::WireSyncClient(std::move(request->watcher));
    EXPECT_EQ(ZX_OK,
              client->OnLevelChanged(fuchsia_memorypressure::wire::Level::kWarning).status());
  }
};

TEST_F(MagmaDriverStarted, DependencyInjection) {
  zx::result client_end =
      driver_test().Connect<fuchsia_gpu_magma::DependencyInjectionService::Device>();
  ASSERT_OK(client_end);
  fidl::WireSyncClient<fuchsia_gpu_magma::DependencyInjection> client{
      std::move(client_end.value())};

  auto memory_pressure_endpoints = fidl::Endpoints<fuchsia_memorypressure::Provider>::Create();

  auto result = client->SetMemoryPressureProvider(std::move(memory_pressure_endpoints.client));
  ASSERT_EQ(ZX_OK, result.status());

  driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    auto server = std::make_unique<MemoryPressureProviderServer>();
    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();
    fidl::BindServer(dispatcher, std::move(memory_pressure_endpoints.server), std::move(server));
  });

  MsdMockDevice* mock_device;
  driver_test().RunInDriverContext([&mock_device](auto& driver) mutable {
    std::lock_guard magma_lock(driver.magma_mutex());
    mock_device = static_cast<MsdMockDevice*>(driver.magma_system_device()->msd_dev());
  });
  mock_device->WaitForMemoryPressureSignal();
  EXPECT_EQ(msd::MAGMA_MEMORY_PRESSURE_LEVEL_WARNING, mock_device->memory_pressure_level());
}

}  // namespace

}  // namespace msd

// Export the |FakeTestDriver| for the test to use.
FUCHSIA_DRIVER_EXPORT2(msd::FakeTestDriver);
