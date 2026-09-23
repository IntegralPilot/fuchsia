// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tcs3400.h"

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.i2c/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.lightsensor/cpp/fidl.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/fake-i2c/fake-i2c.h>
#include <lib/fit/function.h>
#include <lib/inspect/cpp/reader.h>
#include <lib/inspect/testing/cpp/inspect.h>
#include <lib/zx/clock.h>

#include <fbl/auto_lock.h>
#include <fbl/mutex.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"
#include "src/lib/testing/predicates/status.h"
#include "tcs3400-regs.h"

namespace tcs {

class SyncReaderV2EventHandler
    : public fidl::WireSyncEventHandler<fuchsia_input_report::InputReportsReaderV2> {
 public:
  using Callback = fit::function<void(
      fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>;

  explicit SyncReaderV2EventHandler(Callback callback = nullptr) : callback_(std::move(callback)) {}

  void OnInputReports(
      fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) override {
    last_report_stamp = event->last_report_stamp;
    if (callback_) {
      callback_(event);
    }
  }
  void handle_unknown_event(
      fidl::UnknownEventMetadata<fuchsia_input_report::InputReportsReaderV2> metadata) override {}

  uint64_t last_report_stamp = 0;

 private:
  Callback callback_;
};

class FakeLightSensor : public fake_i2c::FakeI2c {
 public:
  uint8_t GetRegisterLastWrite(const uint8_t address) {
    fbl::AutoLock lock(&registers_lock_);
    return registers_[address].size() ? registers_[address].back() : 0;
  }
  uint8_t GetRegisterAtIndex(size_t index, const uint8_t address) {
    fbl::AutoLock lock(&registers_lock_);
    return registers_[address][index];
  }

  void SetRegister(const uint8_t address, const uint8_t value) {
    fbl::AutoLock lock(&registers_lock_);
    registers_[address].push_back(value);
  }

  void set_fail_transactions(bool fail_transactions) { fail_transactions_ = fail_transactions; }

  sync_completion_t* read_completion() { return &read_completion_; }

  sync_completion_t* configuration_completion() { return &configuration_completion_; }

 protected:
  zx_status_t Transact(const uint8_t* write_buffer, size_t write_buffer_size, uint8_t* read_buffer,
                       size_t* read_buffer_size, size_t expected_read_size) override {
    if (fail_transactions_) {
      return ZX_ERR_INTERNAL;
    }

    if (write_buffer_size < 1) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    const uint8_t address = write_buffer[0];
    write_buffer++;
    write_buffer_size--;

    // Assume that there are no multi-byte register accesses.

    if (write_buffer_size > 1) {
      return ZX_ERR_NOT_SUPPORTED;
    }

    {
      fbl::AutoLock lock(&registers_lock_);
      if (write_buffer_size == 1) {
        registers_[address].push_back(write_buffer[0]);
      }
    }
    read_buffer[0] = GetRegisterLastWrite(address);

    *read_buffer_size = 1;

    // The interrupt or timeout has been received and the driver is reading out the data registers.
    if (address == TCS_I2C_BDATAH) {
      sync_completion_signal(&read_completion_);
    } else if (!first_enable_written_ && address == TCS_I2C_ENABLE) {
      first_enable_written_ = true;
    } else if (first_enable_written_ && address == TCS_I2C_ENABLE) {
      first_enable_written_ = false;
      sync_completion_signal(&configuration_completion_);
    }

    return ZX_OK;
  }

 private:
  fbl::Mutex registers_lock_;
  std::array<std::vector<uint8_t>, UINT8_MAX> registers_ TA_GUARDED(registers_lock_) = {};
  sync_completion_t read_completion_;
  sync_completion_t configuration_completion_;
  bool first_enable_written_ = false;
  bool fail_transactions_ = false;
};

class Tcs3400TestEnvironment : public fdf_testing::Environment {
 public:
  void Init(const fuchsia_hardware_lightsensor::Metadata& metadata, zx::interrupt gpio_interrupt) {
    gpio_.SetInterrupt(zx::ok(std::move(gpio_interrupt)));

    ASSERT_OK(
        pdev_.AddFidlMetadata(fuchsia_hardware_lightsensor::Metadata::kSerializableName, metadata));
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    EXPECT_OK(to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
        pdev_.GetInstanceHandler(dispatcher), "pdev"));

    EXPECT_OK(to_driver_vfs.AddService<fuchsia_hardware_i2c::Service>(
        i2c_.CreateInstanceHandler(dispatcher), "i2c"));

    EXPECT_OK(to_driver_vfs.AddService<fuchsia_hardware_gpio::Service>(
        gpio_.CreateInstanceHandler(), "gpio-int"));

    return zx::ok();
  }

  FakeLightSensor& i2c() { return i2c_; }

 private:
  fdf_fake::FakePDev pdev_;
  FakeLightSensor i2c_;
  fake_gpio::FakeGpio gpio_;
};

// Wrapper around `Tcs3400` needed in order to expose the driver's inspect data.
class TestTcs3400 : public Tcs3400 {
 public:
  TestTcs3400() = default;

