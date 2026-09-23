// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-pwm.h"

#include <fidl/fuchsia.driver.metadata/cpp/fidl.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <cstring>
#include <vector>

#include <gtest/gtest.h>
#include <mock-mmio-reg/mock-mmio-reg.h>
#include <soc/aml-common/aml-pwm-regs.h>

#include "src/lib/testing/predicates/status.h"

namespace pwm {

namespace {

class AmlPwmDriverTestEnvironment : public fdf_testing::Environment {
 public:
  void SetupCommon() {
    static constexpr size_t kRegSize = 0x00001000 / sizeof(uint32_t);
    static constexpr size_t kMmioCount = 5;

    std::map<uint32_t, fdf_fake::Mmio> mmios;
    for (size_t i = 0; i < kMmioCount; ++i) {
      auto& mmio = *mmios_.emplace_back(
          std::make_unique<ddk_mock::MockMmioRegRegion>(sizeof(uint32_t), kRegSize));
      mmio[2lu * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFDFFFFFA);
      if (i != 1) {
        mmio[2lu * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFEFFFFF5);
      }
      mmios.insert({i, mmio.GetMmioBuffer()});
    }
    pdev_.SetConfig({.mmios = std::move(mmios), .device_info{{.mmio_count = kMmioCount}}});
  }

  void Init() {
    SetupCommon();
    // Protect channel 3 for protect tests
    static const fuchsia_hardware_pwm::PwmChannelsMetadata kMetadata({
        .channels{
            {
                {{.id = 0}},
                {{.id = 1}},
                {{.id = 2}},
                {{.id = 3, .skip_init = true}},
                {{.id = 4}},
                {{.id = 5}},
                {{.id = 6}},
                {{.id = 7}},
                {{.id = 8}},
                {{.id = 9}},
            },
        },
    });

    pdev_.AddFidlMetadata(fuchsia_hardware_pwm::PwmChannelsMetadata::kSerializableName, kMetadata);
  }

  void InitGeneric(fuchsia_driver_metadata::Dictionary& metadata) {
    SetupCommon();
    pdev_.AddFidlMetadata("fuchsia.hardware.pwm.PwmChannelsMetadata", metadata);
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.GetInstanceHandler(dispatcher));
      if (result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  std::span<std::unique_ptr<ddk_mock::MockMmioRegRegion>> mmios() { return mmios_; }

 private:
  std::vector<std::unique_ptr<ddk_mock::MockMmioRegRegion>> mmios_;
  fdf_fake::FakePDev pdev_;
};

class FixtureConfig final {
 public:
  using DriverType = AmlPwmDriver;
  using EnvironmentType = AmlPwmDriverTestEnvironment;
};

class AmlPwmDriverTest : public ::testing::Test {
 protected:
  void SetUp() override {
    driver_test_.RunInEnvironmentTypeContext([](auto& env) { env.Init(); });
    ASSERT_OK(driver_test_.StartDriver());
    zx::result client_end = driver_test_.Connect<fuchsia_hardware_pwmimpl::Service::Device>();
    ASSERT_TRUE(client_end.is_ok());
    pwm_impl_.Bind(std::move(*client_end));
  }

  void TearDown() override {
    ASSERT_OK(driver_test_.StopDriver());
    WithMmios([](auto mmios) {
      for (auto& mmio : mmios) {
        mmio->VerifyAll();
      }
    });
  }

  static fuchsia_hardware_pwm::wire::PwmConfig CreatePwmConfig(
      bool polarity, uint32_t period_ns, float duty_cycle, mode_config* mode_config,
      size_t mode_config_size_bytes = sizeof(struct mode_config)) {
    auto mode_config_bytes =
        mode_config ? fidl::VectorView<uint8_t>::FromExternal(
                          reinterpret_cast<uint8_t*>(mode_config), mode_config_size_bytes)
                    : fidl::VectorView<uint8_t>{};
    return fuchsia_hardware_pwm::wire::PwmConfig{
        .polarity = polarity,
        .period_ns = period_ns,
        .duty_cycle = duty_cycle,
        .mode_config = mode_config_bytes,
    };
  }

  void WithMmios(
      fit::callback<void(std::span<std::unique_ptr<ddk_mock::MockMmioRegRegion>>)> callback) {
    driver_test_.RunInEnvironmentTypeContext(
        [callback = std::move(callback)](auto& env) mutable { callback(env.mmios()); });
  }

