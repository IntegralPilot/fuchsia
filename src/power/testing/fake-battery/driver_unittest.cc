// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "driver.h"

#include <fidl/fuchsia.hardware.power.battery/cpp/fidl.h>
#include <fidl/fuchsia.hardware.power.battery/cpp/wire.h>
#include <fidl/fuchsia.power.battery/cpp/wire.h>
#include <fidl/test.hardwarepowercontrol/cpp/fidl.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>
#include <sdk/lib/syslog/cpp/macros.h>

#include "src/lib/testing/predicates/status.h"

namespace fake_battery::testing {

namespace fbattery = fuchsia_power_battery;
namespace hbattery = fuchsia_hardware_power_battery;

class FakeBatteryDriverTestEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override { return zx::ok(); }
};

class FixtureConfig final {
 public:
  using DriverType = Driver;
  using EnvironmentType = FakeBatteryDriverTestEnvironment;
};

class FakeBatteryDriverTest : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(driver_test_.StartDriver());

    // Connect to old protocol
    zx::result connect_old = driver_test().Connect<fbattery::InfoService::Device>();
    ASSERT_EQ(ZX_OK, connect_old.status_value());
    battery_info_provider_ = std::move(connect_old.value());

    // Connect to new protocol
    zx::result connect_new = driver_test().Connect<hbattery::Service::Battery>();
    ASSERT_EQ(ZX_OK, connect_new.status_value());
    hardware_battery_ = std::move(connect_new.value());
  }

  void TearDown() override {
    ASSERT_OK(driver_test_.StopDriver());
    driver_test().ShutdownAndDestroyDriver();
  }

 protected:
  fidl::ClientEnd<fbattery::BatteryInfoProvider>& GetBatteryInfoProviderClient() {
    return battery_info_provider_;
  }
  fidl::ClientEnd<hbattery::Battery>& GetHardwareBatteryClient() { return hardware_battery_; }
  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fidl::ClientEnd<fbattery::BatteryInfoProvider> battery_info_provider_;
  fidl::ClientEnd<hbattery::Battery> hardware_battery_;
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(FakeBatteryDriverTest, CanGetBatteryInfoLegacy) {
  auto result = fidl::WireCall(GetBatteryInfoProviderClient())->GetBatteryInfo();
  ASSERT_EQ(result.status(), ZX_OK);
  const auto& info = result.value().info;
  ASSERT_EQ(info.status(), fuchsia_power_battery::BatteryStatus::kOk);
  ASSERT_EQ(info.time_remaining().Which(),
            fuchsia_power_battery::wire::TimeRemaining::Tag::kFullCharge);
  ASSERT_EQ(info.time_remaining().full_charge(), zx::sec(59).to_nsecs());
}

TEST_F(FakeBatteryDriverTest, CanGetStatus) {
  auto result = fidl::WireCall(GetHardwareBatteryClient())->GetStatus();
  ASSERT_EQ(result.status(), ZX_OK);
  ASSERT_TRUE(result->is_ok());
  const auto& status = result->value()->status;
  ASSERT_TRUE(status.has_charge_status());
  ASSERT_EQ(status.charge_status(), hbattery::ChargeStatus::kCharging);
  ASSERT_EQ(status.time_remaining(), zx::sec(59).to_nsecs());

  // Verify presence, voltage, current, and health
  ASSERT_TRUE(status.has_present());
  ASSERT_TRUE(status.present());
  ASSERT_TRUE(status.has_voltage_uv());
  ASSERT_EQ(status.voltage_uv(), test_hardwarepowercontrol::kDefaultPresentVoltageMv * 1000);
  ASSERT_TRUE(status.has_current_ua());
  ASSERT_EQ(status.current_ua(), test_hardwarepowercontrol::kDefaultChargingCurrentUa);
  ASSERT_TRUE(status.has_health());
  ASSERT_EQ(status.health(), hbattery::HealthStatus::kGood);
}