  static DriverRegistration GetDriverRegistration() {
    return FUCHSIA_DRIVER_REGISTRATION_V1(fdf_internal::DriverServer2<TestTcs3400>::initialize,
                                          fdf_internal::DriverServer2<TestTcs3400>::destroy);
  }

  inspect::ComponentInspector& inspector() { return *component_inspector_; }
  std::shared_ptr<sync_completion_t> next_reader_wait() { return next_reader_wait_; }

 protected:
  void OnNextReader() { sync_completion_signal(next_reader_wait_.get()); }

 private:
  std::shared_ptr<sync_completion_t> next_reader_wait_ = std::make_shared<sync_completion_t>();
};

class TestConfig final {
 public:
  using DriverType = TestTcs3400;
  using EnvironmentType = Tcs3400TestEnvironment;
};

class Tcs3400TestBase {
 public:
  void StartDriver(const fuchsia_hardware_lightsensor::Metadata& metadata) {
    ASSERT_OK(zx::interrupt::create(zx::resource(ZX_HANDLE_INVALID), 0, ZX_INTERRUPT_VIRTUAL,
                                    &gpio_interrupt_));
    zx::interrupt gpio_interrupt;
    ASSERT_OK(gpio_interrupt_.duplicate(ZX_RIGHT_SAME_RIGHTS, &gpio_interrupt));

    driver_test_.RunInEnvironmentTypeContext(
        [&](auto& env) { env.Init(metadata, std::move(gpio_interrupt)); });

    ASSERT_OK(driver_test_.StartDriver());

    WaitForConfiguration();

    zx::result input_device = driver_test_.ConnectThroughDevfs<fuchsia_input_report::InputDevice>(
        Tcs3400::kChildNodeName);
    ASSERT_OK(input_device);
    input_device_.Bind(std::move(input_device.value()));

    driver_test_.RunInDriverContext(
        [&](auto& driver) { next_reader_wait_ = driver.next_reader_wait(); });
  }

  void WaitForConfiguration() {
    sync_completion_t* completion;
    driver_test_.RunInEnvironmentTypeContext(
        [&](auto& env) { completion = env.i2c().configuration_completion(); });

    sync_completion_wait(completion, ZX_TIME_INFINITE);
    sync_completion_reset(completion);
  }

  fidl::WireSyncClient<fuchsia_input_report::InputDevice>& input_device() { return input_device_; }
  zx::interrupt& gpio_interrupt() { return gpio_interrupt_; }
  fdf_testing::BackgroundDriverTest<TestConfig>& driver_test() { return driver_test_; }
  std::shared_ptr<sync_completion_t>& next_reader_wait() { return next_reader_wait_; }

 private:
  fdf_testing::BackgroundDriverTest<TestConfig> driver_test_;
  zx::interrupt gpio_interrupt_;
  fidl::WireSyncClient<fuchsia_input_report::InputDevice> input_device_;
  std::shared_ptr<sync_completion_t> next_reader_wait_;
};

class Tcs3400Test : public Tcs3400TestBase, public ::testing::Test {
 public:
  static const fuchsia_hardware_lightsensor::Metadata kLightSensorMetadata;