  zx::result<fuchsia_hardware_pwm::PwmConfig> GetPwmConfig(uint32_t idx) {
    fdf::Arena arena('TEST');
    fdf::WireUnownedResult<fuchsia_hardware_pwmimpl::PwmImpl::GetConfig> result =
        pwm_impl_.buffer(arena)->GetConfig(idx);
    if (!result.ok()) {
      return zx::error(result.status());
    }
    if (result->is_error()) {
      return zx::error(result->error_value());
    }
    return zx::ok(fidl::ToNatural(result->value()->config));
  }

  zx_status_t SetPwmConfig(uint32_t idx, const fuchsia_hardware_pwm::wire::PwmConfig* config) {
    if (config == nullptr) {
      return ZX_ERR_INVALID_ARGS;
    }
    fdf::Arena arena('TEST');
    fdf::WireUnownedResult<fuchsia_hardware_pwmimpl::PwmImpl::SetConfig> result =
        pwm_impl_.buffer(arena)->SetConfig(idx, *config);
    if (!result.ok()) {
      return result.status();
    }
    if (result->is_error()) {
      return result->error_value();
    }
    return ZX_OK;
  }

  zx_status_t EnablePwm(uint32_t idx) {
    fdf::Arena arena('TEST');
    fdf::WireUnownedResult<fuchsia_hardware_pwmimpl::PwmImpl::Enable> result =
        pwm_impl_.buffer(arena)->Enable(idx);
    if (!result.ok()) {
      return result.status();
    }
    if (result->is_error()) {
      return result->error_value();
    }
    return ZX_OK;
  }

