// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-light.h"

#include <fidl/fuchsia.hardware.pwm/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>

#include <cmath>
#include <list>

#include <fbl/alloc_checker.h>
#include <zxtest/zxtest.h>

#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"

bool operator==(const fuchsia_hardware_pwm::wire::PwmConfig& lhs,
                const fuchsia_hardware_pwm::wire::PwmConfig& rhs) {
  return (lhs.polarity == rhs.polarity) && (lhs.period_ns == rhs.period_ns) &&
         (lhs.duty_cycle == rhs.duty_cycle) && (lhs.mode_config.size() == rhs.mode_config.size()) &&
         (reinterpret_cast<aml_pwm::mode_config*>(lhs.mode_config.data())->mode ==
          reinterpret_cast<aml_pwm::mode_config*>(rhs.mode_config.data())->mode);
}

namespace aml_light {
class MockPwmServer final : public fidl::testing::WireTestBase<fuchsia_hardware_pwm::Pwm> {
 public:
  void SetConfig(SetConfigRequestView request, SetConfigCompleter::Sync& completer) override {
    ASSERT_GT(expect_configs_.size(), 0);
    auto expect_config = expect_configs_.front();

    ASSERT_EQ(request->config, expect_config);

    expect_configs_.erase(expect_configs_.begin());
    mode_config_buffers_.pop_front();
    completer.ReplySuccess();
  }
  void Enable(EnableCompleter::Sync& completer) override {
    ASSERT_TRUE(expect_enable_);
    expect_enable_ = false;
    completer.ReplySuccess();
  }

  void ExpectEnable() { expect_enable_ = true; }

  void ExpectSetConfig(fuchsia_hardware_pwm::wire::PwmConfig config) {
    std::unique_ptr<uint8_t[]>& mode_config =
        mode_config_buffers_.emplace_back(std::make_unique<uint8_t[]>(config.mode_config.size()));
    std::copy_n(config.mode_config.data(), config.mode_config.size(), mode_config.get());

    config.mode_config =
        fidl::VectorView<uint8_t>::FromExternal(mode_config.get(), config.mode_config.size());
    expect_configs_.push_back(config);
  }

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

class FakeAmlLight : public AmlLight {
 public:
  static zx::result<std::unique_ptr<FakeAmlLight>> Create(
      fidl::ClientEnd<fuchsia_hardware_gpio::Gpio> gpio,
      std::optional<fidl::WireSyncClient<fuchsia_hardware_pwm::Pwm>> pwm,
      zx_duration_t pwm_period = 170'625) {
    fbl::AllocChecker ac;
    std::unique_ptr device = fbl::make_unique_checked<FakeAmlLight>(&ac);
    if (!ac.check()) {
      return zx::error(ZX_ERR_NO_MEMORY);
    }
    LightDevice& light = device->lights_.emplace_back(
        "test", std::move(gpio), pwm.has_value() ? std::move(pwm) : std::nullopt,
        zx::duration(pwm_period));
    if (zx_status_t status = light.Init(true); status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(std::move(device));
  }

  explicit FakeAmlLight() : AmlLight(nullptr) {}
};

namespace {

class AmlLightTest : public zxtest::Test {
 public:
  void SetUp() override { EXPECT_OK(fidl_servers_loop_.StartThread("fidl-servers")); }

  void Init() {
    loop_ = std::make_unique<async::Loop>(&kAsyncLoopConfigAttachToCurrentThread);
    zx::result server = fidl::CreateEndpoints(&client_);
    ASSERT_OK(server);
    ASSERT_OK(loop_->StartThread("aml-light-test-loop"));
    fidl::BindServer(loop_->dispatcher(), std::move(server.value()), light_.get());
  }

  void TearDown() override {
    pwm_.SyncCall(&MockPwmServer::VerifyAndClear);

    loop_->Quit();
    loop_->JoinThreads();
  }

 protected:
  friend class FakeAmlLight;

  std::unique_ptr<FakeAmlLight> light_;

  async::Loop fidl_servers_loop_{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<MockPwmServer> pwm_{fidl_servers_loop_.dispatcher(),
                                                          std::in_place};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> gpio_{fidl_servers_loop_.dispatcher(),
                                                                 std::in_place};

  fidl::ClientEnd<fuchsia_hardware_light::Light> client_;

 private:
  std::unique_ptr<async::Loop> loop_;
};

TEST_F(AmlLightTest, GetInfoTest1) {
  pwm_.SyncCall(&MockPwmServer::ExpectEnable);
  aml_pwm::mode_config regular = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig init_config = {
      false, 170625, 100.0,
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&regular),
                                              sizeof(regular))};
  pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, init_config);

