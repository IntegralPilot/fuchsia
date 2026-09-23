// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "ti-ina231.h"

#include <fidl/fuchsia.hardware.ti.metadata/cpp/fidl.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fake-i2c/fake-i2c.h>

#include <string_view>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace power_sensor {

class FakeI2c : public fake_i2c::FakeI2c {
 public:
  FakeI2c() {
    // Set bits 15 and 14. Bit 15 (reset) should be masked off, while 14 should be preserved.
    registers_[0] = 0xc000;
  }

  uint16_t configuration() const { return registers_[0]; }
  uint16_t calibration() const { return registers_[5]; }
  uint16_t mask_enable() const { return registers_[6]; }
  uint16_t alert_limit() const { return registers_[7]; }

  void set_bus_voltage(uint16_t voltage) { registers_[2] = voltage; }
  void set_power(uint16_t power) { registers_[3] = power; }

 protected:
  zx_status_t Transact(const uint8_t* write_buffer, size_t write_buffer_size, uint8_t* read_buffer,
                       size_t* read_buffer_size, size_t expected_read_size) override {
    if (write_buffer_size < 1 || write_buffer[0] >= std::size(registers_)) {
      return ZX_ERR_IO;
    }

    if (write_buffer_size == 1) {
      read_buffer[0] = registers_[write_buffer[0]] >> 8;
      read_buffer[1] = registers_[write_buffer[0]] & 0xff;
      *read_buffer_size = 2;
    } else if (write_buffer_size == 3) {
      if (write_buffer[0] >= 1 && write_buffer[0] <= 4) {
        // Write-only registers.
        return ZX_ERR_IO;
      }

      registers_[write_buffer[0]] = static_cast<uint16_t>((write_buffer[1] << 8) | write_buffer[2]);
    }

    return ZX_OK;
  }

 private:
  uint16_t registers_[8] = {};
};

class TiIna231TestEnvironment : public fdf_testing::Environment {
 public:
  void Init(const fuchsia_hardware_ti_metadata::Ina231Metadata& metadata) {
    pdev_.AddFidlMetadata(fuchsia_hardware_ti_metadata::Ina231Metadata::kSerializableName,
                          metadata);
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    EXPECT_OK(to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_.GetInstanceHandler(dispatcher), "pdev"));

    EXPECT_OK(to_driver_vfs.AddService<fuchsia_hardware_i2c::Service>(
        i2c_.CreateInstanceHandler(dispatcher), "i2c"));

    return zx::ok();
  }

  FakeI2c& i2c() { return i2c_; }

 private:
  fdf_fake::FakePDev pdev_;
  FakeI2c i2c_;
};

class FixtureConfig final {
 public:
  using DriverType = TiIna231;
  using EnvironmentType = TiIna231TestEnvironment;
};

class TiIna231Test : public ::testing::Test {
 public:
  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  void StartDriver(const fuchsia_hardware_ti_metadata::Ina231Metadata& metadata) {
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) { env.Init(metadata); });
    ASSERT_OK(driver_test_.StartDriver().status_value());

    zx::result power_sensor =
        driver_test_.Connect<fuchsia_hardware_power_sensor::Service::Device>();
    ASSERT_OK(power_sensor);
    power_sensor_.Bind(std::move(power_sensor.value()));

    // Verify that the power-sensor protocol is also connectable via devfs.
    EXPECT_OK(driver_test_.ConnectThroughDevfs<fuchsia_hardware_power_sensor::Device>(
        TiIna231::kChildNodeName));
  }

  void SetPower(uint16_t power) {
    driver_test_.RunInEnvironmentTypeContext([&](auto& env) { env.i2c().set_power(power); });
  }

  void SetBusVoltage(uint16_t voltage) {
    driver_test_.RunInEnvironmentTypeContext(
        [&](auto& env) { env.i2c().set_bus_voltage(voltage); });
  }

  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }
  fidl::WireSyncClient<fuchsia_hardware_power_sensor::Device>& power_sensor() {
    return power_sensor_;
  }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
  fidl::WireSyncClient<fuchsia_hardware_power_sensor::Device> power_sensor_;
};