  zx_status_t DisablePwm(uint32_t idx) {
    fdf::Arena arena('TEST');
    fdf::WireUnownedResult<fuchsia_hardware_pwmimpl::PwmImpl::Disable> result =
        pwm_impl_.buffer(arena)->Disable(idx);
    if (!result.ok()) {
      return result.status();
    }
    if (result->is_error()) {
      return result->error_value();
    }
    return ZX_OK;
  }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
  fdf::WireSyncClient<fuchsia_hardware_pwmimpl::PwmImpl> pwm_impl_;
};

TEST_F(AmlPwmDriverTest, ProtectPwmTest) {
  mode_config mode_cfg{
      .mode = static_cast<Mode>(100),
      .regular = {},
  };
  auto cfg = CreatePwmConfig(false, 1250, 100.0, &mode_cfg);
  EXPECT_NE(SetPwmConfig(3, &cfg), ZX_OK);
}

TEST_F(AmlPwmDriverTest, GetPwmConfigTest) {
  auto result = GetPwmConfig(0);
  ASSERT_OK(result.status_value());
  EXPECT_EQ(result->polarity(), false);
  EXPECT_EQ(result->period_ns(), 0u);
  EXPECT_EQ(result->duty_cycle(), 0.0f);
  ASSERT_EQ(result->mode_config().size(), sizeof(mode_config));
  auto mode_cfg = reinterpret_cast<const mode_config*>(result->mode_config().data());
  EXPECT_EQ(mode_cfg->mode, Mode::kOff);
}

TEST_F(AmlPwmDriverTest, GetPwmConfigZeroInitializedTest) {
  auto result = GetPwmConfig(0);
  ASSERT_OK(result.status_value());

  // Verify all bytes after the mode field (the union bytes) are zero-initialized.
  ASSERT_EQ(result->mode_config().size(), sizeof(mode_config));
  const auto* bytes = result->mode_config().data();
  for (size_t i = sizeof(Mode); i < sizeof(mode_config); ++i) {
    EXPECT_EQ(bytes[i], 0u) << "Byte at offset " << i << " was not zero-initialized.";
  }
}

TEST_F(AmlPwmDriverTest, SetPwmConfigInvalidNullConfig) {
  // config is null
  EXPECT_NE(SetPwmConfig(0, nullptr), ZX_OK);
}

TEST_F(AmlPwmDriverTest, SetPwmConfigInvalidNoModeBuffer) {
  auto fail_cfg = CreatePwmConfig(false, 1250, 100.0, nullptr);
  EXPECT_NE(SetPwmConfig(0, &fail_cfg), ZX_OK);
}

TEST_F(AmlPwmDriverTest, SetPwmConfigInvalidModeConfigSizeIncorrect) {
  mode_config fail_mode{
      .mode = Mode::kOn,
      .regular = {},
  };
  auto fail_cfg = CreatePwmConfig(false, 1250, 100.0, &fail_mode, 10);
  EXPECT_NE(SetPwmConfig(0, &fail_cfg), ZX_OK);
}

TEST_F(AmlPwmDriverTest, SetPwmConfigInvalidTwoTimerTimer2InvalidDutyCycle) {
  mode_config fail_mode{.mode = Mode::kTwoTimer, .two_timer = {}};
  // Invalid duty cycle for timer 2.
  fail_mode.two_timer.duty_cycle2 = -10.0;
  auto fail_cfg = CreatePwmConfig(false, 1250, 100.0, &fail_mode);
  EXPECT_NE(SetPwmConfig(0, &fail_cfg), ZX_OK);

  fail_mode.two_timer.duty_cycle2 = 120.0;
  fail_cfg = CreatePwmConfig(false, 1250, 100.0, &fail_mode);
  EXPECT_NE(SetPwmConfig(0, &fail_cfg), ZX_OK);
}

TEST_F(AmlPwmDriverTest, SetPwmConfigInvalidTimer1InvalidDutyCycle) {
  mode_config fail_mode{.mode = Mode::kOn, .regular = {}};
  // Invalid duty cycle for timer 1.
  auto fail_cfg = CreatePwmConfig(false, 1250, -10.0, &fail_mode);
  EXPECT_NE(SetPwmConfig(0, &fail_cfg), ZX_OK);

  fail_cfg = CreatePwmConfig(false, 1250, 120.0, &fail_mode);
  EXPECT_NE(SetPwmConfig(0, &fail_cfg), ZX_OK);
}

TEST_F(AmlPwmDriverTest, SetPwmConfigInvalidTimer1InvalidMode) {
  mode_config fail_mode{
      // Invalid mode
      .mode = static_cast<Mode>(100),
      .regular = {},
  };
  auto fail_cfg = CreatePwmConfig(false, 1250, 100.0, &fail_mode);
  EXPECT_NE(SetPwmConfig(0, &fail_cfg), ZX_OK);
}

TEST_F(AmlPwmDriverTest, SetPwmConfigInvalidPwmId) {
  for (Mode mode : {Mode::kOn, Mode::kOff, Mode::kTwoTimer, Mode::kDeltaSigma}) {
    mode_config fail{
        .mode = mode,
    };
    auto fail_cfg = CreatePwmConfig(false, 1250, 100.0, &fail);
    // Incorrect pwm ID.
    EXPECT_NE(SetPwmConfig(10, &fail_cfg), ZX_OK);
  }
}

TEST_F(AmlPwmDriverTest, SetPwmConfigInvalidTimer1PeriodExceedsLimit) {
  mode_config fail_mode{.mode = aml_pwm::Mode::kOn, .regular = {}};
  fuchsia_hardware_pwm::wire::PwmConfig fail_cfg =
      CreatePwmConfig(false, 1'000'000'000, 100.0, &fail_mode);
  EXPECT_NE(SetPwmConfig(0, &fail_cfg), ZX_OK);
}

TEST_F(AmlPwmDriverTest, SetPwmConfigInvalidTwoTimerModeTimer2PeriodExceedsLimit) {
  mode_config fail_mode{
      .mode = aml_pwm::Mode::kTwoTimer,
      .two_timer =
          {
              // period = 1 second, exceeds the maximum allowed period (343'927'680 ns).
              .period_ns2 = 1'000'000'000,
          },
  };
  auto fail_cfg = CreatePwmConfig(false, 1000, 100.0, &fail_mode);
  EXPECT_NE(SetPwmConfig(0, &fail_cfg), ZX_OK);
}

TEST_F(AmlPwmDriverTest, SetPwmConfigTest) {
  // Mode::kOff
  mode_config off{.mode = Mode::kOff, .regular = {}};
  auto off_cfg = CreatePwmConfig(false, 1250, 100.0, &off);
  EXPECT_OK(SetPwmConfig(0, &off_cfg));

  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0x01000000).ExpectWrite(0x01000001);  // SetMode
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFF80FF);  // SetClockDivider
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFBFFFFFF);  // Invert
    (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x10000000);  // EnableConst
    (*mmios[0])[0 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x001E0000);  // SetDutyCycle
  });
  mode_config on{.mode = Mode::kOn, .regular = {}};
  auto on_cfg = CreatePwmConfig(false, 1250, 100.0, &on);
  EXPECT_OK(SetPwmConfig(0, &on_cfg));  // turn on

  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFDFFFFFA);  // SetMode
  });
  EXPECT_OK(SetPwmConfig(0, &off_cfg));
  EXPECT_OK(SetPwmConfig(0, &off_cfg));  // same configs

  // Mode::kOn
  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0x01000000).ExpectWrite(0x00000002);  // SetMode
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFF80FFFF);  // SetClockDivider
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xF7FFFFFF);  // Invert
    (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x20000000);  // EnableConst
    (*mmios[0])[1 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x001E0000);  // SetDutyCycle
  });
  EXPECT_OK(SetPwmConfig(1, &on_cfg));

  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x08000000);  // Invert
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xDFFFFFFF);  // EnableConst
    (*mmios[0])[1 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x00060010);  // SetDutyCycle
  });
  on_cfg = CreatePwmConfig(true, 1000, 30.0, &on);
  EXPECT_OK(SetPwmConfig(1, &on_cfg));  // Change Duty Cycle

  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFEFFFFF5);  // SetMode
  });
  EXPECT_OK(SetPwmConfig(1, &off_cfg));  // Change Mode

  // Mode::kDeltaSigma
  WithMmios([](auto mmios) {
    (*mmios[1])[2 * 4].ExpectRead(0x02000000).ExpectWrite(0x00000004);  // SetMode
    (*mmios[1])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFF80FF);  // SetClockDivider
    (*mmios[1])[3 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFF0064);  // SetDSSetting
    (*mmios[1])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFBFFFFFF);  // Invert
    (*mmios[1])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xEFFFFFFF);  // EnableConst
    (*mmios[1])[0 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x00060010);  // SetDutyCycle
  });
  mode_config ds{
      .mode = Mode::kDeltaSigma,
      .delta_sigma =
          {
              .delta = 100,
          },
  };
  auto ds_cfg = CreatePwmConfig(false, 1000, 30.0, &ds);
  EXPECT_OK(SetPwmConfig(2, &ds_cfg));

  // Mode::kTwoTimer
  WithMmios([](auto mmios) {
    (*mmios[3])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x01000002);  // SetMode
    (*mmios[3])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFF80FFFF);  // SetClockDivider
    (*mmios[3])[6 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x00130003);  // SetDutyCycle2
    (*mmios[3])[4 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFF0302);  // SetTimers
    (*mmios[3])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xF7FFFFFF);  // Invert
    (*mmios[3])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xDFFFFFFF);  // EnableConst
    (*mmios[3])[1 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x00060010);  // SetDutyCycle
  });
  mode_config timer2{
      .mode = Mode::kTwoTimer,
      .two_timer =
          {
              .period_ns2 = 1000,
              .duty_cycle2 = 80.0,
              .timer1 = 3,
              .timer2 = 2,
          },
  };
  auto timer2_cfg = CreatePwmConfig(false, 1000, 30.0, &timer2);
  EXPECT_OK(SetPwmConfig(7, &timer2_cfg));
}

