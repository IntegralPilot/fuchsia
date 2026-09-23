// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.clock/cpp/wire_test_base.h>
#include <lib/ddk/platform-defs.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <optional>

#include <fake-mmio-reg/fake-mmio-reg.h>
#include <gtest/gtest.h>
#include <sdk/lib/inspect/testing/cpp/inspect.h>

#include "src/devices/cpu/drivers/aml-cpu/aml-cpu-driver.h"
#include "src/lib/testing/predicates/status.h"

namespace amlogic_cpu {

using CpuCtrlClient = fidl::WireSyncClient<fuchsia_hardware_cpu_ctrl::Device>;

#define MHZ(x) ((x) * 1000000)

constexpr uint32_t kPdArmA53 = 1;

const std::vector<fuchsia_hardware_amlogic_metadata::PerformanceDomain> kPerformanceDomains{
    {{.id = kPdArmA53, .core_count = 4, .relative_performance = 255, .name = "S905D2 ARM A53"}},
};

const std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> kOperatingPoints{
    {{.freq_hz = 100'000'000, .volt_uv = 731'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 250'000'000, .volt_uv = 731'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 500'000'000, .volt_uv = 731'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 667'000'000, .volt_uv = 731'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 1'000'000'000, .volt_uv = 731'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 1'200'000'000, .volt_uv = 731'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 1'398'000'000, .volt_uv = 761'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 1'512'000'000, .volt_uv = 791'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 1'608'000'000, .volt_uv = 831'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 1'704'000'000, .volt_uv = 861'000, .pd_id = kPdArmA53}},
    {{.freq_hz = 1'896'000'000, .volt_uv = 1'022'000, .pd_id = kPdArmA53}}};

const fuchsia_hardware_amlogic_metadata::CpuMetadata kMetadata(
    {.performance_domains = kPerformanceDomains, .operating_points = kOperatingPoints});

const std::vector<fuchsia_hardware_amlogic_metadata::PerformanceDomain> kTestPerformanceDomains{
    {{.id = kPdArmA53, .core_count = 1, .relative_performance = 0, .name = "testpd"}}};

const std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> kTestOperatingPoints{
    {{.freq_hz = MHZ(10), .volt_uv = 15000, .pd_id = kPdArmA53}},
    {{.freq_hz = MHZ(9), .volt_uv = 13500, .pd_id = kPdArmA53}},
    {{.freq_hz = MHZ(8), .volt_uv = 12000, .pd_id = kPdArmA53}},
    {{.freq_hz = MHZ(7), .volt_uv = 10500, .pd_id = kPdArmA53}},
    {{.freq_hz = MHZ(6), .volt_uv = 9000, .pd_id = kPdArmA53}},
    {{.freq_hz = MHZ(5), .volt_uv = 7500, .pd_id = kPdArmA53}},
    {{.freq_hz = MHZ(4), .volt_uv = 6000, .pd_id = kPdArmA53}},
    {{.freq_hz = MHZ(3), .volt_uv = 4500, .pd_id = kPdArmA53}},
    {{.freq_hz = MHZ(2), .volt_uv = 3000, .pd_id = kPdArmA53}},
    {{.freq_hz = MHZ(1), .volt_uv = 1500, .pd_id = kPdArmA53}},
};

const fuchsia_hardware_amlogic_metadata::CpuMetadata kTestMetadata(
    {.performance_domains = kTestPerformanceDomains, .operating_points = kTestOperatingPoints});

class FakeMmio {
 public:
  FakeMmio() : mmio_(sizeof(uint32_t), kRegCount) {
    mmio_[kCpuVersionOffset].SetReadCallback([]() { return kCpuVersion; });
  }

  fdf::MmioBuffer mmio() { return mmio_.GetMmioBuffer(); }

 private:
  static constexpr size_t kCpuVersionOffset = 0x220;
  static constexpr size_t kRegCount = kCpuVersionOffset / sizeof(uint32_t) + 1;

  constexpr static uint64_t kCpuVersion = 0x28200b02;

  ddk_fake::FakeMmioRegRegion mmio_;
};

class TestClockDevice : public fidl::testing::WireTestBase<fuchsia_hardware_clock::Clock> {
 public:
  fuchsia_hardware_clock::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_clock::Service::InstanceHandler({
        .clock = binding_group_.CreateHandler(
            this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure),
    });
  }

  void Enable(EnableCompleter::Sync& completer) override {
    enabled_ = true;
    completer.ReplySuccess();
  }

  void Disable(DisableCompleter::Sync& completer) override {
    enabled_ = false;
    completer.ReplySuccess();
  }

  void IsEnabled(IsEnabledCompleter::Sync& completer) override { completer.ReplySuccess(enabled_); }

  void SetRate(SetRateRequestView request, SetRateCompleter::Sync& completer) override {
    rate_.emplace(request->hz);
    completer.ReplySuccess();
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) final {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  fidl::ClientEnd<fuchsia_hardware_clock::Clock> Connect() {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_clock::Clock>::Create();
    binding_group_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                              std::move(endpoints.server), this, fidl::kIgnoreBindingClosure);
    return std::move(endpoints.client);
  }

  bool enabled() const { return enabled_; }
  std::optional<uint32_t> rate() const { return rate_; }

 private:
  bool enabled_ = false;
  std::optional<uint32_t> rate_;
  fidl::ServerBindingGroup<fuchsia_hardware_clock::Clock> binding_group_;
};

class TestPowerDevice : public fidl::WireServer<fuchsia_hardware_power::Device> {
 public:
  fuchsia_hardware_power::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_power::Service::InstanceHandler({
        .device = binding_group_.CreateHandler(
            this, fdf::Dispatcher::GetCurrent()->async_dispatcher(), fidl::kIgnoreBindingClosure),
    });
  }

  void ConnectRequest(fidl::ServerEnd<fuchsia_hardware_power::Device> request) {
    binding_group_.AddBinding(fdf::Dispatcher::GetCurrent()->async_dispatcher(), std::move(request),
                              this, fidl::kIgnoreBindingClosure);
  }

  fidl::ClientEnd<fuchsia_hardware_power::Device> Connect() {
    auto endpoints = fidl::Endpoints<fuchsia_hardware_power::Device>::Create();
    ConnectRequest(std::move(endpoints.server));
    return std::move(endpoints.client);
  }

  void RegisterPowerDomain(RegisterPowerDomainRequestView request,
                           RegisterPowerDomainCompleter::Sync& completer) override {
    min_needed_voltage_ = request->min_needed_voltage;
    max_supported_voltage_ = request->max_supported_voltage;
    completer.ReplySuccess();
  }

  void UnregisterPowerDomain(UnregisterPowerDomainCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }
  void GetPowerDomainStatus(GetPowerDomainStatusCompleter::Sync& completer) override {
    completer.ReplySuccess(fuchsia_hardware_power::wire::PowerDomainStatus::kEnabled);
  }
  void GetSupportedVoltageRange(GetSupportedVoltageRangeCompleter::Sync& completer) override {
    completer.ReplySuccess(min_voltage_, max_voltage_);
  }

  void RequestVoltage(RequestVoltageRequestView request,
                      RequestVoltageCompleter::Sync& completer) override {
    if (fault_voltage_) {
      return completer.ReplySuccess(*fault_voltage_);
    }

    voltage_ = request->voltage;
    completer.ReplySuccess(voltage_);
  }

  void GetCurrentVoltage(GetCurrentVoltageRequestView request,
                         GetCurrentVoltageCompleter::Sync& completer) override {
    completer.ReplySuccess(voltage_);
  }

  void WritePmicCtrlReg(WritePmicCtrlRegRequestView request,
                        WritePmicCtrlRegCompleter::Sync& completer) override {
    completer.ReplySuccess();
  }
  void ReadPmicCtrlReg(ReadPmicCtrlRegRequestView request,
                       ReadPmicCtrlRegCompleter::Sync& completer) override {
    completer.ReplySuccess(1);
  }

  void SetSupportedVoltageRange(uint32_t min_voltage, uint32_t max_voltage) {
    min_voltage_ = min_voltage;
    max_voltage_ = max_voltage;
  }

  void SetVoltage(uint32_t voltage) { voltage_ = voltage; }
  void SetFaultVoltage(uint32_t voltage) { fault_voltage_ = voltage; }
  void ResetFaultVoltage() { fault_voltage_ = std::nullopt; }

  uint32_t voltage() const { return voltage_; }
  uint32_t min_needed_voltage() const { return min_needed_voltage_; }
  uint32_t max_supported_voltage() const { return max_supported_voltage_; }

 private:
  std::optional<uint32_t> fault_voltage_ = std::nullopt;
  uint32_t voltage_ = 0;
  uint32_t min_voltage_ = 0;
  uint32_t max_voltage_ = 0;
  uint32_t min_needed_voltage_ = 0;
  uint32_t max_supported_voltage_ = 0;
  fidl::ServerBindingGroup<fuchsia_hardware_power::Device> binding_group_;
};

class AmlCpuEnvironment : public fdf_testing::Environment {
 public:
  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) {
    auto dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    std::map<uint32_t, fdf_fake::Mmio> mmios;
    mmios[0] = mmio_.mmio();

    pdev_server_.SetConfig(fdf_fake::FakePDev::Config{
        .mmios = std::move(mmios),
        .device_info =
            fdf::PDev::DeviceInfo{
                .pid = PDEV_PID_ASTRO,
            },
    });
    EXPECT_OK(to_driver_vfs
                  .AddService<fuchsia_hardware_platform_device::Service>(
                      pdev_server_.GetInstanceHandler(dispatcher), "pdev")
                  .status_value());

    power_server_.SetVoltage(0);
    EXPECT_OK(to_driver_vfs
                  .AddService<fuchsia_hardware_power::Service>(power_server_.GetInstanceHandler(),
                                                               "power-01")
                  .status_value());

    EXPECT_OK(to_driver_vfs
                  .AddService<fuchsia_hardware_clock::Service>(
                      clock_pll_div16_server_.GetInstanceHandler(), "sys-pll-div16")
                  .status_value());

    EXPECT_OK(to_driver_vfs
                  .AddService<fuchsia_hardware_clock::Service>(
                      clock_cpu_div16_server_.GetInstanceHandler(), "sys-cpu-div16")
                  .status_value());

    EXPECT_OK(to_driver_vfs
                  .AddService<fuchsia_hardware_clock::Service>(
                      clock_cpu_scaler_server_.GetInstanceHandler(), "sys-cpu-big-clk")
                  .status_value());
    return zx::ok();
  }

  FakeMmio mmio_;
  fdf_fake::FakePDev pdev_server_;
  TestPowerDevice power_server_;
  TestClockDevice clock_pll_div16_server_;
  TestClockDevice clock_cpu_div16_server_;
  TestClockDevice clock_cpu_scaler_server_;
};

class AmlCpuBindingConfiguration final {
 public:
  using DriverType = AmlCpuDriver;
  using EnvironmentType = AmlCpuEnvironment;
};

class AmlCpuTest : public ::testing::Test {
 public:
  void StartWithMetadata(const fuchsia_hardware_amlogic_metadata::CpuMetadata& metadata) {
    // Notes on AmlCpu Initialization:
    //  + Should enable the CPU and PLL clocks.
    //  + Should initially assume that the device is in it's opp.
    //  + Should configure the device to it's highest opp.
    const fuchsia_hardware_amlogic_metadata::OperatingPoint& slowest =
        metadata.operating_points().value().front();
    const fuchsia_hardware_amlogic_metadata::OperatingPoint& fastest =
        metadata.operating_points().value().back();

    driver_test().RunInEnvironmentTypeContext([slowest, fastest](AmlCpuEnvironment& env) {
      // The DUT should initialize.
      env.power_server_.SetSupportedVoltageRange(slowest.volt_uv(), fastest.volt_uv());

      // The DUT scales up to the fastest available opp.
      env.power_server_.SetVoltage(fastest.volt_uv());
    });

    driver_test().RunInEnvironmentTypeContext([&metadata](AmlCpuEnvironment& env) {
      env.pdev_server_.AddFidlMetadata(
          fuchsia_hardware_amlogic_metadata::CpuMetadata::kSerializableName, metadata);
    });
    ASSERT_OK(driver_test().StartDriver().status_value());

    driver_test().RunInEnvironmentTypeContext([slowest, fastest](AmlCpuEnvironment& env) {
      ASSERT_EQ(env.power_server_.min_needed_voltage(), slowest.volt_uv());
      ASSERT_EQ(env.power_server_.max_supported_voltage(), fastest.volt_uv());
    });

    driver_test().RunInNodeContext(
        [](fdf_testing::TestNode& node) { ASSERT_EQ(node.children().size(), 0u); });
  }

