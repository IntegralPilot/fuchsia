// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-thermal.h"

#include <fidl/fuchsia.hardware.pwm/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/ddk/debug.h>
#include <lib/ddk/device.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <stdint.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/syscalls.h>
#include <string.h>
#include <zircon/syscalls/port.h>
#include <zircon/types.h>

#include <cstddef>
#include <list>
#include <memory>

#include <fbl/alloc_checker.h>
#include <fbl/array.h>
#include <mock-mmio-reg/mock-mmio-reg.h>
#include <zxtest/zxtest.h>

bool operator==(const fuchsia_hardware_pwm::wire::PwmConfig& lhs,
                const fuchsia_hardware_pwm::wire::PwmConfig& rhs) {
  return (lhs.polarity == rhs.polarity) && (lhs.period_ns == rhs.period_ns) &&
         (lhs.duty_cycle == rhs.duty_cycle) && (lhs.mode_config.size() == rhs.mode_config.size()) &&
         (reinterpret_cast<aml_pwm::mode_config*>(lhs.mode_config.data())->mode ==
          reinterpret_cast<aml_pwm::mode_config*>(rhs.mode_config.data())->mode);
}

namespace {

constexpr size_t kRegSize = 0x00002000 / sizeof(uint32_t);  // in 32 bits chunks.

// Temperature Sensor
// Copied from sherlock-thermal.cc
constexpr fuchsia_hardware_thermal::wire::ThermalTemperatureInfo TripPoint(float temp_c,
                                                                           float hysteresis_c,
                                                                           uint16_t cpu_opp_big,
                                                                           uint16_t cpu_opp_little,
                                                                           uint16_t gpu_opp) {
  return {
      .up_temp_celsius = temp_c + hysteresis_c,
      .down_temp_celsius = temp_c - hysteresis_c,
      .fan_level = 0,
      .big_cluster_dvfs_opp = cpu_opp_big,
      .little_cluster_dvfs_opp = cpu_opp_little,
      .gpu_clk_freq_source = gpu_opp,
  };
}

constexpr fuchsia_hardware_thermal::wire::ThermalDeviceInfo
    sherlock_thermal_config =
        {
            .active_cooling = false,
            .passive_cooling = true,
            .gpu_throttling = true,
            .num_trip_points = 6,
            .big_little = true,
            .critical_temp_celsius = 102.0f,
            .trip_point_info =
                {
                    TripPoint(55.0f, 2.0f, 9, 10, 4),
                    TripPoint(75.0f, 2.0f, 8, 9, 4),
                    TripPoint(80.0f, 2.0f, 7, 8, 3),
                    TripPoint(90.0f, 2.0f, 6, 7, 3),
                    TripPoint(95.0f, 2.0f, 5, 6, 3),
                    TripPoint(100.0f, 2.0f, 4, 5, 2),
                    // 0 Kelvin is impossible, marks end of TripPoints
                    TripPoint(-273.15f, 2.0f, 0, 0, 0),
                },
            .opps =
                {
                    fuchsia_hardware_thermal::wire::OperatingPoint{
                        .opp =
                            {
                                fuchsia_hardware_thermal::wire::OperatingPointEntry{
                                    .freq_hz = 100000000, .volt_uv = 751000},
                                {.freq_hz = 250000000, .volt_uv = 751000},
                                {.freq_hz = 500000000, .volt_uv = 751000},
                                {.freq_hz = 667000000, .volt_uv = 751000},
                                {.freq_hz = 1000000000, .volt_uv = 771000},
                                {.freq_hz = 1200000000, .volt_uv = 771000},
                                {.freq_hz = 1398000000, .volt_uv = 791000},
                                {.freq_hz = 1512000000, .volt_uv = 821000},
                                {.freq_hz = 1608000000, .volt_uv = 861000},
                                {.freq_hz = 1704000000, .volt_uv = 891000},
                                {.freq_hz = 1704000000, .volt_uv = 891000},
                            },
                        .latency = 0,
                        .count = 11,
                    },
                    {
                        .opp =
                            {
                                fuchsia_hardware_thermal::wire::OperatingPointEntry{
                                    .freq_hz = 100000000, .volt_uv = 731000},
                                {.freq_hz = 250000000, .volt_uv = 731000},
                                {.freq_hz = 500000000, .volt_uv = 731000},
                                {.freq_hz = 667000000, .volt_uv = 731000},
                                {.freq_hz = 1000000000, .volt_uv = 731000},
                                {.freq_hz = 1200000000, .volt_uv = 731000},
                                {.freq_hz = 1398000000, .volt_uv = 761000},
                                {.freq_hz = 1512000000, .volt_uv = 791000},
                                {.freq_hz = 1608000000, .volt_uv = 831000},
                                {.freq_hz = 1704000000, .volt_uv = 861000},
                                {.freq_hz = 1896000000, .volt_uv = 1011000},
                            },
                        .latency = 0,
                        .count = 11,
                    },
                },
};

constexpr fuchsia_hardware_thermal::wire::ThermalDeviceInfo astro_thermal_config =
    {
        .active_cooling = false,
        .passive_cooling = true,
        .gpu_throttling = true,
        .num_trip_points = 7,
        .big_little = false,
        .critical_temp_celsius = 102.0f,
        .trip_point_info =
            {
                TripPoint(0.0f, 2.0f, 10, 0, 5),
                TripPoint(75.0f, 2.0f, 9, 0, 4),
                TripPoint(80.0f, 2.0f, 8, 0, 3),
                TripPoint(85.0f, 2.0f, 7, 0, 3),
                TripPoint(90.0f, 2.0f, 6, 0, 2),
                TripPoint(95.0f, 2.0f, 5, 0, 1),
                TripPoint(100.0f, 2.0f, 4, 0, 0),
                // 0 Kelvin is impossible, marks end of TripPoints
                TripPoint(-273.15f, 2.0f, 0, 0, 0),
            },
        .opps =
            {
                fuchsia_hardware_thermal::wire::OperatingPoint{
                    .opp =
                        {
                            fuchsia_hardware_thermal::wire::OperatingPointEntry{
                                .freq_hz = 100'000'000, .volt_uv = 731'000},
                            {.freq_hz = 250'000'000, .volt_uv = 731'000},
                            {.freq_hz = 500'000'000, .volt_uv = 731'000},
                            {.freq_hz = 667'000'000, .volt_uv = 731'000},
                            {.freq_hz = 1'000'000'000, .volt_uv = 731'000},
                            {.freq_hz = 1'200'000'000, .volt_uv = 731'000},
                            {.freq_hz = 1'398'000'000, .volt_uv = 761'000},
                            {.freq_hz = 1'512'000'000, .volt_uv = 791'000},
                            {.freq_hz = 1'608'000'000, .volt_uv = 831'000},
                            {.freq_hz = 1'704'000'000, .volt_uv = 861'000},
                            {.freq_hz = 1'896'000'000, .volt_uv = 981'000},
                        },
                    .latency = 0,
                    .count = 11,
                },
            },
};

constexpr fuchsia_hardware_thermal::wire::ThermalDeviceInfo nelson_thermal_config = {
    .active_cooling = false,
    .passive_cooling = true,
    .gpu_throttling = true,
    .num_trip_points = 5,
    .big_little = false,
    .critical_temp_celsius = 110.0f,
    .trip_point_info =
        {
            TripPoint(0.0f, 5.0f, 11, 0, 5),
            TripPoint(60.0f, 5.0f, 9, 0, 4),
            TripPoint(75.0f, 5.0f, 8, 0, 3),
            TripPoint(80.0f, 5.0f, 7, 0, 2),
            TripPoint(110.0f, 1.0f, 0, 0, 0),
            // 0 Kelvin is impossible, marks end of TripPoints
            TripPoint(-273.15f, 2.0f, 0, 0, 0),
        },
    .opps =
        {
            fuchsia_hardware_thermal::wire::OperatingPoint{
                .opp =
                    {
                        fuchsia_hardware_thermal::wire::OperatingPointEntry{.freq_hz = 100'000'000,
                                                                            .volt_uv = 760'000},
                        {.freq_hz = 250'000'000, .volt_uv = 760'000},
                        {.freq_hz = 500'000'000, .volt_uv = 760'000},
                        {.freq_hz = 667'000'000, .volt_uv = 780'000},
                        {.freq_hz = 1'000'000'000, .volt_uv = 800'000},
                        {.freq_hz = 1'200'000'000, .volt_uv = 810'000},
                        {.freq_hz = 1'404'000'000, .volt_uv = 820'000},
                        {.freq_hz = 1'512'000'000, .volt_uv = 830'000},
                        {.freq_hz = 1'608'000'000, .volt_uv = 860'000},
                        {.freq_hz = 1'704'000'000, .volt_uv = 900'000},
                        {.freq_hz = 1'800'000'000, .volt_uv = 940'000},
                        {.freq_hz = 1'908'000'000, .volt_uv = 970'000},
                    },
                .latency = 0,
                .count = 12,
            },
        },
};

// Voltage Regulator
// Copied from sherlock-thermal.cc
aml_thermal_info_t fake_thermal_info = {
    .voltage_table =
        {
            {1022000, 0},  {1011000, 3}, {1001000, 6}, {991000, 10}, {981000, 13}, {971000, 16},
            {961000, 20},  {951000, 23}, {941000, 26}, {931000, 30}, {921000, 33}, {911000, 36},
            {901000, 40},  {891000, 43}, {881000, 46}, {871000, 50}, {861000, 53}, {851000, 56},
            {841000, 60},  {831000, 63}, {821000, 67}, {811000, 70}, {801000, 73}, {791000, 76},
            {781000, 80},  {771000, 83}, {761000, 86}, {751000, 90}, {741000, 93}, {731000, 96},
            {721000, 100},
        },
    .initial_cluster_frequencies =
        {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
            [static_cast<uint32_t>(
                fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain)] =
                1'000'000'000,
            [static_cast<uint32_t>(
                fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain)] =
                1'200'000'000,
#pragma GCC diagnostic pop
        },
    .voltage_pwm_period_ns = 1250,
    .opps = {},
    .cluster_id_map = {},
};

constexpr aml_thermal_info_t nelson_thermal_info = {
    .voltage_table =
        {
            {1'050'000, 0},  {1'040'000, 3}, {1'030'000, 6}, {1'020'000, 8}, {1'010'000, 11},
            {1'000'000, 14}, {990'000, 17},  {980'000, 20},  {970'000, 23},  {960'000, 26},
            {950'000, 29},   {940'000, 31},  {930'000, 34},  {920'000, 37},  {910'000, 40},
            {900'000, 43},   {890'000, 45},  {880'000, 48},  {870'000, 51},  {860'000, 54},
            {850'000, 56},   {840'000, 59},  {830'000, 62},  {820'000, 65},  {810'000, 68},
            {800'000, 70},   {790'000, 73},  {780'000, 76},  {770'000, 79},  {760'000, 81},
            {750'000, 84},   {740'000, 87},  {730'000, 89},  {720'000, 92},  {710'000, 95},
            {700'000, 98},   {690'000, 100},
        },
    .initial_cluster_frequencies =
        {
#pragma GCC diagnostic push
#pragma GCC diagnostic ignored "-Wc99-designator"
            [static_cast<uint32_t>(
                fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain)] =
                1'000'000'000,
#pragma GCC diagnostic pop
        },
    .voltage_pwm_period_ns = 1500,
    .opps = {},
    .cluster_id_map = {},
};

}  // namespace

namespace thermal {

// Temperature Sensor
class FakeAmlTSensor : public AmlTSensor {
 public:
  static std::unique_ptr<FakeAmlTSensor> Create(fdf::MmioBuffer pll_mmio, fdf::MmioBuffer trim_mmio,
                                                fdf::MmioBuffer hiu_mmio, bool less) {
    fbl::AllocChecker ac;

    auto test = fbl::make_unique_checked<FakeAmlTSensor>(&ac, std::move(pll_mmio),
                                                         std::move(trim_mmio), std::move(hiu_mmio));
    if (!ac.check()) {
      return nullptr;
    }

    auto config = sherlock_thermal_config;
    if (less) {
      config.num_trip_points = 2;
      config.trip_point_info[2].up_temp_celsius = -273.15f + 2.0f;
    }

    EXPECT_OK(test->InitSensor(config));
    return test;
  }

  explicit FakeAmlTSensor(fdf::MmioBuffer pll_mmio, fdf::MmioBuffer trim_mmio,
                          fdf::MmioBuffer hiu_mmio)
      : AmlTSensor(std::move(pll_mmio), std::move(trim_mmio), std::move(hiu_mmio)) {}
};

class AmlTSensorTest : public zxtest::Test {
 public:
  void SetUp() override {
    fbl::AllocChecker ac;

    mock_pll_mmio_ =
        fbl::make_unique_checked<ddk_mock::MockMmioRegRegion>(&ac, sizeof(uint32_t), kRegSize);
    if (!ac.check()) {
      zxlogf(ERROR, "AmlTSensorTest::SetUp: mock_pll_mmio_ alloc failed");
      return;
    }

    mock_trim_mmio_ =
        fbl::make_unique_checked<ddk_mock::MockMmioRegRegion>(&ac, sizeof(uint32_t), kRegSize);
    if (!ac.check()) {
      zxlogf(ERROR, "AmlTSensorTest::SetUp: mock_trim_mmio_ alloc failed");
      return;
    }

    mock_hiu_mmio_ =
        fbl::make_unique_checked<ddk_mock::MockMmioRegRegion>(&ac, sizeof(uint32_t), kRegSize);
    if (!ac.check()) {
      zxlogf(ERROR, "AmlTSensorTest::SetUp: mock_hiu_mmio_ alloc failed");
      return;
    }

    (*mock_trim_mmio_)[0].ExpectRead(0x00000000);                             // trim_info_
    (*mock_hiu_mmio_)[(0x64 << 2)].ExpectWrite(0x130U);                       // set clock
    (*mock_pll_mmio_)[(0x1 << 2)].ExpectRead(0x00000000).ExpectWrite(0x63B);  // sensor ctl
  }

  void Create(bool less) {
    // InitTripPoints
    if (!less) {
      (*mock_pll_mmio_)[(0x5 << 2)]
          .ExpectRead(0x00000000)  // set thresholds 4, rise
          .ExpectWrite(0x00027E);
      (*mock_pll_mmio_)[(0x7 << 2)]
          .ExpectRead(0x00000000)  // set thresholds 4, fall
          .ExpectWrite(0x000272);
      (*mock_pll_mmio_)[(0x5 << 2)]
          .ExpectRead(0x00000000)  // set thresholds 3, rise
          .ExpectWrite(0x272000);
      (*mock_pll_mmio_)[(0x7 << 2)]
          .ExpectRead(0x00000000)  // set thresholds 3, fall
          .ExpectWrite(0x268000);
      (*mock_pll_mmio_)[(0x4 << 2)]
          .ExpectRead(0x00000000)  // set thresholds 2, rise
          .ExpectWrite(0x00025A);
      (*mock_pll_mmio_)[(0x6 << 2)]
          .ExpectRead(0x00000000)  // set thresholds 2, fall
          .ExpectWrite(0x000251);
    }
    (*mock_pll_mmio_)[(0x4 << 2)]
        .ExpectRead(0x00000000)  // set thresholds 1, rise
        .ExpectWrite(0x250000);
    (*mock_pll_mmio_)[(0x6 << 2)]
        .ExpectRead(0x00000000)  // set thresholds 1, fall
        .ExpectWrite(0x245000);
    (*mock_pll_mmio_)[(0x1 << 2)]
        .ExpectRead(0x00000000)  // clear IRQs
        .ExpectWrite(0x00FF0000);
    (*mock_pll_mmio_)[(0x1 << 2)]
        .ExpectRead(0x00000000)  // clear IRQs
        .ExpectWrite(0x00000000);
    if (!less) {
      (*mock_pll_mmio_)[(0x1 << 2)]
          .ExpectRead(0x00000000)  // enable IRQs
          .ExpectWrite(0x0F008000);
    } else {
      (*mock_pll_mmio_)[(0x1 << 2)]
          .ExpectRead(0x00000000)  // enable IRQs
          .ExpectWrite(0x01008000);
    }

    // Enable SoC reset at 102.0f
    (*mock_pll_mmio_)[(0x2 << 2)].ExpectRead(0x0);
    (*mock_pll_mmio_)[(0x2 << 2)].ExpectWrite(0xc0ff2880);

    fdf::MmioBuffer pll_mmio(mock_pll_mmio_->GetMmioBuffer());
    fdf::MmioBuffer trim_mmio(mock_trim_mmio_->GetMmioBuffer());
    fdf::MmioBuffer hiu_mmio(mock_hiu_mmio_->GetMmioBuffer());
    tsensor_ = FakeAmlTSensor::Create(std::move(pll_mmio), std::move(trim_mmio),
                                      std::move(hiu_mmio), less);
    ASSERT_TRUE(tsensor_ != nullptr);
  }

  void TearDown() override {
    // Verify
    mock_pll_mmio_->VerifyAll();
    mock_trim_mmio_->VerifyAll();
    mock_hiu_mmio_->VerifyAll();
  }

 protected:
  std::unique_ptr<FakeAmlTSensor> tsensor_;

  std::unique_ptr<ddk_mock::MockMmioRegRegion> mock_pll_mmio_;
  std::unique_ptr<ddk_mock::MockMmioRegRegion> mock_trim_mmio_;
  std::unique_ptr<ddk_mock::MockMmioRegRegion> mock_hiu_mmio_;
};

TEST_F(AmlTSensorTest, ReadTemperatureCelsiusTest0) {
  Create(false);
  for (int j = 0; j < 0x10; j++) {
    (*mock_pll_mmio_)[(0x10 << 2)].ExpectRead(0x0000);
  }

  float val = tsensor_->ReadTemperatureCelsius();
  EXPECT_EQ(val, 0.0);
}

TEST_F(AmlTSensorTest, ReadTemperatureCelsiusTest1) {
  Create(false);
  for (int j = 0; j < 0x10; j++) {
    (*mock_pll_mmio_)[(0x10 << 2)].ExpectRead(0x18A9);
  }

  float val = tsensor_->ReadTemperatureCelsius();
  EXPECT_EQ(val, 429496704.0);
}

TEST_F(AmlTSensorTest, ReadTemperatureCelsiusTest2) {
  Create(false);
  for (int j = 0; j < 0x10; j++) {
    (*mock_pll_mmio_)[(0x10 << 2)].ExpectRead(0x32A7);
  }

  float val = tsensor_->ReadTemperatureCelsius();
  EXPECT_EQ(val, 0.0);
}

TEST_F(AmlTSensorTest, ReadTemperatureCelsiusTest3) {
  Create(false);
  (*mock_pll_mmio_)[(0x10 << 2)].ExpectRead(0x18A9);
  (*mock_pll_mmio_)[(0x10 << 2)].ExpectRead(0x18AA);
  for (int j = 0; j < 0xE; j++) {
    (*mock_pll_mmio_)[(0x10 << 2)].ExpectRead(0x0000);
  }

  float val = tsensor_->ReadTemperatureCelsius();
  EXPECT_EQ(val, 429496704.0);
}

TEST_F(AmlTSensorTest, GetStateChangePortTest) {
  Create(false);
  zx_handle_t port;
  EXPECT_OK(tsensor_->GetStateChangePort(&port));

  zx_port_packet_t packet;
  memset(&packet, 0xff, sizeof(packet));
  EXPECT_OK(zx_port_wait(port, zx_deadline_after(ZX_SEC(5)), &packet));
  EXPECT_EQ(packet.key, 0);
  EXPECT_EQ(packet.type, ZX_PKT_TYPE_USER);
  EXPECT_EQ(packet.status, 0);
  for (uint64_t val : packet.user.u64) {
    EXPECT_EQ(val, 0);
  }

  zx_handle_close(port);
}

TEST_F(AmlTSensorTest, LessTripPointsTest) { Create(true); }

// Voltage Regulator
class FakeAmlVoltageRegulator : public AmlVoltageRegulator {
 public:
  static std::unique_ptr<FakeAmlVoltageRegulator> Create(
      fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> big_cluster_pwm,
      fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> little_cluster_pwm, uint32_t pid) {
    fbl::AllocChecker ac;

    auto test = fbl::make_unique_checked<FakeAmlVoltageRegulator>(&ac);
    if (!ac.check()) {
      return nullptr;
    }

    const auto& config = (pid == 4 ? sherlock_thermal_config : astro_thermal_config);
    EXPECT_OK(test->Init(std::move(big_cluster_pwm), std::move(little_cluster_pwm), config,
                         &fake_thermal_info));
    return test;
  }
};

class MockPwmServer final : public fidl::testing::WireTestBase<fuchsia_hardware_pwm::Pwm> {
 public:
  void SetConfig(SetConfigRequestView request, SetConfigCompleter::Sync& completer) override {
    ASSERT_GT(expect_configs_.size(), 0);
    auto expect_config = expect_configs_.front();

    ASSERT_EQ(request->config, expect_config);

    expect_configs_.pop_front();
    mode_config_buffers_.pop_front();
    completer.ReplySuccess();
  }

  void Enable(EnableCompleter::Sync& completer) override {
    ASSERT_TRUE(expect_enable_);
    expect_enable_ = false;
    completer.ReplySuccess();
  }

  void ExpectSetConfig(fuchsia_hardware_pwm::wire::PwmConfig config) {
    std::unique_ptr<uint8_t[]> mode_config = std::make_unique<uint8_t[]>(config.mode_config.size());
    memcpy(mode_config.get(), config.mode_config.data(), config.mode_config.size());
    auto copy = config;
    copy.mode_config =
        fidl::VectorView<uint8_t>::FromExternal(mode_config.get(), config.mode_config.size());
    expect_configs_.push_back(std::move(copy));
    mode_config_buffers_.push_back(std::move(mode_config));
  }

  void ExpectEnable() { expect_enable_ = true; }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> BindServer() {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_pwm::Pwm>::Create();
    fidl::BindServer(async_get_default_dispatcher(), std::move(endpoints.server), this);
    return fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm>(std::move(endpoints.client));
  }

  void VerifyAndClear() {
    ASSERT_EQ(expect_configs_.size(), 0);
    ASSERT_EQ(mode_config_buffers_.size(), 0);
    ASSERT_FALSE(expect_enable_);
  }

 private:
  std::list<fuchsia_hardware_pwm::wire::PwmConfig> expect_configs_;
  std::list<std::unique_ptr<uint8_t[]>> mode_config_buffers_;
  bool expect_enable_ = false;
};

class AmlVoltageRegulatorTest : public zxtest::Test {
 public:
  void SetUp() override { EXPECT_OK(pwm_loop_.StartThread("pwm-servers")); }

  void TearDown() override {
    // Verify
    big_cluster_pwm_.SyncCall(&MockPwmServer::VerifyAndClear);
    little_cluster_pwm_.SyncCall(&MockPwmServer::VerifyAndClear);
    pwm_loop_.Shutdown();
  }

  void Create(uint32_t pid) {
    aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
    fuchsia_hardware_pwm::wire::PwmConfig cfg = {
        false, 1250, 43,
        fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
    switch (pid) {
      case 4: {  // Sherlock
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectEnable);
        cfg.duty_cycle = 43;
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);

        little_cluster_pwm_.SyncCall(&MockPwmServer::ExpectEnable);
        cfg.duty_cycle = 3;
        little_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
        break;
      }
      case 3: {  // Astro
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectEnable);
        cfg.duty_cycle = 13;
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
        break;
      }
      default:
        zxlogf(ERROR, "AmlVoltageRegulatorTest::Create: unsupported SOC PID %u", pid);
        return;
    }

    auto big_cluster_pwm_client = big_cluster_pwm_.SyncCall(&MockPwmServer::BindServer);
    auto little_cluster_pwm_client = little_cluster_pwm_.SyncCall(&MockPwmServer::BindServer);
    voltage_regulator_ = FakeAmlVoltageRegulator::Create(std::move(big_cluster_pwm_client),
                                                         std::move(little_cluster_pwm_client), pid);
    ASSERT_TRUE(voltage_regulator_ != nullptr);
  }

 protected:
  std::unique_ptr<FakeAmlVoltageRegulator> voltage_regulator_;

  // Mmio Regs and Regions
  async::Loop pwm_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<MockPwmServer> big_cluster_pwm_{pwm_loop_.dispatcher(),
                                                                      std::in_place};

  async_patterns::TestDispatcherBound<MockPwmServer> little_cluster_pwm_{pwm_loop_.dispatcher(),
                                                                         std::in_place};
};

TEST_F(AmlVoltageRegulatorTest, SherlockGetVoltageTest) {
  Create(4);
  uint32_t val = voltage_regulator_->GetVoltage(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 891000);
  val = voltage_regulator_->GetVoltage(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain);
  EXPECT_EQ(val, 1011000);
}

TEST_F(AmlVoltageRegulatorTest, AstroGetVoltageTest) {
  Create(3);
  uint32_t val = voltage_regulator_->GetVoltage(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 981000);
}

TEST_F(AmlVoltageRegulatorTest, SherlockSetVoltageTest) {
  Create(4);
  // SetBigClusterVoltage(761000)
  aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig cfg = {
      false, 1250, 53,
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 63;
  big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 73;
  big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 83;
  big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 86;
  big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  EXPECT_OK(voltage_regulator_->SetVoltage(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, 761000));
  uint32_t val = voltage_regulator_->GetVoltage(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 761000);

  // SetLittleClusterVoltage(911000)
  cfg.duty_cycle = 13;
  little_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 23;
  little_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 33;
  little_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 36;
  little_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  EXPECT_OK(voltage_regulator_->SetVoltage(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain, 911000));
  val = voltage_regulator_->GetVoltage(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain);
  EXPECT_EQ(val, 911000);
}

TEST_F(AmlVoltageRegulatorTest, AstroSetVoltageTest) {
  Create(3);
  // SetBigClusterVoltage(861000)
  aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig cfg = {
      false, 1250, 23,
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
  big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 33;
  big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 43;
  big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  cfg.duty_cycle = 53;
  big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);
  EXPECT_OK(voltage_regulator_->SetVoltage(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, 861000));
  uint32_t val = voltage_regulator_->GetVoltage(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 861000);
}

TEST_F(AmlVoltageRegulatorTest, GetVoltageUninitializedTest) {
  // Simply construct regulator without calling Init() to verify default values
  FakeAmlVoltageRegulator regulator;
  // Should return 0 (not crash or access out-of-bounds)
  EXPECT_EQ(
      regulator.GetVoltage(fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain), 0);
  EXPECT_EQ(
      regulator.GetVoltage(fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain),
      0);
}

TEST_F(AmlVoltageRegulatorTest, InvalidPowerDomainTest) {
  Create(4);  // Sherlock (big-little)

  // Cast an invalid value to PowerDomain to test bounds checks
  auto invalid_domain = static_cast<fuchsia_hardware_thermal::wire::PowerDomain>(99);

  EXPECT_EQ(voltage_regulator_->GetVoltage(invalid_domain), 0);
  EXPECT_EQ(voltage_regulator_->SetVoltage(invalid_domain, 891000), ZX_ERR_INVALID_ARGS);
}

TEST_F(AmlVoltageRegulatorTest, AstroUnsupportedDomainTest) {
  Create(3);  // Astro (single cluster)

  // Astro is not big-little, so little cluster is not supported
  EXPECT_EQ(voltage_regulator_->GetVoltage(
                fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain),
            0);
  EXPECT_EQ(voltage_regulator_->SetVoltage(
                fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain, 891000),
            ZX_ERR_NOT_SUPPORTED);
}

// CPU Frequency and Scaling
class FakeAmlCpuFrequency : public AmlCpuFrequency {
 public:
  static std::unique_ptr<FakeAmlCpuFrequency> Create(fdf::MmioBuffer hiu_mmio,
                                                     fdf::MmioBuffer mock_hiu_internal_mmio,
                                                     uint32_t pid) {
    const auto& config = (pid == 4 ? sherlock_thermal_config : astro_thermal_config);

    fbl::AllocChecker ac;
    auto test = fbl::make_unique_checked<FakeAmlCpuFrequency>(
        &ac, std::move(hiu_mmio), std::move(mock_hiu_internal_mmio), config, fake_thermal_info);
    if (!ac.check()) {
      return nullptr;
    }

    EXPECT_OK(test->Init());
    return test;
  }

  FakeAmlCpuFrequency(fdf::MmioBuffer hiu_mmio, fdf::MmioBuffer hiu_internal_mmio,
                      const fuchsia_hardware_thermal::wire::ThermalDeviceInfo& thermal_config,
                      const aml_thermal_info_t& thermal_info)
      : AmlCpuFrequency(std::move(hiu_mmio), std::move(hiu_internal_mmio), thermal_config,
                        thermal_info) {}
};

class AmlCpuFrequencyTest : public zxtest::Test {
 public:
  void SetUp() override {
    fbl::AllocChecker ac;

    mock_hiu_mmio_ =
        fbl::make_unique_checked<ddk_mock::MockMmioRegRegion>(&ac, sizeof(uint32_t), kRegSize);
    if (!ac.check()) {
      zxlogf(ERROR, "AmlCpuFrequencyTest::SetUp: mock_hiu_mmio_ alloc failed");
      return;
    }

    hiu_internal_mmio_ = fdf_testing::CreateMmioBuffer(kRegSize * sizeof(uint32_t));
    if (!ac.check()) {
      zxlogf(ERROR, "AmlCpuFrequencyTest::SetUp: hiu_internal_mmio_ alloc failed");
      return;
    }

    mock_hiu_internal_mmio_ = hiu_internal_mmio_->View(0);
    InitHiuInternalMmio();
  }

  void TearDown() override {
    // Verify
    mock_hiu_mmio_->VerifyAll();
  }

  void Create(uint32_t pid) {
    switch (pid) {
      case 4: {  // Sherlock
        // Big
        (*mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectRead(0x00000000);  // WaitForBusyCpu
        (*mock_hiu_mmio_)[520]
            .ExpectRead(0x00000000)
            .ExpectWrite(0x00010400);  // Dynamic mux 0 is in use
        // Little
        (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);  // WaitForBusyCpu
        (*mock_hiu_mmio_)[412]
            .ExpectRead(0x00000000)
            .ExpectWrite(0x00010400);  // Dynamic mux 0 is in use
        break;
      }
      case 3: {  // Astro
        // Big
        (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);  // WaitForBusyCpu
        (*mock_hiu_mmio_)[412]
            .ExpectRead(0x00000000)
            .ExpectWrite(0x00010400);  // Dynamic mux 0 is in use
        break;
      }
      default:
        zxlogf(ERROR, "AmlCpuFrequencyTest::Create: unsupported SOC PID %u", pid);
        return;
    }

    fdf::MmioBuffer hiu_mmio(mock_hiu_mmio_->GetMmioBuffer());
    cpufreq_scaling_ =
        FakeAmlCpuFrequency::Create(std::move(hiu_mmio), mock_hiu_internal_mmio_->View(0), pid);
    ASSERT_TRUE(cpufreq_scaling_ != nullptr);
  }

  void InitHiuInternalMmio() {
    for (uint32_t i = 0; i < hiu_internal_mmio_->get_size(); i += sizeof(uint32_t)) {
      hiu_internal_mmio_->Write32((1 << 31), i);
    }
  }

 protected:
  std::unique_ptr<FakeAmlCpuFrequency> cpufreq_scaling_;

  // Mmio Regs and Regions
  std::optional<fdf::MmioBuffer> hiu_internal_mmio_;
  std::unique_ptr<ddk_mock::MockMmioRegRegion> mock_hiu_mmio_;
  std::optional<fdf::MmioBuffer> mock_hiu_internal_mmio_;
};

TEST_F(AmlCpuFrequencyTest, SherlockGetFrequencyTest) {
  Create(4);
  InitHiuInternalMmio();
  uint32_t val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 1000000000);
  InitHiuInternalMmio();
  val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain);
  EXPECT_EQ(val, 1000000000);
}

TEST_F(AmlCpuFrequencyTest, AstroGetFrequencyTest) {
  Create(3);
  InitHiuInternalMmio();
  uint32_t val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 1000000000);
}

TEST_F(AmlCpuFrequencyTest, SherlockSetFrequencyTest0) {
  Create(4);
  // Big
  (*mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectWrite(0x00350400);
  InitHiuInternalMmio();
  EXPECT_OK(cpufreq_scaling_->SetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, 250000000));
  InitHiuInternalMmio();
  uint32_t val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 250000000);

  // Little
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00350400);
  InitHiuInternalMmio();
  EXPECT_OK(cpufreq_scaling_->SetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain, 250000000));
  InitHiuInternalMmio();
  val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain);
  EXPECT_EQ(val, 250000000);
}

TEST_F(AmlCpuFrequencyTest, SherlockSetFrequencyTest1) {
  Create(4);
  // Big
  (*mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectWrite(0x00000800);
  InitHiuInternalMmio();
  EXPECT_OK(cpufreq_scaling_->SetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, 1536000000));
  InitHiuInternalMmio();
  uint32_t val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 1536000000);

  (*mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectWrite(0x00010400);
  (*mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectWrite(0x00000800);
  InitHiuInternalMmio();
  EXPECT_OK(cpufreq_scaling_->SetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, 1494000000));
  InitHiuInternalMmio();
  val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 1494000000);

  // Little
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00000800);
  InitHiuInternalMmio();
  EXPECT_OK(cpufreq_scaling_->SetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain, 1200000000));
  InitHiuInternalMmio();
  val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain);
  EXPECT_EQ(val, 1200000000);

  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00010400);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00000800);
  InitHiuInternalMmio();
  EXPECT_OK(cpufreq_scaling_->SetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain, 1398000000));
  InitHiuInternalMmio();
  val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain);
  EXPECT_EQ(val, 1398000000);
}

