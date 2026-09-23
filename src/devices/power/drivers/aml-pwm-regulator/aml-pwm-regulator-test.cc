// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/power/drivers/aml-pwm-regulator/aml-pwm-regulator.h"

#include <fidl/fuchsia.hardware.pwm/cpp/wire_test_base.h>
#include <fidl/fuchsia.hardware.vreg/cpp/fidl.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace aml_pwm_regulator {

class MockPwmServer final : public fidl::testing::WireTestBase<fuchsia_hardware_pwm::Pwm> {
 public:
  explicit MockPwmServer()
      : dispatcher_(fdf::Dispatcher::GetCurrent()->async_dispatcher()),
        outgoing_(fdf::Dispatcher::GetCurrent()->async_dispatcher()) {}

  void SetConfig(SetConfigRequestView request, SetConfigCompleter::Sync& completer) override {
    ASSERT_TRUE(expect_configs_.size() > 0);
    auto expect_config = expect_configs_.front();

    const auto& actual_config = request->config;
    ASSERT_EQ(actual_config.polarity, expect_config.polarity);
    ASSERT_EQ(actual_config.period_ns, expect_config.period_ns);
    ASSERT_EQ(actual_config.duty_cycle, expect_config.duty_cycle);
    ASSERT_EQ(actual_config.mode_config.size(), expect_config.mode_config.size());
    ASSERT_EQ(reinterpret_cast<aml_pwm::mode_config*>(actual_config.mode_config.data())->mode,
              reinterpret_cast<aml_pwm::mode_config*>(expect_config.mode_config.data())->mode);

    expect_configs_.pop_front();
    mode_config_buffers_.pop_front();
    completer.ReplySuccess();
  }

  void GetConfig(GetConfigCompleter::Sync& completer) override {
    ASSERT_TRUE(expect_default_configs_.size() > 0);
    auto default_config = expect_default_configs_.front();

    expect_default_configs_.pop_front();
    completer.ReplySuccess(default_config);
  }

  void Enable(EnableCompleter::Sync& completer) override {
    ASSERT_TRUE(expect_enable_);
    expect_enable_ = false;
    completer.ReplySuccess();
  }

  void ExpectEnable() { expect_enable_ = true; }

  void ExpectSetConfig(fuchsia_hardware_pwm::wire::PwmConfig config) {
    std::unique_ptr<uint8_t[]> mode_config = std::make_unique<uint8_t[]>(config.mode_config.size());
    memcpy(mode_config.get(), config.mode_config.data(), config.mode_config.size());

    auto copy = config;
    copy.mode_config =
        fidl::VectorView<uint8_t>::FromExternal(mode_config.get(), config.mode_config.size());
    expect_configs_.push_back(std::move(copy));
    mode_config_buffers_.push_back(std::move(mode_config));
  }

  void ExpectGetConfig(fuchsia_hardware_pwm::wire::PwmConfig config) {
    expect_default_configs_.push_back(std::move(config));
  }

  void NotImplemented_(const std::string& name, ::fidl::CompleterBase& completer) override {
    completer.Close(ZX_ERR_NOT_SUPPORTED);
  }

  fidl::ClientEnd<fuchsia_io::Directory> Connect() {
    auto device_handler = [this](fidl::ServerEnd<fuchsia_hardware_pwm::Pwm> request) {
      fidl::BindServer(dispatcher_, std::move(request), this);
    };
    fuchsia_hardware_pwm::Service::InstanceHandler handler({.pwm = std::move(device_handler)});

    auto service_result = outgoing_.AddService<fuchsia_hardware_pwm::Service>(std::move(handler));
    ZX_ASSERT(service_result.is_ok());

    auto endpoints = fidl::Endpoints<fuchsia_io::Directory>::Create();
    ZX_ASSERT(outgoing_.Serve(std::move(endpoints.server)).is_ok());

    return std::move(endpoints.client);
  }

  void VerifyAndClear() {
    ASSERT_EQ(expect_configs_.size(), 0u);
    ASSERT_EQ(mode_config_buffers_.size(), 0u);
    ASSERT_FALSE(expect_enable_);
  }

  fuchsia_hardware_pwm::Service::InstanceHandler GetInstanceHandler() {
    return fuchsia_hardware_pwm::Service::InstanceHandler({
        .pwm = bindings_.CreateHandler(this, fdf::Dispatcher::GetCurrent()->async_dispatcher(),
                                       fidl::kIgnoreBindingClosure),
    });
  }

 private:
  async_dispatcher_t* dispatcher_;
  component::OutgoingDirectory outgoing_;

  fidl::ServerBindingGroup<fuchsia_hardware_pwm::Pwm> bindings_;