  void SetUp() override {
    StartDriver(kLightSensorMetadata);

    driver_test().RunInEnvironmentTypeContext([](auto& env) {
      EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_ATIME), 35);
      EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_CONTROL), 0x02);
    });
  }

  void TearDown() override { ASSERT_OK(driver_test().StopDriver()); }

 protected:
  void GetFeatureReport(Tcs3400FeatureReport* const out_report) {
    const auto response = input_device()->GetFeatureReport();
    ASSERT_TRUE(response.ok());
    ASSERT_FALSE(response->is_error());
    ASSERT_TRUE(response->value()->report.has_sensor());

    const auto& report = response->value()->report.sensor();
    EXPECT_TRUE(report.has_report_interval());
    ASSERT_TRUE(report.has_reporting_state());

    ASSERT_TRUE(report.has_sensitivity());
    ASSERT_EQ(report.sensitivity().size(), 1u);

    ASSERT_TRUE(report.has_threshold_high());
    ASSERT_EQ(report.threshold_high().size(), 1u);

    ASSERT_TRUE(report.has_threshold_low());
    ASSERT_EQ(report.threshold_low().size(), 1u);

    ASSERT_TRUE(report.has_sampling_rate());

    out_report->report_interval_us = report.report_interval();
    out_report->reporting_state = report.reporting_state();
    out_report->sensitivity = report.sensitivity()[0];
    out_report->threshold_high = report.threshold_high()[0];
    out_report->threshold_low = report.threshold_low()[0];
    out_report->integration_time_us = report.sampling_rate();
  }

  void WaitForNextReader() {
    sync_completion_wait(next_reader_wait().get(), ZX_TIME_INFINITE);
    sync_completion_reset(next_reader_wait().get());
  }

  zx_status_t ReadAndAcknowledgeOneEvent(
      fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2>& reader,
      fit::function<
          void(fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>*)>
          callback) {
    SyncReaderV2EventHandler handler(std::move(callback));
    fidl::Status result = reader.HandleOneEvent(handler);
    if (!result.ok()) {
      return result.status();
    }
    return reader->AcknowledgeReports(handler.last_report_stamp).status();
  }

  auto SetFeatureReport(const Tcs3400FeatureReport& report) {
    fidl::Arena<512> allocator;
    fidl::VectorView<int64_t> sensitivity(allocator, 1);
    sensitivity[0] = report.sensitivity;

    fidl::VectorView<int64_t> threshold_high(allocator, 1);
    threshold_high[0] = report.threshold_high;

    fidl::VectorView<int64_t> threshold_low(allocator, 1);
    threshold_low[0] = report.threshold_low;

    const auto set_sensor_report =
        fuchsia_input_report::wire::SensorFeatureReport::Builder(allocator)
            .report_interval(report.report_interval_us)
            .reporting_state(report.reporting_state)
            .sensitivity(sensitivity)
            .threshold_high(threshold_high)
            .threshold_low(threshold_low)
            .sampling_rate(report.integration_time_us)
            .Build();

    const auto set_report = fuchsia_input_report::wire::FeatureReport::Builder(allocator)
                                .sensor(set_sensor_report)
                                .Build();

    return input_device()->SetFeatureReport(set_report);
  }

  void SetLightDataRegisters(uint16_t illuminance, uint16_t red, uint16_t green, uint16_t blue) {
    driver_test().RunInEnvironmentTypeContext([&](auto& env) {
      env.i2c().SetRegister(TCS_I2C_CDATAL, illuminance & 0xff);
      env.i2c().SetRegister(TCS_I2C_CDATAH, illuminance >> 8);

      env.i2c().SetRegister(TCS_I2C_RDATAL, red & 0xff);
      env.i2c().SetRegister(TCS_I2C_RDATAH, red >> 8);

      env.i2c().SetRegister(TCS_I2C_GDATAL, green & 0xff);
      env.i2c().SetRegister(TCS_I2C_GDATAH, green >> 8);

      env.i2c().SetRegister(TCS_I2C_BDATAL, blue & 0xff);
      env.i2c().SetRegister(TCS_I2C_BDATAH, blue >> 8);
    });
  }

  void WaitForLightDataRead() {
    sync_completion_t* completion;
    driver_test().RunInEnvironmentTypeContext(
        [&](auto& env) { completion = env.i2c().read_completion(); });

    sync_completion_wait(completion, ZX_TIME_INFINITE);
    sync_completion_reset(completion);
  }
};

const fuchsia_hardware_lightsensor::Metadata Tcs3400Test::kLightSensorMetadata({
    .gain = 16,
    .integration_time = zx::usec(615'680).get(),
    .polling_time = zx::usec(0).get(),
});

TEST_F(Tcs3400Test, GetInputReports) {
  constexpr Tcs3400FeatureReport kEnableThresholdEvents = {
      .report_interval_us = 0,
      .reporting_state = fuchsia_input_report::wire::SensorReportingState::kReportThresholdEvents,
      .sensitivity = 16,
      .threshold_high = 0x8000,
      .threshold_low = 0x1000,
      .integration_time_us = 615'000,
  };

  {
    const auto response = SetFeatureReport(kEnableThresholdEvents);
    ASSERT_TRUE(response.ok());
    EXPECT_FALSE(response->is_error());
  }

  fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> endpoints =
      fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> result =
      input_device()->GetInputReportsReaderV2(std::move(endpoints.server), 10);
  ASSERT_OK(result.status());
  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader(
      std::move(endpoints.client));
  WaitForNextReader();

  SetLightDataRegisters(0x00f8, 0xe79d, 0xa5e4, 0xfb1b);

  EXPECT_OK(gpio_interrupt().trigger(0, zx::clock::get_boot()));

  // Wait for the driver to read out the data registers. At this point the interrupt has been ack'd
  // and it is safe to trigger again.
  WaitForLightDataRead();

  zx_status_t status1 = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        ASSERT_TRUE(reports[0].has_sensor());
        ASSERT_TRUE(reports[0].sensor().has_values());
        ASSERT_EQ(reports[0].sensor().values().size(), 4u);

        EXPECT_EQ(reports[0].sensor().values()[0], 0x00f8);
        EXPECT_EQ(reports[0].sensor().values()[1], 0xe79d);
        EXPECT_EQ(reports[0].sensor().values()[2], 0xa5e4);
        EXPECT_EQ(reports[0].sensor().values()[3], 0xfb1b);
      });
  ASSERT_OK(status1);

  SetLightDataRegisters(0x67f3, 0xbe39, 0x21e9, 0x319a);
  EXPECT_OK(gpio_interrupt().trigger(0, zx::clock::get_boot()));
  WaitForLightDataRead();

  SetLightDataRegisters(0xa5df, 0x0101, 0xc776, 0xc531);
  EXPECT_OK(gpio_interrupt().trigger(0, zx::clock::get_boot()));
  WaitForLightDataRead();

  // The previous illuminance value did not cross a threshold, so there should only be one report to
  // read out.
  zx_status_t status2 = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;

        ASSERT_EQ(reports.size(), 1u);
        ASSERT_TRUE(reports[0].has_sensor());
        ASSERT_TRUE(reports[0].sensor().has_values());
        ASSERT_EQ(reports[0].sensor().values().size(), 4u);

        EXPECT_EQ(reports[0].sensor().values()[0], 0xa5df);
        EXPECT_EQ(reports[0].sensor().values()[1], 0x0101);
        EXPECT_EQ(reports[0].sensor().values()[2], 0xc776);
        EXPECT_EQ(reports[0].sensor().values()[3], 0xc531);
      });
  ASSERT_OK(status2);

  SetLightDataRegisters(0x1772, 0x95fa, 0xb263, 0x2f32);

  constexpr Tcs3400FeatureReport kEnableAllEvents = {
      .report_interval_us = 1'000,
      .reporting_state = fuchsia_input_report::wire::SensorReportingState::kReportAllEvents,
      .sensitivity = 16,
      .threshold_high = 0x8000,
      .threshold_low = 0x1000,
      .integration_time_us = 615'000,
  };

  {
    const auto response = SetFeatureReport(kEnableAllEvents);
    ASSERT_TRUE(response.ok());
    EXPECT_FALSE(response->is_error());
  }

  for (uint32_t report_count = 0; report_count < 10;) {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          for (const fuchsia_input_report::wire::InputReport& report : event->reports) {
            ASSERT_TRUE(report.has_sensor());
            ASSERT_TRUE(report.sensor().has_values());
            ASSERT_EQ(report.sensor().values().size(), 4u);

            EXPECT_EQ(report.sensor().values()[0], 0x1772);
            EXPECT_EQ(report.sensor().values()[1], 0x95fa);
            EXPECT_EQ(report.sensor().values()[2], 0xb263);
            EXPECT_EQ(report.sensor().values()[3], 0x2f32);
            report_count++;
          }
        });
    ASSERT_OK(status);
  }
}