TEST_F(AmlCpuFrequencyTest, AstroSetFrequencyTest0) {
  Create(3);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00350400);
  InitHiuInternalMmio();
  EXPECT_OK(cpufreq_scaling_->SetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, 250000000));
  InitHiuInternalMmio();
  uint32_t val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 250000000);
}

TEST_F(AmlCpuFrequencyTest, AstroSetFrequencyTest1) {
  Create(3);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00000800);
  InitHiuInternalMmio();
  EXPECT_OK(cpufreq_scaling_->SetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, 1536000000));
  InitHiuInternalMmio();
  uint32_t val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 1536000000);

  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x10400);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
  (*mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00000800);
  InitHiuInternalMmio();
  EXPECT_OK(cpufreq_scaling_->SetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain, 1494000000));
  InitHiuInternalMmio();
  val = cpufreq_scaling_->GetFrequency(
      fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain);
  EXPECT_EQ(val, 1494000000);
}

// Thermal
class FakeAmlThermal : public AmlThermal {
 public:
  static std::unique_ptr<FakeAmlThermal> Create(
      fdf::MmioBuffer tsensor_pll_mmio, fdf::MmioBuffer tsensor_trim_mmio,
      fdf::MmioBuffer tsensor_hiu_mmio,
      fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> big_cluster_pwm,
      fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm> little_cluster_pwm,
      fdf::MmioBuffer cpufreq_scaling_hiu_mmio,
      fdf::MmioBuffer cpufreq_scaling_mock_hiu_internal_mmio, uint32_t pid) {
    fbl::AllocChecker ac;

    const auto& config = (pid == 4 ? sherlock_thermal_config
                                   : (pid == 5 ? nelson_thermal_config : astro_thermal_config));
    const auto& info = (pid == 5 ? nelson_thermal_info : fake_thermal_info);

    // Temperature Sensor
    auto tsensor = fbl::make_unique_checked<AmlTSensor>(&ac, std::move(tsensor_pll_mmio),
                                                        std::move(tsensor_trim_mmio),
                                                        std::move(tsensor_hiu_mmio));
    if (!ac.check()) {
      return nullptr;
    }
    EXPECT_OK(tsensor->InitSensor(config));

    // Voltage Regulator
    zx_status_t status = ZX_OK;
    auto voltage_regulator = fbl::make_unique_checked<AmlVoltageRegulator>(&ac);
    if (!ac.check() || (status != ZX_OK)) {
      return nullptr;
    }
    EXPECT_OK(voltage_regulator->Init(std::move(big_cluster_pwm), std::move(little_cluster_pwm),
                                      config, &info));

    // CPU Frequency and Scaling
    auto cpufreq_scaling = fbl::make_unique_checked<AmlCpuFrequency>(
        &ac, std::move(cpufreq_scaling_hiu_mmio), std::move(cpufreq_scaling_mock_hiu_internal_mmio),
        config, fake_thermal_info);
    if (!ac.check()) {
      return nullptr;
    }
    EXPECT_OK(cpufreq_scaling->Init());

    auto test = fbl::make_unique_checked<FakeAmlThermal>(
        &ac, std::move(tsensor), std::move(voltage_regulator), std::move(cpufreq_scaling), config);
    if (!ac.check()) {
      return nullptr;
    }

    // SetTarget
    EXPECT_OK(test->SetTarget(config.trip_point_info[0].big_cluster_dvfs_opp,
                              fuchsia_hardware_thermal::wire::PowerDomain::kBigClusterPowerDomain));
    if (config.big_little) {
      EXPECT_OK(
          test->SetTarget(config.trip_point_info[0].little_cluster_dvfs_opp,
                          fuchsia_hardware_thermal::wire::PowerDomain::kLittleClusterPowerDomain));
    }

    return test;
  }