TEST_F(TiIna231Test, GetPowerWatts) {
  static const fuchsia_hardware_ti_metadata::Ina231Metadata kMetadata({
      .mode = fuchsia_hardware_ti_metadata::Mode::kShuntAndBusContinuous,
      .shunt_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .bus_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .averages = fuchsia_hardware_ti_metadata::Averages::kAverages1024,
      .shunt_resistance_microohm = 10'000,
      .alert = fuchsia_hardware_ti_metadata::Alert::kNone,
      .power_sensor_domain = 0,
  });

  StartDriver(kMetadata);

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    FakeI2c& i2c = env.i2c();
    EXPECT_EQ(i2c.configuration(), 0x4e97);
    EXPECT_EQ(i2c.calibration(), 2048);
    EXPECT_EQ(i2c.mask_enable(), 0);
  });

  {
    SetPower(4792);
    auto response = power_sensor()->GetPowerWatts();
    ASSERT_TRUE(response.ok());
    ASSERT_FALSE(response->is_error());
    EXPECT_NEAR(response->value()->power, 29.95f, 0.001);
  }

  {
    SetPower(0);
    auto response = power_sensor()->GetPowerWatts();
    ASSERT_TRUE(response.ok());
    ASSERT_FALSE(response->is_error());
    EXPECT_NEAR(response->value()->power, 0.0f, 0.001);
  }

  {
    SetPower(65535);
    auto response = power_sensor()->GetPowerWatts();
    ASSERT_TRUE(response.ok());
    ASSERT_FALSE(response->is_error());
    EXPECT_NEAR(response->value()->power, 409.59375f, 0.001);
  }
}

TEST_F(TiIna231Test, SetAlertLimit) {
  static const fuchsia_hardware_ti_metadata::Ina231Metadata kMetadata({
      .mode = fuchsia_hardware_ti_metadata::Mode::kShuntAndBusContinuous,
      .shunt_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .bus_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .averages = fuchsia_hardware_ti_metadata::Averages::kAverages1024,
      .shunt_resistance_microohm = 10'000,
      .bus_voltage_limit_microvolt = 11'000'000,
      .alert = fuchsia_hardware_ti_metadata::Alert::kBusUnderVoltage,
      .power_sensor_domain = 0,
  });

  StartDriver(kMetadata);

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    FakeI2c& i2c = env.i2c();
    EXPECT_EQ(i2c.configuration(), 0x4e97);
    EXPECT_EQ(i2c.calibration(), 2048);
    EXPECT_EQ(i2c.mask_enable(), 0x1000);
    EXPECT_EQ(i2c.alert_limit(), 0x2260);
  });
}

TEST_F(TiIna231Test, GetVoltageVolts) {
  static const fuchsia_hardware_ti_metadata::Ina231Metadata kMetadata({
      .mode = fuchsia_hardware_ti_metadata::Mode::kShuntAndBusContinuous,
      .shunt_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .bus_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .averages = fuchsia_hardware_ti_metadata::Averages::kAverages1024,
      .shunt_resistance_microohm = 10'000,
      .alert = fuchsia_hardware_ti_metadata::Alert::kNone,
      .power_sensor_domain = 0,
  });

  StartDriver(kMetadata);

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    FakeI2c& i2c = env.i2c();
    EXPECT_EQ(i2c.configuration(), 0x4e97);
    EXPECT_EQ(i2c.calibration(), 2048);
    EXPECT_EQ(i2c.mask_enable(), 0);
  });

  {
    SetBusVoltage(9200);
    auto response = power_sensor()->GetVoltageVolts();
    ASSERT_TRUE(response.ok());
    ASSERT_FALSE(response->is_error());
    EXPECT_NEAR(response->value()->voltage, 11.5f, 0.001);
  }

  {
    SetBusVoltage(0);
    auto response = power_sensor()->GetVoltageVolts();
    ASSERT_TRUE(response.ok());
    ASSERT_FALSE(response->is_error());
    EXPECT_NEAR(response->value()->voltage, 0.0f, 0.001);
  }

  {
    SetBusVoltage(65535);
    auto response = power_sensor()->GetVoltageVolts();
    ASSERT_TRUE(response.ok());
    ASSERT_FALSE(response->is_error());
    EXPECT_NEAR(response->value()->voltage, 81.91875f, 0.001);
  }
}

TEST_F(TiIna231Test, GetSensorName) {
  static constexpr std::string_view kSensorName = "sensor name";

  static const fuchsia_hardware_ti_metadata::Ina231Metadata kMetadata({
      .mode = fuchsia_hardware_ti_metadata::Mode::kShuntAndBusContinuous,
      .shunt_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .bus_voltage_conversion_time =
          fuchsia_hardware_ti_metadata::ConversionTime::kConversionTime332Us,
      .averages = fuchsia_hardware_ti_metadata::Averages::kAverages1024,
      .shunt_resistance_microohm = 10'000,
      .alert = fuchsia_hardware_ti_metadata::Alert::kNone,
      .power_sensor_domain = 0,
  });

  driver_test().RunInEnvironmentTypeContext(
      [](auto& env) { env.i2c().set_name(std::optional<std::string>{kSensorName}); });

  StartDriver(kMetadata);

  {
    fidl::WireResult response = power_sensor()->GetSensorName();
    ASSERT_TRUE(response.ok());
    const std::string_view name(response->name.data(), response->name.size());
    EXPECT_EQ(name, kSensorName);
  }
}

}  // namespace power_sensor