TEST_F(FakeBatteryDriverTest, CanGetSpec) {
  auto result = fidl::WireCall(GetHardwareBatteryClient())->GetSpec();
  ASSERT_EQ(result.status(), ZX_OK);
  ASSERT_TRUE(result->is_ok());
  const auto& spec = result->value()->spec;
  ASSERT_TRUE(spec.has_design_capacity_uah());
  ASSERT_EQ(spec.design_capacity_uah(), test_hardwarepowercontrol::kDefaultFullCapacityUah);
  ASSERT_TRUE(spec.has_supported_options());
  ASSERT_TRUE(spec.supported_options().has_interest());
  ASSERT_TRUE(spec.supported_options().interest().has_level_percent());
  ASSERT_TRUE(spec.supported_options().interest().has_charge_status());
}

TEST_F(FakeBatteryDriverTest, CanConfigureWatch) {
  fidl::SyncClient client(std::move(GetHardwareBatteryClient()));
  fuchsia_hardware_power_battery::Status interest;
  interest.level_percent(0.0f);
  interest.charge_status(fuchsia_hardware_power_battery::ChargeStatus::kCharging);

  fuchsia_hardware_power_battery::WatchOptions options;
  options.interest(std::move(interest));

  auto result = client->ConfigureWatch({std::move(options)});
  ASSERT_TRUE(result.is_ok());
  const auto& effective_options = result->effective_options();
  ASSERT_TRUE(effective_options.interest().has_value());
  ASSERT_TRUE(effective_options.interest()->level_percent().has_value());
  ASSERT_TRUE(effective_options.interest()->charge_status().has_value());

  // Restore client end
  GetHardwareBatteryClient() = client.TakeClientEnd();
}

TEST_F(FakeBatteryDriverTest, ConfigureWatchUnsupportedReturnsError) {
  fidl::SyncClient client(std::move(GetHardwareBatteryClient()));
  fuchsia_hardware_power_battery::Status interest;
  interest.voltage_uv(4200000);
  interest.current_ua(1000000);

  fuchsia_hardware_power_battery::WatchOptions options;
  options.interest(std::move(interest));

  auto result = client->ConfigureWatch({std::move(options)});
  ASSERT_TRUE(result.is_error());
  ASSERT_TRUE(result.error_value().is_domain_error());
  ASSERT_EQ(result.error_value().domain_error(),
            fuchsia_hardware_power_battery::Error::kNotSupported);

  // Restore client end
  GetHardwareBatteryClient() = client.TakeClientEnd();
}

TEST_F(FakeBatteryDriverTest, ConfigureWatchFiltersUnsupportedFields) {
  fidl::SyncClient client(std::move(GetHardwareBatteryClient()));
  fuchsia_hardware_power_battery::Status interest;
  interest.voltage_uv(4200000);
  interest.level_percent(0.0f);

  fuchsia_hardware_power_battery::WatchOptions options;
  options.interest(std::move(interest));

  auto result = client->ConfigureWatch({std::move(options)});
  ASSERT_TRUE(result.is_ok());
  const auto& effective_options = result->effective_options();
  ASSERT_TRUE(effective_options.interest().has_value());
  ASSERT_TRUE(effective_options.interest()->level_percent().has_value());
  ASSERT_FALSE(effective_options.interest()->voltage_uv().has_value());

  // Restore client end
  GetHardwareBatteryClient() = client.TakeClientEnd();
}

TEST_F(FakeBatteryDriverTest, CanWatch) {
  fidl::SyncClient client(std::move(GetHardwareBatteryClient()));
  auto result = client->Watch({});
  ASSERT_TRUE(result.is_ok());
  const auto& status = result->status();
  ASSERT_TRUE(status.level_percent().has_value());
  ASSERT_EQ(status.level_percent().value(), 98.7f);

  // Restore client end
  GetHardwareBatteryClient() = client.TakeClientEnd();
}