TEST_F(AmlPwmDriverTest, SingleTimerModeClockDividerChangePwmTest) {
  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0x01000000).ExpectWrite(0x01000001);  // SetMode
    // Expected clock divider value = 1, raw value of divider register field = 0
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFF80FF);  // SetClockDivider
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFBFFFFFF);  // Invert
    (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x10000000);  // EnableConst
    (*mmios[0])[0 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x001E0000);  // SetDutyCycle
  });
  mode_config on{.mode = Mode::kOn, .regular = {}};
  auto on_cfg = CreatePwmConfig(false, 1250, 100.0, &on);
  EXPECT_OK(SetPwmConfig(0, &on_cfg));  // Success

  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x10000000);  // EnableConst
    (*mmios[0])[0 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x5F460000);  // SetDutyCycle
  });
  on_cfg = CreatePwmConfig(false, 1'000'000, 100.0, &on);
  EXPECT_OK(SetPwmConfig(0, &on_cfg));  // Success

  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFF81FF);  // SetClockDivider
    (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x10000000);  // EnableConst
    (*mmios[0])[0 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x8EE90000);  // SetDutyCycle
  });
  on_cfg = CreatePwmConfig(false, 3'000'000, 100.0, &on);
  EXPECT_OK(SetPwmConfig(0, &on_cfg));  // Success

  on_cfg = CreatePwmConfig(false, 1'000'000'000, 100.0, &on);
  EXPECT_NE(SetPwmConfig(0, &on_cfg), ZX_OK);  // Failure
}