  std::list<fuchsia_hardware_pwm::wire::PwmConfig> expect_configs_;
  std::list<fuchsia_hardware_pwm::wire::PwmConfig> expect_default_configs_;
  std::list<std::unique_ptr<uint8_t[]>> mode_config_buffers_;

  bool expect_enable_ = false;
};

class AmlPwmRegulatorTestEnvironment : public fdf_testing::Environment {
 public:
  void Init(std::string_view pwm_name) {
    fuchsia_hardware_vreg::VregMetadata metadata{{
        .name{pwm_name},
        .min_voltage_uv = 690000,
        .voltage_step_uv = 1000,
        .num_steps = 11,
    }};
    pdev_.AddFidlMetadata(fuchsia_hardware_vreg::VregMetadata::kSerializableName, metadata);

    pwm_.ExpectEnable();
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.GetInstanceHandler(fdf::Dispatcher::GetCurrent()->async_dispatcher()), "pdev");
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result =
          to_driver_vfs.AddService<fuchsia_hardware_pwm::Service>(pwm_.GetInstanceHandler(), "pwm");
      if (result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  MockPwmServer& pwm() { return pwm_; }

 private:
  MockPwmServer pwm_;
  fdf_fake::FakePDev pdev_;
};

class FixtureConfig final {
 public:
  using DriverType = AmlPwmRegulatorDriver;
  using EnvironmentType = AmlPwmRegulatorTestEnvironment;
};

class AmlPwmRegulatorTest : public ::testing::Test {
 public:
  void SetUp() override {
    static constexpr std::string_view kPwmName = "pwm-0-regulator";

    driver_test_.RunInEnvironmentTypeContext([](auto& env) { env.Init(kPwmName); });
    ASSERT_OK(driver_test_.StartDriver());
    zx::result result = driver_test_.Connect<fuchsia_hardware_vreg::Service::Vreg>(kPwmName);
    ASSERT_OK(result);
    client_.Bind(std::move(result.value()));
  }

  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

 protected:
  void WithPwm(fit::callback<void(MockPwmServer&)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [callback = std::move(callback)](auto& env) mutable { callback(env.pwm()); });
  }

  void SetAndExpectVoltageStep(uint32_t value) {
    fidl::WireResult result = client_->SetVoltageStep(value);
    EXPECT_OK(result.status());
    EXPECT_TRUE(result->is_ok());

    fidl::WireResult voltage_step = client_->GetVoltageStep();
    EXPECT_TRUE(voltage_step.ok());
    EXPECT_TRUE(voltage_step->is_ok());
    EXPECT_EQ(voltage_step.value()->result, value);
  }

  fidl::WireSyncClient<fuchsia_hardware_vreg::Vreg>& client() { return client_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
  fidl::WireSyncClient<fuchsia_hardware_vreg::Vreg> client_;
};

TEST_F(AmlPwmRegulatorTest, RegulatorTest) {
  fidl::WireResult params = client()->GetRegulatorParams();
  EXPECT_TRUE(params.ok());
  EXPECT_TRUE(params->is_ok());
  EXPECT_EQ(params.value()->min_uv, 690'000u);
  EXPECT_EQ(params.value()->num_steps, 11u);
  EXPECT_EQ(params.value()->step_size_uv, 1'000u);

  fidl::WireResult voltage_step = client()->GetVoltageStep();
  EXPECT_TRUE(voltage_step.ok());
  EXPECT_TRUE(voltage_step->is_ok());
  EXPECT_EQ(voltage_step.value()->result, 11u);

  aml_pwm::mode_config mode = {
      .mode = aml_pwm::Mode::kOn,
      .regular = {},
  };
  fuchsia_hardware_pwm::wire::PwmConfig cfg = {
      .polarity = false,
      .period_ns = 1250,
  };

  WithPwm([&](auto& pwm) { pwm.ExpectGetConfig(cfg); });

  cfg.duty_cycle = 70;
  cfg.mode_config =
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&mode), sizeof(mode));

  WithPwm([&](auto& pwm) { pwm.ExpectSetConfig(cfg); });

  SetAndExpectVoltageStep(3);

  WithPwm([&](auto& pwm) { pwm.ExpectGetConfig(cfg); });

  cfg.duty_cycle = 10;
  WithPwm([&](auto& pwm) { pwm.ExpectSetConfig(cfg); });
  SetAndExpectVoltageStep(9);

  fidl::WireResult result = client()->SetVoltageStep(14);
  EXPECT_TRUE(result.ok());
  EXPECT_TRUE(result->is_error());
  EXPECT_EQ(result->error_value(), ZX_ERR_INVALID_ARGS);

  WithPwm([&](auto& pwm) { pwm.VerifyAndClear(); });
}

}  // namespace aml_pwm_regulator
