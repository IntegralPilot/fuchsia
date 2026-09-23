// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "thermal-cli.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/driver/runtime/testing/cpp/sync_helpers.h>
#include <lib/mock-function/mock-function.h>

#include <optional>

#include <zxtest/zxtest.h>

namespace {

class FakeThermalDevice : public fidl::WireServer<fuchsia_hardware_thermal::Device> {
 public:
  FakeThermalDevice() = default;

  zx_status_t temperature_status = ZX_OK;
  int64_t fan_level = -1;
  int32_t cluster_operating_point[2]{-1, -1};

 private:
  void GetInfo(GetInfoCompleter::Sync& completer) override {}
  void GetDeviceInfo(GetDeviceInfoCompleter::Sync& completer) override {}

  void GetDvfsInfo(GetDvfsInfoRequestView request, GetDvfsInfoCompleter::Sync& completer) override {
    fuchsia_hardware_thermal::wire::OperatingPointEntry entry0{
        .freq_hz = 100,
        .volt_uv = 42,
    };
    fuchsia_hardware_thermal::wire::OperatingPointEntry entry1{
        .freq_hz = 200,
        .volt_uv = 42,
    };
    fuchsia_hardware_thermal::wire::OperatingPoint op_info{
        .opp = {entry0, entry1},
        .latency = 42,
        .count = 2,
    };
    completer.Reply(ZX_OK, fidl::ObjectView<decltype(op_info)>::FromExternal(&op_info));
  }

  void GetTemperatureCelsius(GetTemperatureCelsiusCompleter::Sync& completer) override {
    completer.Reply(temperature_status, 0);
  }

  void GetSensorName(GetSensorNameCompleter::Sync& completer) override {
    constexpr char kSensorName[] = "thermal sensor";
    completer.Reply(fidl::StringView::FromExternal(kSensorName));
  }

  void GetStateChangeEvent(GetStateChangeEventCompleter::Sync& completer) override {}
  void GetStateChangePort(GetStateChangePortCompleter::Sync& completer) override {}
  void SetTripCelsius(SetTripCelsiusRequestView request,
                      SetTripCelsiusCompleter::Sync& completer) override {}

  void GetDvfsOperatingPoint(GetDvfsOperatingPointRequestView request,
                             GetDvfsOperatingPointCompleter::Sync& completer) override {
    completer.Reply(ZX_OK, 1);
  }

  void SetDvfsOperatingPoint(SetDvfsOperatingPointRequestView request,
                             SetDvfsOperatingPointCompleter::Sync& completer) override {
    const auto cluster = static_cast<uint32_t>(request->power_domain);
    cluster_operating_point[cluster] = request->op_idx;
    completer.Reply(ZX_OK);
  }

  void GetFanLevel(GetFanLevelCompleter::Sync& completer) override { completer.Reply(ZX_OK, 0); }

  void SetFanLevel(SetFanLevelRequestView request, SetFanLevelCompleter::Sync& completer) override {
    fan_level = request->fan_level;
    completer.Reply(ZX_OK);
  }
};

class ThermalCliTest : public zxtest::Test {
 public:
  void SetUp() override {
    loop_.StartThread("thermal-cli-test-loop");

    ASSERT_OK(fdf::RunOnDispatcherSync(loop_.dispatcher(), [this]() {
      zx::result server = fidl::CreateEndpoints(&client_);
      ASSERT_OK(server);

      device_.emplace();
      fidl::BindServer(loop_.dispatcher(), std::move(server.value()), &*device_);
    }));
  }

  void TearDown() override {
    ASSERT_OK(fdf::RunOnDispatcherSync(loop_.dispatcher(), [this]() { device_.reset(); }));
  }

 protected:
  fidl::ClientEnd<fuchsia_hardware_thermal::Device> client_;
  async::Loop loop_{&kAsyncLoopConfigAttachToCurrentThread};
  std::optional<FakeThermalDevice> device_;
};

TEST_F(ThermalCliTest, Temperature) {
  ThermalCli thermal_cli(std::move(client_));
  EXPECT_OK(thermal_cli.PrintTemperature());
}

TEST_F(ThermalCliTest, TemperatureFails) {
  ThermalCli thermal_cli(std::move(client_));
  device_->temperature_status = ZX_ERR_IO;
  EXPECT_EQ(thermal_cli.PrintTemperature(), ZX_ERR_IO);
}

TEST_F(ThermalCliTest, SensorName) {
  ThermalCli thermal_cli(std::move(client_));
  const zx::result name = thermal_cli.GetSensorName();
  ASSERT_TRUE(name.is_ok());
  EXPECT_STREQ(*name, "thermal sensor");
}

TEST_F(ThermalCliTest, GetFanLevel) {
  ThermalCli thermal_cli(std::move(client_));
  EXPECT_OK(thermal_cli.FanLevelCommand(nullptr));
}

TEST_F(ThermalCliTest, SetFanLevel) {
  ThermalCli thermal_cli(std::move(client_));

  EXPECT_OK(thermal_cli.FanLevelCommand("42"));
  EXPECT_EQ(device_->fan_level, 42);
}

TEST_F(ThermalCliTest, InvalidFanLevel) {
  ThermalCli thermal_cli(std::move(client_));

  EXPECT_EQ(thermal_cli.FanLevelCommand("123abcd"), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(thermal_cli.FanLevelCommand("-1"), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(thermal_cli.FanLevelCommand("4294967295"), ZX_ERR_INVALID_ARGS);

  EXPECT_EQ(device_->fan_level, -1);
}

TEST_F(ThermalCliTest, GetOperatingPoint) {
  ThermalCli thermal_cli(std::move(client_));

  EXPECT_OK(thermal_cli.FrequencyCommand(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, nullptr));
}

TEST_F(ThermalCliTest, SetOperatingPoint) {
  ThermalCli thermal_cli(std::move(client_));

  EXPECT_OK(thermal_cli.FrequencyCommand(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, "200"));

  EXPECT_EQ(device_->cluster_operating_point[0], 1);
  EXPECT_EQ(device_->cluster_operating_point[1], -1);
}

TEST_F(ThermalCliTest, FrequencyNotFound) {
  ThermalCli thermal_cli(std::move(client_));

  EXPECT_EQ(thermal_cli.FrequencyCommand(
                fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, "300"),
            ZX_ERR_NOT_FOUND);

  EXPECT_EQ(device_->cluster_operating_point[0], -1);
  EXPECT_EQ(device_->cluster_operating_point[1], -1);
}

TEST_F(ThermalCliTest, InvalidFrequency) {
  ThermalCli thermal_cli(std::move(client_));

  EXPECT_EQ(thermal_cli.FrequencyCommand(
                fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, "123abcd"),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(thermal_cli.FrequencyCommand(
                fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, "-1"),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(thermal_cli.FrequencyCommand(
                fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, "4294967295"),
            ZX_ERR_INVALID_ARGS);

  EXPECT_EQ(device_->cluster_operating_point[0], -1);
  EXPECT_EQ(device_->cluster_operating_point[1], -1);
}

}  // namespace