  void DdkRelease() { delete this; }

  FakeAmlThermal(std::unique_ptr<thermal::AmlTSensor> tsensor,
                 std::unique_ptr<thermal::AmlVoltageRegulator> voltage_regulator,
                 std::unique_ptr<thermal::AmlCpuFrequency> cpufreq_scaling,
                 const fuchsia_hardware_thermal::wire::ThermalDeviceInfo& thermal_config)
      : AmlThermal(nullptr, std::move(tsensor), std::move(voltage_regulator),
                   std::move(cpufreq_scaling), thermal_config) {}
};

class AmlThermalTest : public zxtest::Test {
 public:
  void SetUp() override {
    fbl::AllocChecker ac;

    // Temperature Sensor
    tsensor_mock_pll_mmio_ =
        fbl::make_unique_checked<ddk_mock::MockMmioRegRegion>(&ac, sizeof(uint32_t), kRegSize);
    if (!ac.check()) {
      zxlogf(ERROR, "AmlThermalTest::SetUp: mock_pll_mmio_ alloc failed");
      return;
    }
    tsensor_mock_trim_mmio_ =
        fbl::make_unique_checked<ddk_mock::MockMmioRegRegion>(&ac, sizeof(uint32_t), kRegSize);
    if (!ac.check()) {
      zxlogf(ERROR, "AmlThermalTest::SetUp: mock_trim_mmio_ alloc failed");
      return;
    }
    tsensor_mock_hiu_mmio_ =
        fbl::make_unique_checked<ddk_mock::MockMmioRegRegion>(&ac, sizeof(uint32_t), kRegSize);
    if (!ac.check()) {
      zxlogf(ERROR, "AmlThermalTest::SetUp: mock_hiu_mmio_ alloc failed");
      return;
    }
    (*tsensor_mock_trim_mmio_)[0].ExpectRead(0x00000000);                             // trim_info_
    (*tsensor_mock_hiu_mmio_)[(0x64 << 2)].ExpectWrite(0x130U);                       // set clock
    (*tsensor_mock_pll_mmio_)[(0x1 << 2)].ExpectRead(0x00000000).ExpectWrite(0x63B);  // sensor ctl
    (*tsensor_mock_pll_mmio_)[(0x1 << 2)]
        .ExpectRead(0x00000000)  // clear IRQs
        .ExpectWrite(0x00FF0000);
    (*tsensor_mock_pll_mmio_)[(0x1 << 2)]
        .ExpectRead(0x00000000)  // clear IRQs
        .ExpectWrite(0x00000000);
    (*tsensor_mock_pll_mmio_)[(0x1 << 2)]
        .ExpectRead(0x00000000)  // enable IRQs
        .ExpectWrite(0x0F008000);

    // CPU Frequency and Scaling
    cpufreq_scaling_mock_hiu_mmio_ =
        fbl::make_unique_checked<ddk_mock::MockMmioRegRegion>(&ac, sizeof(uint32_t), kRegSize);
    if (!ac.check()) {
      zxlogf(ERROR, "AmlThermalTest::SetUp: cpufreq_scaling_mock_hiu_mmio_ alloc failed");
      return;
    }
    cpufreq_scaling_hiu_internal_mmio_ = fdf_testing::CreateMmioBuffer(kRegSize * sizeof(uint32_t));
    cpufreq_scaling_mock_hiu_internal_mmio_ = cpufreq_scaling_hiu_internal_mmio_->View(0);
    InitHiuInternalMmio();

    EXPECT_OK(pwm_loop_.StartThread("pwm-servers"));
  }