TEST_F(Tcs3400Test, GetMultipleInputReports) {
  constexpr Tcs3400FeatureReport kEnableThresholdEvents = {
      .report_interval_us = 0,
      .reporting_state = fuchsia_input_report::wire::SensorReportingState::kReportThresholdEvents,
      .sensitivity = 16,
      .threshold_high = 0x8000,
      .threshold_low = 0x1000,
      .integration_time_us = 615'000,
  };

  const auto response = SetFeatureReport(kEnableThresholdEvents);
  ASSERT_TRUE(response.ok());
  EXPECT_FALSE(response->is_error());

  WaitForConfiguration();

  fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> endpoints =
      fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> result =
      input_device()->GetInputReportsReaderV2(std::move(endpoints.server), 10);
  ASSERT_OK(result.status());
  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader(
      std::move(endpoints.client));
  WaitForNextReader();

  constexpr uint16_t kExpectedLightValues[][4] = {
      {0x00f8, 0xe79d, 0xfb1b, 0xa5e4},
      {0x87f3, 0xbe39, 0x319a, 0x21e9},
      {0xa772, 0x95fa, 0x2f32, 0xb263},
  };

  for (const auto& values : kExpectedLightValues) {
    SetLightDataRegisters(values[0], values[1], values[2], values[3]);
    EXPECT_OK(gpio_interrupt().trigger(0, zx::clock::get_boot()));
    WaitForLightDataRead();
  }

  for (size_t i = 0; i < std::size(kExpectedLightValues);) {
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          for (const fuchsia_input_report::wire::InputReport& report : event->reports) {
            ASSERT_TRUE(report.has_sensor());
            ASSERT_TRUE(report.sensor().has_values());
            ASSERT_EQ(report.sensor().values().size(), 4u);

            EXPECT_EQ(report.sensor().values()[0], kExpectedLightValues[i][0]);
            EXPECT_EQ(report.sensor().values()[1], kExpectedLightValues[i][1]);
            EXPECT_EQ(report.sensor().values()[2], kExpectedLightValues[i][2]);
            EXPECT_EQ(report.sensor().values()[3], kExpectedLightValues[i][3]);
            i++;
          }
        });
    ASSERT_OK(status);
  }
}