  auto gpio_client = gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
  auto pwm_client = pwm_.SyncCall(&MockPwmServer::BindServer);
  zx::result light = FakeAmlLight::Create(std::move(gpio_client), std::move(pwm_client));
  ASSERT_OK(light);
  light_ = std::move(light.value());
  Init();

  fidl::WireSyncClient<fuchsia_hardware_light::Light> client(std::move(client_));
  auto result = client->GetInfo(0);
  EXPECT_OK(result.status());
  EXPECT_FALSE(result->is_error());
  EXPECT_EQ(strcmp(result->value()->info.name.begin(), "test"), 0);
  EXPECT_EQ(result->value()->info.capability, Capability::kBrightness);
}

TEST_F(AmlLightTest, GetInfoTest2) {
  gpio_.SyncCall(&fake_gpio::FakeGpio::SetCurrentState,
                 fake_gpio::State{.sub_state = fake_gpio::WriteSubState{.value = 0}});
  auto gpio_client = gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
  zx::result light = FakeAmlLight::Create(std::move(gpio_client), std::nullopt);
  ASSERT_OK(light);
  light_ = std::move(light.value());
  Init();
  EXPECT_EQ(1, gpio_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));

  fidl::WireSyncClient<fuchsia_hardware_light::Light> client(std::move(client_));
  auto result = client->GetInfo(0);
  EXPECT_OK(result.status());
  EXPECT_FALSE(result->is_error());
  EXPECT_EQ(strcmp(result->value()->info.name.begin(), "test"), 0);
  EXPECT_EQ(result->value()->info.capability, Capability::kSimple);
}

TEST_F(AmlLightTest, SetValueTest1) {
  gpio_.SyncCall(&fake_gpio::FakeGpio::SetCurrentState,
                 fake_gpio::State{.sub_state = fake_gpio::WriteSubState{.value = 0}});
  auto gpio_client = gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
  zx::result light = FakeAmlLight::Create(std::move(gpio_client), std::nullopt);
  ASSERT_OK(light);
  light_ = std::move(light.value());
  Init();
  EXPECT_EQ(1, gpio_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));

  fidl::WireSyncClient<fuchsia_hardware_light::Light> client(std::move(client_));
  {
    auto get_result = client->GetCurrentSimpleValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, true);
  }
  {
    auto get_result = client->GetCurrentSimpleValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, true);
  }
  {
    auto set_result = client->SetSimpleValue(0, false);
    EXPECT_EQ(0, gpio_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    EXPECT_OK(set_result.status());
    EXPECT_FALSE(set_result->is_error());
  }
  {
    auto get_result = client->GetCurrentSimpleValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, false);
  }
  {
    auto set_result = client->SetSimpleValue(0, true);
    EXPECT_EQ(1, gpio_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    EXPECT_OK(set_result.status());
    EXPECT_FALSE(set_result->is_error());
  }
  {
    auto set_result = client->SetSimpleValue(0, true);
    EXPECT_EQ(1, gpio_.SyncCall(&fake_gpio::FakeGpio::GetWriteValue));
    EXPECT_OK(set_result.status());
    EXPECT_FALSE(set_result->is_error());
  }
  {
    auto get_result = client->GetCurrentSimpleValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, true);
  }
}