  void TearDown() override {
    // Verify
    tsensor_mock_pll_mmio_->VerifyAll();
    tsensor_mock_trim_mmio_->VerifyAll();
    tsensor_mock_hiu_mmio_->VerifyAll();

    big_cluster_pwm_.SyncCall(&MockPwmServer::VerifyAndClear);
    little_cluster_pwm_.SyncCall(&MockPwmServer::VerifyAndClear);
    cpufreq_scaling_mock_hiu_mmio_->VerifyAll();

    pwm_loop_.Shutdown();

    // Tear down
    thermal_device_ = nullptr;
  }

  void Create(uint32_t pid) {
    ddk_mock::MockMmioRegRegion& tsensor_mmio = *tsensor_mock_pll_mmio_;

    aml_pwm::mode_config on = {aml_pwm::Mode::kOn, {}};
    fuchsia_hardware_pwm::wire::PwmConfig cfg = {
        false, 1250, 43,
        fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&on), sizeof(on))};
    switch (pid) {
      case 4: {  // Sherlock
        // Voltage Regulator
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectEnable);
        cfg.duty_cycle = 43;
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);

        little_cluster_pwm_.SyncCall(&MockPwmServer::ExpectEnable);
        cfg.duty_cycle = 3;
        little_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);

        // CPU Frequency and Scaling
        // Big
        (*cpufreq_scaling_mock_hiu_mmio_)[520]
            .ExpectRead(0x00000000)
            .ExpectRead(0x00000000);  // WaitForBusyCpu
        (*cpufreq_scaling_mock_hiu_mmio_)[520]
            .ExpectRead(0x00000000)
            .ExpectWrite(0x00010400);  // Dynamic mux 0 is in use
        // Little
        (*cpufreq_scaling_mock_hiu_mmio_)[412]
            .ExpectRead(0x00000000)
            .ExpectRead(0x00000000);  // WaitForBusyCpu
        (*cpufreq_scaling_mock_hiu_mmio_)[412]
            .ExpectRead(0x00000000)
            .ExpectWrite(0x00010400);  // Dynamic mux 0 is in use

        // SetTarget
        (*cpufreq_scaling_mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectRead(0x00000000);
        (*cpufreq_scaling_mock_hiu_mmio_)[520].ExpectRead(0x00000000).ExpectWrite(0x00000800);
        (*cpufreq_scaling_mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
        (*cpufreq_scaling_mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00000800);

        // InitTripPoints
        tsensor_mmio[(0x5 << 2)].ExpectWrite(0x00027E);  // set thresholds 4, rise
        tsensor_mmio[(0x7 << 2)].ExpectWrite(0x000272);  // set thresholds 4, fall
        tsensor_mmio[(0x5 << 2)].ExpectWrite(0x27227E);  // set thresholds 3, rise
        tsensor_mmio[(0x7 << 2)].ExpectWrite(0x268272);  // set thresholds 3, fall
        tsensor_mmio[(0x4 << 2)].ExpectWrite(0x00025A);  // set thresholds 2, rise
        tsensor_mmio[(0x6 << 2)].ExpectWrite(0x000251);  // set thresholds 2, fall
        tsensor_mmio[(0x4 << 2)].ExpectWrite(0x25025A);  // set thresholds 1, rise
        tsensor_mmio[(0x6 << 2)].ExpectWrite(0x245251);  // set thresholds 1, fall

        break;
      }
      case 3: {  // Astro
        // Voltage Regulator
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectEnable);
        cfg.duty_cycle = 13;
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);

        // CPU Frequency and Scaling
        (*cpufreq_scaling_mock_hiu_mmio_)[412]
            .ExpectRead(0x00000000)
            .ExpectRead(0x00000000);  // WaitForBusyCpu
        (*cpufreq_scaling_mock_hiu_mmio_)[412]
            .ExpectRead(0x00000000)
            .ExpectWrite(0x00010400);  // Dynamic mux 0 is in use

        // SetTarget
        (*cpufreq_scaling_mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
        (*cpufreq_scaling_mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00000800);

        // InitTripPoints
        tsensor_mmio[(0x5 << 2)].ExpectWrite(0x000272);  // set thresholds 4, rise
        tsensor_mmio[(0x7 << 2)].ExpectWrite(0x000268);  // set thresholds 4, fall
        tsensor_mmio[(0x5 << 2)].ExpectWrite(0x266272);  // set thresholds 3, rise
        tsensor_mmio[(0x7 << 2)].ExpectWrite(0x25c268);  // set thresholds 3, fall
        tsensor_mmio[(0x4 << 2)].ExpectWrite(0x00025A);  // set thresholds 2, rise
        tsensor_mmio[(0x6 << 2)].ExpectWrite(0x000251);  // set thresholds 2, fall
        tsensor_mmio[(0x4 << 2)].ExpectWrite(0x25025A);  // set thresholds 1, rise
        tsensor_mmio[(0x6 << 2)].ExpectWrite(0x245251);  // set thresholds 1, fall
        break;
      }
      case 5: {  // Nelson
        // Voltage Regulator
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectEnable);
        cfg.period_ns = 1500;
        cfg.duty_cycle = 23;
        big_cluster_pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, cfg);

        // CPU Frequency and Scaling
        (*cpufreq_scaling_mock_hiu_mmio_)[412]
            .ExpectRead(0x00000000)
            .ExpectRead(0x00000000);  // WaitForBusyCpu
        (*cpufreq_scaling_mock_hiu_mmio_)[412]
            .ExpectRead(0x00000000)
            .ExpectWrite(0x00010400);  // Dynamic mux 0 is in use

        // SetTarget
        (*cpufreq_scaling_mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectRead(0x00000000);
        (*cpufreq_scaling_mock_hiu_mmio_)[412].ExpectRead(0x00000000).ExpectWrite(0x00000800);

        // InitTripPoints
        tsensor_mmio[(0x5 << 2)].ExpectWrite(0x00029D);  // set thresholds 4, rise
        tsensor_mmio[(0x7 << 2)].ExpectWrite(0x000299);  // set thresholds 4, fall
        tsensor_mmio[(0x5 << 2)].ExpectWrite(0x26329D);  // set thresholds 3, rise
        tsensor_mmio[(0x7 << 2)].ExpectWrite(0x24A299);  // set thresholds 3, fall
        tsensor_mmio[(0x4 << 2)].ExpectWrite(0x000257);  // set thresholds 2, rise
        tsensor_mmio[(0x6 << 2)].ExpectWrite(0x00023F);  // set thresholds 2, fall
        tsensor_mmio[(0x4 << 2)].ExpectWrite(0x236257);  // set thresholds 1, rise
        tsensor_mmio[(0x6 << 2)].ExpectWrite(0x21F23F);  // set thresholds 1, fall
        break;
      }
      default:
        zxlogf(ERROR, "AmlThermalTest::Create: unsupported SOC PID %u", pid);
        return;
    }

    fdf::MmioBuffer tsensor_pll_mmio(tsensor_mock_pll_mmio_->GetMmioBuffer());
    fdf::MmioBuffer tsensor_trim_mmio(tsensor_mock_trim_mmio_->GetMmioBuffer());
    fdf::MmioBuffer tsensor_hiu_mmio(tsensor_mock_hiu_mmio_->GetMmioBuffer());
    auto big_cluster_pwm_client = big_cluster_pwm_.SyncCall(&MockPwmServer::BindServer);
    auto little_cluster_pwm_client = little_cluster_pwm_.SyncCall(&MockPwmServer::BindServer);
    fdf::MmioBuffer cpufreq_scaling_hiu_mmio(cpufreq_scaling_mock_hiu_mmio_->GetMmioBuffer());
    thermal_device_ = FakeAmlThermal::Create(
        std::move(tsensor_pll_mmio), std::move(tsensor_trim_mmio), std::move(tsensor_hiu_mmio),
        std::move(big_cluster_pwm_client), std::move(little_cluster_pwm_client),
        std::move(cpufreq_scaling_hiu_mmio), cpufreq_scaling_mock_hiu_internal_mmio_->View(0), pid);
    ASSERT_TRUE(thermal_device_ != nullptr);
  }

  void InitHiuInternalMmio() {
    for (uint32_t i = 0; i < kRegSize; i += sizeof(uint32_t)) {
      cpufreq_scaling_hiu_internal_mmio_->Write32((1 << 31), i);
    }
  }

 protected:
  std::unique_ptr<FakeAmlThermal> thermal_device_;

  // Temperature Sensor
  std::unique_ptr<ddk_mock::MockMmioRegRegion> tsensor_mock_pll_mmio_;
  std::unique_ptr<ddk_mock::MockMmioRegRegion> tsensor_mock_trim_mmio_;
  std::unique_ptr<ddk_mock::MockMmioRegRegion> tsensor_mock_hiu_mmio_;

  // Voltage Regulator
  async::Loop pwm_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<MockPwmServer> big_cluster_pwm_{pwm_loop_.dispatcher(),
                                                                      std::in_place};
  async_patterns::TestDispatcherBound<MockPwmServer> little_cluster_pwm_{pwm_loop_.dispatcher(),
                                                                         std::in_place};

  // CPU Frequency and Scaling
  std::optional<fdf::MmioBuffer> cpufreq_scaling_hiu_internal_mmio_;
  std::unique_ptr<ddk_mock::MockMmioRegRegion> cpufreq_scaling_mock_hiu_mmio_;
  std::optional<fdf::MmioBuffer> cpufreq_scaling_mock_hiu_internal_mmio_;
};

TEST_F(AmlThermalTest, SherlockInitTest) {
  Create(4);
  ASSERT_TRUE(true);
}

TEST_F(AmlThermalTest, AstroInitTest) {
  Create(3);
  ASSERT_TRUE(true);
}

TEST_F(AmlThermalTest, NelsonInitTest) { Create(5); }

}  // namespace thermal