TEST_F(FakeBatteryDriverTest, WatchHangingUntilSetBatteryStatus) {
  fidl::SyncClient sync_client(std::move(GetHardwareBatteryClient()));
  // First watch resolves immediately with initial status.
  auto first_res = sync_client->Watch({});
  ASSERT_TRUE(first_res.is_ok());

  // Second watch will hang because state has not changed.
  bool watch_resolved = false;
  fuchsia_hardware_power_battery::ChargeStatus received_charge_status;
  float received_level_percent = 0.0f;

  fidl::Client async_client(sync_client.TakeClientEnd(),
                            driver_test().runtime().GetForegroundDispatcher()->async_dispatcher());

  async_client->Watch({}).Then(
      [&](fidl::Result<fuchsia_hardware_power_battery::Battery::Watch>& res) {
        ASSERT_TRUE(res.is_ok());
        const auto& status = res->status();
        if (status.level_percent().has_value()) {
          received_level_percent = status.level_percent().value();
        }
        if (status.charge_status().has_value()) {
          received_charge_status = status.charge_status().value();
        }
        watch_resolved = true;
      });

  driver_test().runtime().RunUntilIdle();
  EXPECT_FALSE(watch_resolved);

  // Trigger state update via Control protocol.
  zx::result connect_ctrl = driver_test().Connect<test_hardwarepowercontrol::Service::Control>();
  ASSERT_EQ(ZX_OK, connect_ctrl.status_value());

  fidl::SyncClient ctrl_client(std::move(connect_ctrl.value()));
  fuchsia_hardware_power_battery::Status new_status;
  new_status.level_percent(42.0f);
  new_status.charge_status(fuchsia_hardware_power_battery::ChargeStatus::kDischarging);
  auto set_res = ctrl_client->SetBatteryStatus({std::move(new_status)});
  ASSERT_TRUE(set_res.is_ok());

  driver_test().runtime().RunUntil([&]() { return watch_resolved; });
  EXPECT_TRUE(watch_resolved);
  EXPECT_EQ(received_level_percent, 42.0f);
  EXPECT_EQ(received_charge_status, fuchsia_hardware_power_battery::ChargeStatus::kDischarging);
}

TEST_F(FakeBatteryDriverTest, ConcurrentWatchReturnsAlreadyWatching) {
  fidl::SyncClient sync_client(std::move(GetHardwareBatteryClient()));
  // First watch resolves immediately with initial status.
  auto first_res = sync_client->Watch({});
  ASSERT_TRUE(first_res.is_ok());

  // Second watch will hang because state has not changed.
  fidl::Client async_client(sync_client.TakeClientEnd(),
                            driver_test().runtime().GetForegroundDispatcher()->async_dispatcher());

  bool first_watch_resolved = false;
  async_client->Watch({}).Then(
      [&](fidl::Result<fuchsia_hardware_power_battery::Battery::Watch>& res) {
        first_watch_resolved = true;
      });

  driver_test().runtime().RunUntilIdle();
  EXPECT_FALSE(first_watch_resolved);

  // Third watch on the same connection while the second is hanging must be rejected with
  // Error::kAlreadyWatching.
  bool second_watch_resolved = false;
  std::optional<fuchsia_hardware_power_battery::Error> second_watch_error;
  async_client->Watch({}).Then(
      [&](fidl::Result<fuchsia_hardware_power_battery::Battery::Watch>& res) {
        second_watch_resolved = true;
        ASSERT_TRUE(res.is_error());
        ASSERT_TRUE(res.error_value().is_domain_error());
        second_watch_error = res.error_value().domain_error();
      });

  driver_test().runtime().RunUntil([&]() { return second_watch_resolved; });
  EXPECT_TRUE(second_watch_resolved);
  EXPECT_EQ(second_watch_error, fuchsia_hardware_power_battery::Error::kAlreadyWatching);
  EXPECT_FALSE(first_watch_resolved);
}