TEST_F(AmlPwmDriverTest, TwoTimerModeClockDividerChangePwmTest) {
  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x01000002);  // SetMode
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFF80FFFF);  // SetClockDivider
    (*mmios[0])[6 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x00130003);  // SetDutyCycle2
    (*mmios[0])[4 * 4]
        .ExpectRead(0xFFFFFFFF)
        .ExpectWrite(0xFFFF0302);  // SetTimers sets b1=3, b2=2 at [15:0]
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xF7FFFFFF);  // Invert
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xDFFFFFFF);  // EnableConst
    (*mmios[0])[1 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x00060010);  // SetDutyCycle
  });
  mode_config timer2{
      .mode = Mode::kTwoTimer,
      .two_timer =
          {
              .period_ns2 = 1000,
              .duty_cycle2 = 80.0,
              .timer1 = 3,
              .timer2 = 2,
          },
  };
  auto timer2_cfg = CreatePwmConfig(false, 1000, 30.0, &timer2);
  EXPECT_OK(SetPwmConfig(1, &timer2_cfg));

  WithMmios([](auto mmios) {
    // timer1 needs divider = 2, timer2 needs divider = 1,
    // so the divider = max(2, 1) = 2. The raw value is set to (2 - 1) = 1.
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFF81FFFF);  // SetClockDivider
    (*mmios[0])[6 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x00090001);  // SetDutyCycle2
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xDFFFFFFF);  // EnableConst
    (*mmios[0])[1 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x2ADF6408);  // SetDutyCycle
  });
  timer2_cfg = CreatePwmConfig(false, 3'000'000, 30.0, &timer2);
  EXPECT_OK(SetPwmConfig(1, &timer2_cfg));  // Success

  WithMmios([](auto mmios) {
    // timer1 needs divider = 2, timer2 needs divider = 3,
    // so the divider = max(2, 3) = 3. The raw value is set to (3 - 1) = 2.
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFF82FFFF);  // SetClockDivider
    (*mmios[0])[6 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x986F261B);  // SetDutyCycle2
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xDFFFFFFF);  // EnableConst
    (*mmios[0])[1 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x1C9442B0);  // SetDutyCycle
  });
  timer2.two_timer.period_ns2 = 6'000'000;
  timer2_cfg = CreatePwmConfig(false, 3'000'000, 30.0, &timer2);
  EXPECT_OK(SetPwmConfig(1, &timer2_cfg));  // Success
}

TEST_F(AmlPwmDriverTest, SetPwmConfigFailTest) {
  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0x01000000).ExpectWrite(0x01000001);  // SetMode
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFF80FF);  // SetClockDivider
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFBFFFFFF);  // Invert
    (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x10000000);  // EnableConst
    (*mmios[0])[0 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x001E0000);  // SetDutyCycle
  });
  mode_config on{.mode = Mode::kOn, .regular = {}};
  auto on_cfg = CreatePwmConfig(false, 1250, 100.0, &on);
  EXPECT_OK(SetPwmConfig(0, &on_cfg));  // Success

  WithMmios([](auto mmios) {
    // Nothing should happen on the register if the input is incorrect.
    (*mmios[0])[2 * 4].VerifyAndClear();
    (*mmios[0])[0 * 4].VerifyAndClear();
  });
  on_cfg = CreatePwmConfig(true, 1250, 120.0, &on);
  EXPECT_NE(SetPwmConfig(0, &on_cfg), ZX_OK);  // Fail
}

TEST_F(AmlPwmDriverTest, EnablePwmTest) {
  EXPECT_NE(EnablePwm(10), ZX_OK);  // Fail

  WithMmios([](auto mmios) { (*mmios[1])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x00008000); });
  EXPECT_OK(EnablePwm(2));
  EXPECT_OK(EnablePwm(2));  // Enable twice

  WithMmios([](auto mmios) { (*mmios[2])[2 * 4].ExpectRead(0x00008000).ExpectWrite(0x00808000); });
  EXPECT_OK(EnablePwm(5));  // Enable other PWMs
}