TEST_F(Tcs3400Test, GetInputReportsMultipleReaders) {
  constexpr Tcs3400FeatureReport kEnableThresholdEvents = {
      .report_interval_us = 0,
      .reporting_state = fuchsia_input_report::wire::SensorReportingState::kReportThresholdEvents,
      .sensitivity = 16,
      .threshold_high = 0x8000,
      .threshold_low = 0x1000,
      .integration_time_us = 615'000,
  };

  const auto response = SetFeatureReport(kEnableThresholdEvents);
  ASSERT_TRUE(response.ok());
  EXPECT_FALSE(response->is_error());

  constexpr size_t kReaderCount = 5;

  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> readers[kReaderCount];
  for (fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2>& reader : readers) {
    fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> endpoints =
        fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
    reader.Bind(std::move(endpoints.client));
    fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> result =
        input_device()->GetInputReportsReaderV2(std::move(endpoints.server), 10);
    ASSERT_OK(result.status());
    WaitForNextReader();
  }

  SetLightDataRegisters(0x00f8, 0xe79d, 0xa5e4, 0xfb1b);

  EXPECT_OK(gpio_interrupt().trigger(0, zx::clock::get_boot()));

  for (fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2>& reader : readers) {
    bool got_report = false;
    zx_status_t status = ReadAndAcknowledgeOneEvent(
        reader,
        [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
          const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;
          ASSERT_EQ(reports.size(), 1u);
          ASSERT_TRUE(reports[0].has_sensor());
          ASSERT_TRUE(reports[0].sensor().has_values());
          ASSERT_EQ(reports[0].sensor().values().size(), 4u);

          EXPECT_EQ(reports[0].sensor().values()[0], 0x00f8);
          EXPECT_EQ(reports[0].sensor().values()[1], 0xe79d);
          EXPECT_EQ(reports[0].sensor().values()[2], 0xa5e4);
          EXPECT_EQ(reports[0].sensor().values()[3], 0xfb1b);
          got_report = true;
        });
    ASSERT_OK(status);
    EXPECT_TRUE(got_report);
  }
}

TEST_F(Tcs3400Test, InputReportSaturatedSensor) {
  constexpr Tcs3400FeatureReport kEnableThresholdEvents = {
      .report_interval_us = 0,
      .reporting_state = fuchsia_input_report::wire::SensorReportingState::kReportAllEvents,
      .sensitivity = 16,
      .threshold_high = 0x8000,
      .threshold_low = 0x1000,
      .integration_time_us = 615'000,
  };

  {
    const auto response = SetFeatureReport(kEnableThresholdEvents);
    ASSERT_TRUE(response.ok());
    EXPECT_FALSE(response->is_error());
  }

  fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2> endpoints =
      fidl::Endpoints<fuchsia_input_report::InputReportsReaderV2>::Create();
  fidl::WireSyncClient<fuchsia_input_report::InputReportsReaderV2> reader(
      std::move(endpoints.client));
  fidl::WireResult<fuchsia_input_report::InputDevice::GetInputReportsReaderV2> result =
      input_device()->GetInputReportsReaderV2(std::move(endpoints.server), 10);
  ASSERT_OK(result.status());
  WaitForNextReader();

  // Set normal value so we can be sure status register is causing saturation.
  SetLightDataRegisters(0x0010, 0x0010, 0x0010, 0x0010);
  driver_test().RunInEnvironmentTypeContext(
      [](auto& env) { env.i2c().SetRegister(TCS_I2C_STATUS, 0x0 | TCS_I2C_STATUS_ASAT); });

  EXPECT_OK(gpio_interrupt().trigger(0, zx::clock::get_boot()));

  WaitForLightDataRead();

  bool got_report = false;
  zx_status_t status = ReadAndAcknowledgeOneEvent(
      reader,
      [&](fidl::WireEvent<fuchsia_input_report::InputReportsReaderV2::OnInputReports>* event) {
        const fidl::VectorView<fuchsia_input_report::wire::InputReport>& reports = event->reports;
        ASSERT_EQ(reports.size(), 1u);
        ASSERT_TRUE(reports[0].has_sensor());
        ASSERT_TRUE(reports[0].sensor().has_values());
        ASSERT_EQ(reports[0].sensor().values().size(), 4u);

        EXPECT_EQ(reports[0].sensor().values()[0], 65085);
        EXPECT_EQ(reports[0].sensor().values()[1], 21067);
        EXPECT_EQ(reports[0].sensor().values()[2], 20395);
        EXPECT_EQ(reports[0].sensor().values()[3], 20939);
        got_report = true;
      });
  ASSERT_OK(status);
  EXPECT_TRUE(got_report);

  driver_test().RunInEnvironmentTypeContext(
      [](auto& env) { EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_CICLEAR), 0x00); });
}