TEST_F(AmlLightTest, SetValueTest2) {
  pwm_.SyncCall(&MockPwmServer::ExpectEnable);
  aml_pwm::mode_config regular = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig config = {
      false, 170625, 100.0,
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&regular),
                                              sizeof(regular))};
  pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, config);

  auto gpio_client = gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
  auto pwm_client = pwm_.SyncCall(&MockPwmServer::BindServer);
  zx::result light = FakeAmlLight::Create(std::move(gpio_client), std::move(pwm_client));
  ASSERT_OK(light);
  light_ = std::move(light.value());
  Init();

  fidl::WireSyncClient<fuchsia_hardware_light::Light> client(std::move(client_));
  {
    auto get_result = client->GetCurrentBrightnessValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, 1.0);
  }
  {
    auto get_result = client->GetCurrentBrightnessValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, 1.0);
  }
  {
    config.duty_cycle = 0;
    pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, config);
    auto set_result = client->SetBrightnessValue(0, 0.0);
    EXPECT_OK(set_result.status());
    EXPECT_FALSE(set_result->is_error());
  }
  {
    auto get_result = client->GetCurrentBrightnessValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, 0.0);
  }
  {
    config.duty_cycle = 20.0;
    pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, config);
    auto set_result = client->SetBrightnessValue(0, 0.2);
    EXPECT_OK(set_result.status());
    EXPECT_FALSE(set_result->is_error());
  }
  {
    pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, config);
    auto set_result = client->SetBrightnessValue(0, 0.2);
    EXPECT_OK(set_result.status());
    EXPECT_FALSE(set_result->is_error());
  }
  {
    auto get_result = client->GetCurrentBrightnessValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, 0.2);
  }
}

TEST_F(AmlLightTest, SetInvalidValueTest) {
  pwm_.SyncCall(&MockPwmServer::ExpectEnable);
  aml_pwm::mode_config regular = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig config = {
      false, 170625, 100.0,
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&regular),
                                              sizeof(regular))};
  pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, config);

  auto gpio_client = gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
  auto pwm_client = pwm_.SyncCall(&MockPwmServer::BindServer);
  zx::result light = FakeAmlLight::Create(std::move(gpio_client), std::move(pwm_client));
  ASSERT_OK(light);
  light_ = std::move(light.value());
  Init();

  fidl::WireSyncClient<fuchsia_hardware_light::Light> client(std::move(client_));
  {
    auto get_result = client->GetCurrentBrightnessValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, 1.0);
  }
  {
    auto set_result = client->SetBrightnessValue(0, 3.2);
    EXPECT_OK(set_result.status());
    EXPECT_TRUE(set_result->is_error());
  }
  {
    auto get_result = client->GetCurrentBrightnessValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, 1.0);
  }
  {
    auto set_result = client->SetBrightnessValue(0, -0.225);
    EXPECT_OK(set_result.status());
    EXPECT_TRUE(set_result->is_error());
  }
  {
    auto set_result = client->SetBrightnessValue(0, NAN);
    EXPECT_OK(set_result.status());
    EXPECT_TRUE(set_result->is_error());
  }
  {
    auto get_result = client->GetCurrentBrightnessValue(0);
    EXPECT_OK(get_result.status());
    EXPECT_FALSE(get_result->is_error());
    EXPECT_EQ(get_result->value()->value, 1.0);
  }
}

TEST_F(AmlLightTest, SetValueTestNelson) {
  pwm_.SyncCall(&MockPwmServer::ExpectEnable);
  aml_pwm::mode_config regular = {aml_pwm::Mode::kOn, {}};
  fuchsia_hardware_pwm::wire::PwmConfig config = {
      false, 500'000, 100.0,
      fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&regular),
                                              sizeof(regular))};
  pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, config);

  auto gpio_client = gpio_.SyncCall(&fake_gpio::FakeGpio::Connect);
  auto pwm_client = pwm_.SyncCall(&MockPwmServer::BindServer);
  zx::result light = FakeAmlLight::Create(std::move(gpio_client), std::move(pwm_client), 500'000);
  ASSERT_OK(light);
  light_ = std::move(light.value());
  Init();

  fidl::WireSyncClient<fuchsia_hardware_light::Light> client(std::move(client_));

  {
    config.duty_cycle = 0;
    pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, config);
    auto set_result = client->SetBrightnessValue(0, 0.0);
    EXPECT_OK(set_result.status());
    EXPECT_FALSE(set_result->is_error());
  }
  {
    config.duty_cycle = 20.0;
    pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, config);
    auto set_result = client->SetBrightnessValue(0, 0.2);
    EXPECT_OK(set_result.status());
    EXPECT_FALSE(set_result->is_error());
  }
  {
    config.duty_cycle = 100.0;
    pwm_.SyncCall(&MockPwmServer::ExpectSetConfig, config);
    auto set_result = client->SetBrightnessValue(0, 1.0);
    EXPECT_OK(set_result.status());
    EXPECT_FALSE(set_result->is_error());
  }
}

}  // namespace

}  // namespace aml_light