TEST_F(AmlPwmDriverTest, DisablePwmTest) {
  EXPECT_NE(DisablePwm(10), ZX_OK);  // Fail

  EXPECT_OK(DisablePwm(0));  // Disable first

  WithMmios([](auto mmios) { (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x00008000); });
  EXPECT_OK(EnablePwm(0));

  WithMmios([](auto mmios) { (*mmios[0])[2 * 4].ExpectRead(0x00008000).ExpectWrite(0x00000000); });
  EXPECT_OK(DisablePwm(0));
  EXPECT_OK(DisablePwm(0));  // Disable twice

  WithMmios([](auto mmios) { (*mmios[2])[2 * 4].ExpectRead(0x00008000).ExpectWrite(0x00808000); });
  EXPECT_OK(EnablePwm(5));  // Enable other PWMs

  WithMmios([](auto mmios) { (*mmios[2])[2 * 4].ExpectRead(0x00808000).ExpectWrite(0x00008000); });
  EXPECT_OK(DisablePwm(5));  // Disable other PWMs
}

TEST_F(AmlPwmDriverTest, SetPwmConfigPeriodNotDivisibleBy100Test) {
  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4].ExpectRead(0x01000000).ExpectWrite(0x01000001);  // SetMode
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFF80FF);  // SetClockDivider
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFBFFFFFF);  // Invert
    (*mmios[0])[2 * 4].ExpectRead(0x00000000).ExpectWrite(0x10000000);  // EnableConst
    (*mmios[0])[0 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x10420000);  // SetDutyCycle
  });
  mode_config on{.mode = Mode::kOn, .regular = {}};
  auto on_cfg = CreatePwmConfig(false, 170625, 100.0, &on);
  EXPECT_OK(SetPwmConfig(0, &on_cfg));  // Success
}

TEST_F(AmlPwmDriverTest, TwoTimerChannelSeparationPwmTest) {
  mode_config timer2{
      .mode = Mode::kTwoTimer,
      .two_timer =
          {
              .period_ns2 = 1000,
              .duty_cycle2 = 80.0,
              .timer1 = 3,
              .timer2 = 2,
          },
  };
  const auto timer2_cfg = CreatePwmConfig(false, 1000, 30.0, &timer2);

  WithMmios([](auto mmios) {
    (*mmios[0])[2 * 4]
        .ExpectRead(0x00000000)
        .ExpectWrite(0x02000001);  // SetMode sets en_a and en_a2 (bit 25)
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFF80FF);  // SetClockDivider
    (*mmios[0])[5 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x00130003);  // SetDutyCycle2
    (*mmios[0])[4 * 4]
        .ExpectRead(0xFFFFFFFF)
        .ExpectWrite(0x0302FFFF);  // SetTimers sets a1=3, a2=2 at [31:16]
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFBFFFFFF);  // Invert
    (*mmios[0])[2 * 4].ExpectRead(0xFFFFFFFF).ExpectWrite(0xEFFFFFFF);  // EnableConst
    (*mmios[0])[0 * 4].ExpectRead(0xA39D9259).ExpectWrite(0x00060010);  // SetDutyCycle
  });
  EXPECT_OK(SetPwmConfig(0, &timer2_cfg));
}

class AmlPwmDriverGenericMetadataTest : public ::testing::Test {
 protected:
  void TearDown() override { ASSERT_OK(driver_test_.StopDriver()); }

  fdf_testing::ForegroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(AmlPwmDriverGenericMetadataTest, GenericMetadataTest) {
  std::vector<fuchsia_driver_metadata::DictionaryEntry> entries;
  entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
      "channels._count", fuchsia_driver_metadata::DictionaryValue::WithInt64(1)));
  entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
      "channels.0.channel", fuchsia_driver_metadata::DictionaryValue::WithInt64(0)));
  entries.push_back(fuchsia_driver_metadata::DictionaryEntry(
      "channels.0.period_ns", fuchsia_driver_metadata::DictionaryValue::WithInt64(1000)));

  fuchsia_driver_metadata::Dictionary dict{{.entries = std::move(entries)}};

  driver_test().RunInEnvironmentTypeContext(
      [dict = std::move(dict)](auto& env) mutable { env.InitGeneric(dict); });
  ASSERT_OK(driver_test().StartDriver());
}

}  // namespace

}  // namespace pwm