TEST_F(Tcs3400Test, GetDescriptor) {
  const auto response = input_device()->GetDescriptor();
  ASSERT_TRUE(response.ok());
  ASSERT_TRUE(response.value().descriptor.has_device_information());
  ASSERT_TRUE(response.value().descriptor.has_sensor());
  ASSERT_TRUE(response.value().descriptor.sensor().has_input());
  ASSERT_EQ(response.value().descriptor.sensor().input().size(), 1u);
  ASSERT_TRUE(response.value().descriptor.sensor().input()[0].has_values());
  ASSERT_EQ(response.value().descriptor.sensor().input()[0].values().size(), 4u);

  EXPECT_EQ(response.value().descriptor.device_information().vendor_id(),
            static_cast<uint32_t>(fuchsia_input_report::wire::VendorId::kGoogle));
  EXPECT_EQ(
      response.value().descriptor.device_information().product_id(),
      static_cast<uint32_t>(fuchsia_input_report::wire::VendorGoogleProductId::kAmsLightSensor));

  const auto& sensor_axes = response.value().descriptor.sensor().input()[0].values();
  EXPECT_EQ(sensor_axes[0].type, fuchsia_input_report::wire::SensorType::kLightIlluminance);
  EXPECT_EQ(sensor_axes[1].type, fuchsia_input_report::wire::SensorType::kLightRed);
  EXPECT_EQ(sensor_axes[2].type, fuchsia_input_report::wire::SensorType::kLightGreen);
  EXPECT_EQ(sensor_axes[3].type, fuchsia_input_report::wire::SensorType::kLightBlue);

  for (const auto& axis : sensor_axes) {
    EXPECT_EQ(axis.axis.range.min, 0);
    EXPECT_EQ(axis.axis.range.max, UINT16_MAX);
    EXPECT_EQ(axis.axis.unit.type, fuchsia_input::wire::UnitType::kOther);
    EXPECT_EQ(axis.axis.unit.exponent, 0);
  }

  ASSERT_TRUE(response.value().descriptor.sensor().has_feature());
  ASSERT_EQ(response.value().descriptor.sensor().feature().size(), 1u);
  const auto& feature_descriptor = response.value().descriptor.sensor().feature()[0];

  ASSERT_TRUE(feature_descriptor.has_report_interval());
  ASSERT_TRUE(feature_descriptor.has_supports_reporting_state());

  ASSERT_TRUE(feature_descriptor.has_sensitivity());
  ASSERT_EQ(feature_descriptor.sensitivity().size(), 1u);

  ASSERT_TRUE(feature_descriptor.has_threshold_high());
  ASSERT_EQ(feature_descriptor.threshold_high().size(), 1u);

  ASSERT_TRUE(feature_descriptor.has_threshold_low());
  ASSERT_EQ(feature_descriptor.threshold_low().size(), 1u);

  EXPECT_EQ(feature_descriptor.report_interval().range.min, 0);
  EXPECT_EQ(feature_descriptor.report_interval().unit.type,
            fuchsia_input::wire::UnitType::kSeconds);
  EXPECT_EQ(feature_descriptor.report_interval().unit.exponent, -6);

  EXPECT_TRUE(feature_descriptor.supports_reporting_state());

  EXPECT_EQ(feature_descriptor.sensitivity()[0].type,
            fuchsia_input_report::wire::SensorType::kLightIlluminance);
  EXPECT_EQ(feature_descriptor.sensitivity()[0].axis.range.min, 1);
  EXPECT_EQ(feature_descriptor.sensitivity()[0].axis.range.max, 64);
  EXPECT_EQ(feature_descriptor.sensitivity()[0].axis.unit.type,
            fuchsia_input::wire::UnitType::kOther);
  EXPECT_EQ(feature_descriptor.sensitivity()[0].axis.unit.exponent, 0);

  EXPECT_EQ(feature_descriptor.threshold_high()[0].type,
            fuchsia_input_report::wire::SensorType::kLightIlluminance);
  EXPECT_EQ(feature_descriptor.threshold_high()[0].axis.range.min, 0);
  EXPECT_EQ(feature_descriptor.threshold_high()[0].axis.range.max, UINT16_MAX);
  EXPECT_EQ(feature_descriptor.threshold_high()[0].axis.unit.type,
            fuchsia_input::wire::UnitType::kOther);
  EXPECT_EQ(feature_descriptor.threshold_high()[0].axis.unit.exponent, 0);

  EXPECT_EQ(feature_descriptor.threshold_low()[0].type,
            fuchsia_input_report::wire::SensorType::kLightIlluminance);
  EXPECT_EQ(feature_descriptor.threshold_low()[0].axis.range.min, 0);
  EXPECT_EQ(feature_descriptor.threshold_low()[0].axis.range.max, UINT16_MAX);
  EXPECT_EQ(feature_descriptor.threshold_low()[0].axis.unit.type,
            fuchsia_input::wire::UnitType::kOther);
  EXPECT_EQ(feature_descriptor.threshold_low()[0].axis.unit.exponent, 0);
}