  void ConnectToCpuCtrl(const fuchsia_hardware_amlogic_metadata::PerformanceDomain& perf_domain) {
    auto device =
        driver_test().Connect<fuchsia_hardware_cpu_ctrl::Service::Device>(perf_domain.name());
    EXPECT_OK(device.status_value());
    cpu_ctrl_.Bind(std::move(device.value()));
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::BackgroundDriverTest<AmlCpuBindingConfiguration>& driver_test() {
    return driver_test_;
  }
  fdf_testing::BackgroundDriverTest<AmlCpuBindingConfiguration> driver_test_;

  CpuCtrlClient cpu_ctrl_;
};

TEST_F(AmlCpuTest, TrivialBinding) { StartWithMetadata(kMetadata); }

TEST_F(AmlCpuTest, UnorderedOperatingPoints) {
  // AML CPU's bind hook expects that all operating points are strictly
  // ordered and it should handle the situation where there are duplicate
  // frequencies.
  const std::vector<fuchsia_hardware_amlogic_metadata::OperatingPoint> kOperatingPoints = {
      {{.freq_hz = MHZ(1), .volt_uv = 200'000, .pd_id = kPdArmA53}},
      {{.freq_hz = MHZ(1), .volt_uv = 100'000, .pd_id = kPdArmA53}},
      {{.freq_hz = MHZ(1), .volt_uv = 300'000, .pd_id = kPdArmA53}},
  };

  const fuchsia_hardware_amlogic_metadata::CpuMetadata kMetadata(
      {.performance_domains = kPerformanceDomains, .operating_points = kOperatingPoints});

  StartWithMetadata(kMetadata);

  ConnectToCpuCtrl(kPerformanceDomains[0]);

  auto out = cpu_ctrl_->SetCurrentOperatingPoint(0);
  EXPECT_OK(out.status());
  EXPECT_EQ(out->value()->out_opp, 0ul);

  driver_test().RunInEnvironmentTypeContext([](AmlCpuEnvironment& env) {
    uint32_t voltage = env.power_server_.voltage();
    EXPECT_EQ(voltage, 300'000u);
  });
}

TEST_F(AmlCpuTest, TestGetOperatingPointInfo) {
  StartWithMetadata(kTestMetadata);
  ConnectToCpuCtrl(kTestPerformanceDomains[0]);

  auto opp_size = kTestOperatingPoints.size();

  // Make sure that we can get information about all the supported opps.
  for (size_t i = 0; i < opp_size; i++) {
    const uint32_t opp = static_cast<uint32_t>(i);
    auto oppInfo = cpu_ctrl_->GetOperatingPointInfo(opp);

    // First, make sure there were no transport errors.
    ASSERT_OK(oppInfo.status());

    // Then make sure that the driver accepted the call.
    ASSERT_FALSE(oppInfo->is_error());

    // Then make sure that we're getting the expected frequency and voltage values.
    EXPECT_EQ(oppInfo->value()->info.frequency_hz, kTestOperatingPoints[i].freq_hz());
    EXPECT_EQ(oppInfo->value()->info.voltage_uv, kTestOperatingPoints[i].volt_uv());
  }

  // Make sure that we can't get any information about opps that don't
  // exist.
  for (size_t i = opp_size; i < opp_size + 10; i++) {
    const uint32_t opp = static_cast<uint32_t>(i);
    auto oppInfo = cpu_ctrl_->GetOperatingPointInfo(opp);

    // Even if it's an unsupported opp, we still expect the transport to
    // deliver the message successfully.
    ASSERT_OK(oppInfo.status());

    // Make sure that the driver returns an error, however.
    EXPECT_TRUE(oppInfo->is_error());
  }
}

TEST_F(AmlCpuTest, TestSetCurrentOperatingPoint) {
  StartWithMetadata(kTestMetadata);
  ConnectToCpuCtrl(kTestPerformanceDomains[0]);

  // Scale to the lowest opp.
  const uint32_t min_opp_index = static_cast<uint32_t>(kTestOperatingPoints.size() - 1);
  const fuchsia_hardware_amlogic_metadata::OperatingPoint& min_opp =
      kTestOperatingPoints[min_opp_index];

  driver_test().RunInEnvironmentTypeContext(
      [&min_opp](AmlCpuEnvironment& env) { env.power_server_.SetVoltage(min_opp.volt_uv()); });

  auto min_result = cpu_ctrl_->SetCurrentOperatingPoint(min_opp_index);
  EXPECT_OK(min_result.status());
  EXPECT_TRUE(min_result->is_ok());
  EXPECT_EQ(min_result->value()->out_opp, min_opp_index);
  driver_test().RunInEnvironmentTypeContext([&min_opp](AmlCpuEnvironment& env) {
    auto rate = env.clock_cpu_scaler_server_.rate();
    ASSERT_TRUE(rate.has_value());
    ASSERT_EQ(rate.value(), min_opp.freq_hz());
  });

  // Check that we get the same value back from GetCurrentOperatingPoint
  auto min_result_again = cpu_ctrl_->GetCurrentOperatingPoint();
  EXPECT_OK(min_result_again.status());
  EXPECT_EQ(min_result_again.value().out_opp, min_opp_index);

  // Scale to the highest opp.
  const uint32_t max_opp_index = 0;
  const fuchsia_hardware_amlogic_metadata::OperatingPoint& max_opp =
      kTestOperatingPoints[max_opp_index];

  driver_test().RunInEnvironmentTypeContext(
      [&max_opp](AmlCpuEnvironment& env) { env.power_server_.SetVoltage(max_opp.volt_uv()); });

  auto max_result = cpu_ctrl_->SetCurrentOperatingPoint(max_opp_index);
  EXPECT_OK(max_result.status());
  EXPECT_TRUE(max_result->is_ok());
  EXPECT_EQ(max_result->value()->out_opp, max_opp_index);
  driver_test().RunInEnvironmentTypeContext([&max_opp](AmlCpuEnvironment& env) {
    auto rate = env.clock_cpu_scaler_server_.rate();
    ASSERT_TRUE(rate.has_value());
    ASSERT_EQ(rate.value(), max_opp.freq_hz());
  });

  // Check that we get the same value back from GetCurrentOperatingPoint
  auto max_result_again = cpu_ctrl_->GetCurrentOperatingPoint();
  EXPECT_OK(max_result_again.status());
  EXPECT_EQ(max_result_again.value().out_opp, max_opp_index);

  // Set to the opp that we're already at and make sure that it's a no-op.
  auto same_result = cpu_ctrl_->SetCurrentOperatingPoint(max_opp_index);
  EXPECT_OK(same_result.status());
  EXPECT_TRUE(same_result->is_ok());
  EXPECT_EQ(same_result->value()->out_opp, max_opp_index);

  // Check that we get the same value back from GetCurrentOperatingPoint
  auto same_result_again = cpu_ctrl_->GetCurrentOperatingPoint();
  EXPECT_OK(same_result_again.status());
  EXPECT_EQ(same_result_again.value().out_opp, max_opp_index);
}

TEST_F(AmlCpuTest, TestCpuInfo) {
  using namespace inspect::testing;

  StartWithMetadata(kTestMetadata);

  driver_test().RunInDriverContext([](AmlCpuDriver& driver) {
    auto& dut = driver.performance_domains().front();

    auto hierarchy = inspect::ReadFromVmo(dut->Inspector().DuplicateVmo());
    EXPECT_TRUE(hierarchy.is_ok());
    auto* cpu_info = hierarchy.value().GetByPath({"cpu_info_service"});
    EXPECT_TRUE(cpu_info);

    // cpu_major_revision : 40
    EXPECT_THAT(
        *cpu_info,
        NodeMatches(AllOf(NameMatches("cpu_info_service"),
                          PropertyList(testing::IsSupersetOf({UintIs("cpu_major_revision", 40),
                                                              UintIs("cpu_minor_revision", 11),
                                                              UintIs("cpu_package_id", 2)})))));
  });
}

TEST_F(AmlCpuTest, TestGetOperatingPointCount) {
  StartWithMetadata(kTestMetadata);
  ConnectToCpuCtrl(kTestPerformanceDomains[0]);

  auto resp = cpu_ctrl_->GetOperatingPointCount();

  ASSERT_OK(resp.status());

  EXPECT_EQ(resp.value()->count, kTestOperatingPoints.size());
}

TEST_F(AmlCpuTest, TestGetLogicalCoreCount) {
  StartWithMetadata(kTestMetadata);
  ConnectToCpuCtrl(kTestPerformanceDomains[0]);

  auto coreCountResp = cpu_ctrl_->GetNumLogicalCores();

  ASSERT_OK(coreCountResp.status());

  EXPECT_EQ(coreCountResp.value().count, kTestPerformanceDomains[0].core_count());
}

// We attempt to lower the voltage and then inject a fault into the parent voltage driver.
TEST_F(AmlCpuTest, TestVregLowerFail) {
  StartWithMetadata(kTestMetadata);
  ConnectToCpuCtrl(kTestPerformanceDomains[0]);

  uint32_t initial_rate;
  driver_test().RunInEnvironmentTypeContext([&initial_rate](AmlCpuEnvironment& env) {
    auto out_rate = env.clock_cpu_scaler_server_.rate();
    ASSERT_TRUE(out_rate.has_value());
    initial_rate = out_rate.value();
  });

  // Inject a fault into the voltage driver.
  driver_test().RunInEnvironmentTypeContext(
      [](AmlCpuEnvironment& env) { env.power_server_.SetFaultVoltage(1); });

  // Scale to the lowest opp.
  const uint32_t min_opp_index = static_cast<uint32_t>(kTestOperatingPoints.size() - 1);

  auto min_result = cpu_ctrl_->SetCurrentOperatingPoint(min_opp_index);
  EXPECT_OK(min_result.status());

  // We injected a fault which means that the driver was unable to set the voltate which means that
  // this call should fail.
  EXPECT_FALSE(min_result->is_ok());

  // If the change in voltage fails, the frequency should not be changed.
  driver_test().RunInEnvironmentTypeContext([initial_rate](AmlCpuEnvironment& env) {
    auto new_rate = env.clock_cpu_scaler_server_.rate();
    ASSERT_TRUE(new_rate.has_value());
    EXPECT_EQ(initial_rate, new_rate.value());
  });
}

// This test is much like TestVregLowerFail from above but we inject the fault after lowering the
// voltage and then attempt to raise it which should fail.
TEST_F(AmlCpuTest, TestVregRaiseFail) {
  StartWithMetadata(kTestMetadata);
  ConnectToCpuCtrl(kTestPerformanceDomains[0]);

  // Scale to the lowest opp.
  const uint32_t min_opp_index = static_cast<uint32_t>(kTestOperatingPoints.size() - 1);

  auto min_result = cpu_ctrl_->SetCurrentOperatingPoint(min_opp_index);
  EXPECT_OK(min_result.status());
  EXPECT_TRUE(min_result->is_ok());

  // Inject a fault into the voltage driver.
  driver_test().RunInEnvironmentTypeContext(
      [](AmlCpuEnvironment& env) { env.power_server_.SetFaultVoltage(1); });

  // Cache the rate of the clocks.
  uint32_t initial_rate;
  driver_test().RunInEnvironmentTypeContext([&initial_rate](AmlCpuEnvironment& env) {
    auto out_rate = env.clock_cpu_scaler_server_.rate();
    ASSERT_TRUE(out_rate.has_value());
    initial_rate = out_rate.value();
  });

  // Raising the voltage after injecting the fault should fail.
  auto max_result = cpu_ctrl_->SetCurrentOperatingPoint(0);
  EXPECT_OK(max_result.status());
  EXPECT_FALSE(max_result->is_ok());

  // If the change in voltage fails, the frequency should not be changed.
  driver_test().RunInEnvironmentTypeContext([initial_rate](AmlCpuEnvironment& env) {
    auto new_rate = env.clock_cpu_scaler_server_.rate();
    ASSERT_TRUE(new_rate.has_value());
    EXPECT_EQ(initial_rate, new_rate.value());
  });
}

}  // namespace amlogic_cpu
