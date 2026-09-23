// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-pwm-init.h"

#include <fidl/fuchsia.hardware.gpio/cpp/wire.h>
#include <fidl/fuchsia.hardware.pwm/cpp/wire_test_base.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/default.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>

#include <cstring>
#include <list>

#include <fbl/alloc_checker.h>
#include <zxtest/zxtest.h>

#include "src/devices/gpio/testing/fake-gpio/fake-gpio.h"

bool operator==(const fuchsia_hardware_pwm::wire::PwmConfig& lhs,
                const fuchsia_hardware_pwm::wire::PwmConfig& rhs) {
  if (lhs.polarity != rhs.polarity || lhs.period_ns != rhs.period_ns ||
      lhs.duty_cycle != rhs.duty_cycle || lhs.mode_config.size() != rhs.mode_config.size()) {
    return false;
  }
  return memcmp(lhs.mode_config.data(), rhs.mode_config.data(), lhs.mode_config.size()) == 0;
}

namespace pwm_init {

class MockPwmServer final : public fidl::testing::WireTestBase<fuchsia_hardware_pwm::Pwm> {
 public:
  void SetConfig(SetConfigRequestView request, SetConfigCompleter::Sync& completer) override {
    ASSERT_GT(expect_configs_.size(), 0);
    auto expect_config = expect_configs_.front();

    ASSERT_EQ(request->config, expect_config);
    if (request->config.mode_config.size() == sizeof(aml_pwm::mode_config)) {
      const auto* bytes = request->config.mode_config.data();
      EXPECT_EQ(bytes[14], 0u);
      EXPECT_EQ(bytes[15], 0u);
    }

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

TEST(PwmInitDeviceTest, InitTest) {
  fdf::Logger logger;
  fdf::Logger::SetGlobalInstance(&logger);

  async::Loop fidl_loop{&kAsyncLoopConfigNoAttachToCurrentThread};
  async_patterns::TestDispatcherBound<MockPwmServer> pwm{fidl_loop.dispatcher(), std::in_place};
  async_patterns::TestDispatcherBound<fake_gpio::FakeGpio> bt_gpio{fidl_loop.dispatcher(),
                                                                   std::in_place};
  EXPECT_OK(fidl_loop.StartThread("fidl-servers"));

  auto pwm_client = pwm.SyncCall(&MockPwmServer::BindServer);
  auto bt_gpio_client = bt_gpio.SyncCall(&fake_gpio::FakeGpio::Connect);
  // Create a clock connection, but don't connect it to anything.
  auto clock_endpoints = fidl::Endpoints<fuchsia_hardware_clock::Clock>::Create();
  fidl::ClientEnd<fuchsia_hardware_clock::Clock> clock(std::move(clock_endpoints.client));
  clock_endpoints.server.Close(ZX_OK);

  pwm.SyncCall(&MockPwmServer::ExpectEnable);
  aml_pwm::mode_config two_timer;
  memset(&two_timer, 0, sizeof(two_timer));
  two_timer.mode = aml_pwm::Mode::kTwoTimer;
  two_timer.two_timer.period_ns2 = 30052;
  two_timer.two_timer.duty_cycle2 = 50.0;
  two_timer.two_timer.timer1 = 0x0a;
  two_timer.two_timer.timer2 = 0x0a;
  fuchsia_hardware_pwm::wire::PwmConfig init_cfg = {
      .polarity = false,
      .period_ns = 30053,
      .duty_cycle = static_cast<float>(49.931787176),
      .mode_config = fidl::VectorView<uint8_t>::FromExternal(reinterpret_cast<uint8_t*>(&two_timer),
                                                             sizeof(two_timer))};
  pwm.SyncCall(&MockPwmServer::ExpectSetConfig, init_cfg);

  PwmInitDevice dev(std::move(clock), std::move(pwm_client), std::move(bt_gpio_client));
  EXPECT_OK(dev.Init());

  std::vector states = bt_gpio.SyncCall(&fake_gpio::FakeGpio::GetStateLog);
  ASSERT_EQ(2, states.size());
  ASSERT_EQ(fake_gpio::WriteSubState{.value = 0}, states[0].sub_state);
  ASSERT_EQ(fake_gpio::WriteSubState{.value = 1}, states[1].sub_state);

  pwm.SyncCall(&MockPwmServer::VerifyAndClear);
}

}  // namespace pwm_init