TEST_F(Tcs3400Test, FeatureReport) {
  Tcs3400FeatureReport report;
  ASSERT_NO_FATAL_FAILURE(GetFeatureReport(&report));

  // Check the default report values.
  EXPECT_EQ(report.reporting_state,
            fuchsia_input_report::wire::SensorReportingState::kReportAllEvents);
  EXPECT_EQ(report.threshold_high, 0xffff);
  EXPECT_EQ(report.threshold_low, 0x0000);
  EXPECT_EQ(report.integration_time_us, 614'380);

  // These values are passed in through metadata.
  EXPECT_EQ(report.report_interval_us, 0);
  EXPECT_EQ(report.sensitivity, 16);

  // Inspect report should match.
  {
    zx::vmo inspect_vmo;
    driver_test().RunInDriverContext(
        [&](auto& driver) { inspect_vmo = driver.inspector().inspector().DuplicateVmo(); });
    inspect::Hierarchy hierarchy = inspect::ReadFromVmo(inspect_vmo).take_value();
    const inspect::Hierarchy* root = hierarchy.GetByPath({"feature_report", "1"});
    ASSERT_EQ(root, nullptr);
  }

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    env.i2c().SetRegister(TCS_I2C_ENABLE, 0);
    env.i2c().SetRegister(TCS_I2C_AILTL, 0);
    env.i2c().SetRegister(TCS_I2C_AILTH, 0);
    env.i2c().SetRegister(TCS_I2C_AIHTL, 0);
    env.i2c().SetRegister(TCS_I2C_AIHTH, 0);
    env.i2c().SetRegister(TCS_I2C_PERS, 0);
    env.i2c().SetRegister(TCS_I2C_CONTROL, 0);
    env.i2c().SetRegister(TCS_I2C_ATIME, 0);
  });

  constexpr Tcs3400FeatureReport kNewFeatureReport = {
      .report_interval_us = 1'000,
      .reporting_state = fuchsia_input_report::wire::SensorReportingState::kReportAllEvents,
      .sensitivity = 64,
      .threshold_high = 0xabcd,
      .threshold_low = 0x1234,
      .integration_time_us = 278'000,
  };
  const auto response = SetFeatureReport(kNewFeatureReport);
  ASSERT_TRUE(response.ok());
  EXPECT_FALSE(response->is_error());

  WaitForConfiguration();

  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    EXPECT_EQ(env.i2c().GetRegisterAtIndex(0, TCS_I2C_ENABLE), 0b0001'0001);
    EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_AILTL), 0x34);
    EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_AILTH), 0x12);
    EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_AIHTL), 0xcd);
    EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_AIHTH), 0xab);
    EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_CONTROL), 3);
    EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_ATIME), 156);
    EXPECT_EQ(env.i2c().GetRegisterAtIndex(1, TCS_I2C_ENABLE), 0b0001'0011);
  });

  ASSERT_NO_FATAL_FAILURE(GetFeatureReport(&report));
  EXPECT_EQ(report.report_interval_us, 1'000);
  EXPECT_EQ(report.reporting_state,
            fuchsia_input_report::wire::SensorReportingState::kReportAllEvents);
  EXPECT_EQ(report.sensitivity, 64);
  EXPECT_EQ(report.threshold_high, 0xabcd);
  EXPECT_EQ(report.threshold_low, 0x1234);
  EXPECT_EQ(report.integration_time_us, 278'000);

  // Inspect report should match.
  zx::vmo inspect_vmo;
  driver_test().RunInDriverContext(
      [&](auto& driver) { inspect_vmo = driver.inspector().inspector().DuplicateVmo(); });
  inspect::Hierarchy hierarchy = inspect::ReadFromVmo(inspect_vmo).take_value();
  const inspect::Hierarchy* root = hierarchy.GetByPath({"feature_reports", "1"});
  ASSERT_NE(root, nullptr);

  EXPECT_THAT(root->node(),
              inspect::testing::PropertyList(testing::AllOf(
                  testing::Contains(inspect::testing::UintIs("report_interval_us", 1'000)),
                  testing::Contains(inspect::testing::StringIs("reporting_state", "AllEvents")),
                  testing::Contains(inspect::testing::UintIs("sensitivity", 64)),
                  testing::Contains(inspect::testing::UintIs("threshold_high", 0xabcd)),
                  testing::Contains(inspect::testing::UintIs("threshold_low", 0x1234)),
                  testing::Contains(inspect::testing::UintIs("integration_time_us", 278'000)))));
}

TEST_F(Tcs3400Test, SetInvalidFeatureReport) {
  constexpr Tcs3400FeatureReport kInvalidReportInterval = {
      .report_interval_us = -1,
      .reporting_state = fuchsia_input_report::wire::SensorReportingState::kReportAllEvents,
      .sensitivity = 1,
  };

  {
    const auto response = SetFeatureReport(kInvalidReportInterval);
    ASSERT_TRUE(response.ok());
    EXPECT_TRUE(response->is_error());
  }

  Tcs3400FeatureReport report;
  ASSERT_NO_FATAL_FAILURE(GetFeatureReport(&report));
  // Make sure the feature report wasn't affected by the bad call.
  EXPECT_EQ(report.sensitivity, 16);
  EXPECT_EQ(report.report_interval_us, 0);

  constexpr Tcs3400FeatureReport kInvalidSensitivity = {
      .reporting_state = fuchsia_input_report::wire::SensorReportingState::kReportAllEvents,
      .sensitivity = 50,
  };

  {
    const auto response = SetFeatureReport(kInvalidSensitivity);
    ASSERT_TRUE(response.ok());
    EXPECT_TRUE(response->is_error());
  }

  ASSERT_NO_FATAL_FAILURE(GetFeatureReport(&report));
  EXPECT_EQ(report.sensitivity, 16);

  constexpr Tcs3400FeatureReport kInvalidThresholdHigh = {
      .reporting_state = fuchsia_input_report::wire::SensorReportingState::kReportAllEvents,
      .sensitivity = 1,
      .threshold_high = 0x10000,
  };

  {
    const auto response = SetFeatureReport(kInvalidThresholdHigh);
    ASSERT_TRUE(response.ok());
    EXPECT_TRUE(response->is_error());
  }

  ASSERT_NO_FATAL_FAILURE(GetFeatureReport(&report));
  EXPECT_EQ(report.threshold_high, 0xffff);
  EXPECT_EQ(report.sensitivity, 16);

  // Make sure the call fails if a field is omitted.
  fidl::Arena<512> allocator;
  fidl::VectorView<int64_t> sensitivity(allocator, 1);
  sensitivity[0] = 1;

  fidl::VectorView<int64_t> threshold_high(allocator, 1);
  threshold_high[0] = 0;

  const auto set_sensor_report =
      fuchsia_input_report::wire::SensorFeatureReport::Builder(allocator)
          .report_interval(report.report_interval_us)
          .reporting_state(fuchsia_input_report::wire::SensorReportingState::kReportAllEvents)
          .sensitivity(sensitivity)
          .threshold_high(threshold_high)
          .Build();

  const auto set_report = fuchsia_input_report::wire::FeatureReport::Builder(allocator)
                              .sensor(set_sensor_report)
                              .Build();

  {
    const auto response = input_device()->SetFeatureReport(set_report);
    ASSERT_TRUE(response.ok());
    EXPECT_TRUE(response->is_error());
  }

  ASSERT_NO_FATAL_FAILURE(GetFeatureReport(&report));
  EXPECT_EQ(report.threshold_high, 0xffff);
  EXPECT_EQ(report.threshold_low, 0x0000);
  EXPECT_EQ(report.sensitivity, 16);
  EXPECT_EQ(report.report_interval_us, 0);
  EXPECT_EQ(report.reporting_state,
            fuchsia_input_report::wire::SensorReportingState::kReportAllEvents);
}

class Tcs3400MetadataTest
    : public Tcs3400TestBase,
      public ::testing::TestWithParam<std::tuple<uint8_t, uint8_t, zx::duration, uint8_t>> {
 public:
  void TearDown() override { ASSERT_OK(driver_test().StopDriver()); }

 protected:
  void StartDriver(uint8_t gain, zx::duration integration_time) {
    driver_test().RunInEnvironmentTypeContext([](auto& env) {
      env.i2c().SetRegister(TCS_I2C_ATIME, 0xff);
      env.i2c().SetRegister(TCS_I2C_CONTROL, 0xff);
    });

    const fuchsia_hardware_lightsensor::Metadata metadata(
        {.gain = gain, .integration_time = integration_time.get(), .polling_time = 0});

    Tcs3400TestBase::StartDriver(metadata);
  }
};

TEST_P(Tcs3400MetadataTest, MetadataTest) {
  auto [gain, again_register, integration_time, atime_register] = GetParam();

  StartDriver(gain, integration_time);

  driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_ATIME), atime_register);
    EXPECT_EQ(env.i2c().GetRegisterLastWrite(TCS_I2C_CONTROL), again_register);
  });
}

INSTANTIATE_TEST_SUITE_P(
    Tcs3400MetadataTests, Tcs3400MetadataTest,
    ::testing::Values(
        std::tuple(99, 0x00, zx::usec(612'000), 36),  // Invalid gain sets again = 0 (gain = 1).
        std::tuple(1, 0x00, zx::usec(612'000), 36), std::tuple(4, 0x01, zx::usec(612'000), 36),
        std::tuple(16, 0x02, zx::usec(612'000), 36), std::tuple(64, 0x03, zx::usec(612'000), 36),
        std::tuple(1, 0x00, zx::usec(750'000), 0x01),  // Invalid integration time sets atime = 1
        std::tuple(1, 0x00, zx::usec(708'900), 0x01), std::tuple(1, 0x00, zx::usec(706'120), 0x02),
        std::tuple(1, 0x00, zx::usec(703'340), 0x03), std::tuple(1, 0x00, zx::usec(2'780), 0xFF)));

TEST(Tcs3400ErrorTest, TooManyI2cErrors) {
  Tcs3400TestBase test;
  static const fuchsia_hardware_lightsensor::Metadata kMetadata({
      .gain = 64,
      .integration_time = zx::usec(708'900).get(),  // For atime = 0x01.
  });

  zx::interrupt gpio_interrupt;
  ASSERT_OK(zx::interrupt::create(zx::resource(ZX_HANDLE_INVALID), 0, ZX_INTERRUPT_VIRTUAL,
                                  &gpio_interrupt));

  test.driver_test().RunInEnvironmentTypeContext([&](auto& env) {
    env.Init(kMetadata, std::move(gpio_interrupt));
    env.i2c().set_fail_transactions(true);
  });

  ASSERT_NE(test.driver_test().StartDriver().status_value(), ZX_OK);
}

}  // namespace tcs