TEST_F(FakeBatteryDriverTest, MultiClientWatchIndependent) {
  fidl::SyncClient client_a(std::move(GetHardwareBatteryClient()));
  // Client A first watch resolves immediately.
  auto res_a1 = client_a->Watch({});
  ASSERT_TRUE(res_a1.is_ok());

  // Client A second watch will hang.
  fidl::Client async_client_a(
      client_a.TakeClientEnd(),
      driver_test().runtime().GetForegroundDispatcher()->async_dispatcher());
  bool watch_a2_resolved = false;
  async_client_a->Watch({}).Then(
      [&](fidl::Result<fuchsia_hardware_power_battery::Battery::Watch>& res) {
        watch_a2_resolved = true;
      });
  driver_test().runtime().RunUntilIdle();
  EXPECT_FALSE(watch_a2_resolved);

  // Connect Client B independently.
  zx::result connect_b = driver_test().Connect<hbattery::Service::Battery>();
  ASSERT_EQ(ZX_OK, connect_b.status_value());
  fidl::SyncClient client_b(std::move(connect_b.value()));

  // Client B's first watch must resolve immediately and NOT be blocked or rejected by Client A.
  auto res_b1 = client_b->Watch({});
  ASSERT_TRUE(res_b1.is_ok());
  EXPECT_FALSE(watch_a2_resolved);
}

class ForegroundFakeBatteryDriverTest : public ::testing::Test {
 public:
  void SetUp() override {
    ASSERT_OK(driver_test_.StartDriver());
    zx::result connect_result = driver_test().Connect<fbattery::InfoService::Device>();
    EXPECT_EQ(ZX_OK, connect_result.status_value());
    battery_info_provider_ = std::move(connect_result.value());
  }

  void TearDown() override {
    ASSERT_OK(driver_test_.StopDriver());
    driver_test().ShutdownAndDestroyDriver();
  }

 protected:
  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }
  fidl::ClientEnd<fbattery::BatteryInfoProvider>& GetBatteryInfoProviderClient() {
    return battery_info_provider_;
  }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
  fidl::ClientEnd<fbattery::BatteryInfoProvider> battery_info_provider_;
};

// Picking ForegroundDriverTest to test Watch. Otherwise we have to use a control fidl to make sure
// the driver receives the reply for OnChangeBatteryInfo, or we have to ignore ZX_ERR_CANCELED.
TEST_F(ForegroundFakeBatteryDriverTest, CanWatchLegacy) {
  class FakeBatteryInfoWatcher : public fidl::Server<fbattery::BatteryInfoWatcher> {
   public:
    void Bind(fidl::ServerEnd<fbattery::BatteryInfoWatcher> server_end,
              fdf_testing::DriverRuntime* runtime) {
      bindings_.AddBinding(runtime->GetForegroundDispatcher()->async_dispatcher(),
                           std::move(server_end), this, fidl::kIgnoreBindingClosure);
      test_runtime_ = runtime;
    }

    void OnChangeBatteryInfo(OnChangeBatteryInfoRequest& request,
                             OnChangeBatteryInfoCompleter::Sync& completer) override {
      EXPECT_EQ(request.info().charge_status(), fbattery::ChargeStatus::kCharging);
      EXPECT_EQ(request.info().charge_source(), fbattery::ChargeSource::kAcAdapter);
      EXPECT_EQ(request.info().present_voltage_mv(), 4752);
      EXPECT_EQ(request.info().present_charging_current_ua(), 250014);
      EXPECT_EQ(request.info().health(), fbattery::HealthStatus::kGood);
      completer.Reply();
      EXPECT_TRUE(test_runtime_);
      completed_ = true;
    }

    bool Completed() const { return completed_; }

   private:
    fidl::ServerBindingGroup<fbattery::BatteryInfoWatcher> bindings_;
    fdf_testing::DriverRuntime* test_runtime_;
    bool completed_ = false;

  } fake_watcher;
  {
    auto& battery_info_provider = GetBatteryInfoProviderClient();
    auto [client_end, server_end] = fidl::Endpoints<fbattery::BatteryInfoWatcher>::Create();
    fake_watcher.Bind(std::move(server_end), &driver_test().runtime());
    auto result = fidl::WireCall(battery_info_provider)->Watch(std::move(client_end));
    ASSERT_TRUE(result.ok());

    // Wait for the driver to receive and handle Watch message.
    driver_test().runtime().RunUntilIdle();
    driver_test().runtime().RunUntil([&fake_watcher]() { return fake_watcher.Completed(); });
    // Wait for the driver to receive and handle the reply from the fake watcher.
    driver_test().runtime().RunUntilIdle();
  }
}

}  // namespace fake_battery::testing
