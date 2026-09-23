// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "aml-nna.h"

#include <lib/ddk/platform-defs.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/driver/testing/cpp/driver_test.h>

#include <gtest/gtest.h>
#include <mock-mmio-reg/mock-mmio-reg.h>

#include "src/devices/registers/testing/mock-registers/mock-registers.h"
#include "src/lib/testing/predicates/status.h"

namespace aml_nna {

class AmlNnaTestEnvironment : public fdf_testing::Environment {
 public:
  void Init(uint32_t pid) {
    std::map<uint32_t, fdf_fake::Mmio> mmios;
    mmios.insert({AmlNnaDriver::kHiuMmioIndex, hiu_mmio_.GetMmioBuffer()});
    mmios.insert({AmlNnaDriver::kPowerDomainMmioIndex, power_mmio_.GetMmioBuffer()});
    mmios.insert({AmlNnaDriver::kMemoryDomainMmioIndex, memory_pd_mmio_.GetMmioBuffer()});

    pdev_.SetConfig({.mmios = std::move(mmios),
                     .board_info{{
                         .vid = PDEV_VID_AMLOGIC,
                         .pid = pid,
                     }}});
  }

  zx::result<> Serve(fdf::OutgoingDirectory& to_driver_vfs) override {
    auto* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_platform_device::Service>(
          pdev_.GetInstanceHandler(dispatcher), AmlNnaDriver::kPlatformDeviceParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    {
      zx::result result = to_driver_vfs.AddService<fuchsia_hardware_registers::Service>(
          reset_register_.GetInstanceHandler(), AmlNnaDriver::kResetRegisterParentName);
      if (result.is_error()) {
        return result.take_error();
      }
    }

    return zx::ok();
  }

  ddk_mock::MockMmioRegRegion& hiu_mmio() { return hiu_mmio_; }
  ddk_mock::MockMmioRegRegion& power_mmio() { return power_mmio_; }
  ddk_mock::MockMmioRegRegion& memory_pd_mmio() { return memory_pd_mmio_; }
  mock_registers::MockRegisters& reset_register() { return reset_register_; }

 private:
  fdf_fake::FakePDev pdev_;
  ddk_mock::MockMmioRegRegion hiu_mmio_{sizeof(uint32_t), 0x2000 / sizeof(uint32_t)};
  ddk_mock::MockMmioRegRegion power_mmio_{sizeof(uint32_t), 0x1000 / sizeof(uint32_t)};
  ddk_mock::MockMmioRegRegion memory_pd_mmio_{sizeof(uint32_t), 0x1000 / sizeof(uint32_t)};
  mock_registers::MockRegisters reset_register_{fdf::Dispatcher::GetCurrent()->async_dispatcher()};
};

class FixtureConfig final {
 public:
  using DriverType = AmlNnaDriver;
  using EnvironmentType = AmlNnaTestEnvironment;
};

class AmlNnaTest : public ::testing::Test {
 public:
  void StartDriver(uint32_t vid) {
    driver_test_.RunInEnvironmentTypeContext([vid](auto& env) { env.Init(vid); });
    ASSERT_OK(driver_test_.StartDriver());
  }

  void TearDown() override {
    ASSERT_OK(driver_test_.StopDriver());
    driver_test_.RunInEnvironmentTypeContext([](auto& env) {
      env.hiu_mmio().VerifyAll();
      env.power_mmio().VerifyAll();
      env.memory_pd_mmio().VerifyAll();
      EXPECT_OK(env.reset_register().VerifyAll());
    });
  }

 protected:
  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;
};

TEST_F(AmlNnaTest, InitT931) {
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    auto& power_mmio = env.power_mmio();
    power_mmio[0x3a * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFCFFFF);
    power_mmio[0x3b * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFCFFFF);

    auto& memory_pd_mmio = env.memory_pd_mmio();
    memory_pd_mmio[0x43 * sizeof(uint32_t)].ExpectWrite(0);
    memory_pd_mmio[0x44 * sizeof(uint32_t)].ExpectWrite(0);

    auto& reset_register = env.reset_register();
    reset_register.template ExpectWrite<uint32_t>(0x88, 1 << 12, 0);
    reset_register.template ExpectWrite<uint32_t>(0x88, 1 << 12, 1 << 12);

    auto& hiu_mmio = env.hiu_mmio();
    hiu_mmio[0x72 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x700);
    hiu_mmio[0x72 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x7000000);
  });

  StartDriver(PDEV_PID_AMLOGIC_T931);
}

TEST_F(AmlNnaTest, InitS905d3) {
  driver_test().RunInEnvironmentTypeContext([](auto& env) {
    auto& power_mmio = env.power_mmio();
    power_mmio[0x3a * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFEFFFF);
    power_mmio[0x3b * sizeof(uint32_t)].ExpectRead(0xFFFFFFFF).ExpectWrite(0xFFFEFFFF);

    auto& memory_pd_mmio = env.memory_pd_mmio();
    memory_pd_mmio[0x46 * sizeof(uint32_t)].ExpectWrite(0);
    memory_pd_mmio[0x47 * sizeof(uint32_t)].ExpectWrite(0);

    auto& reset_register = env.reset_register();
    reset_register.template ExpectWrite<uint32_t>(0x88, 1 << 12, 0);
    reset_register.template ExpectWrite<uint32_t>(0x88, 1 << 12, 1 << 12);

    auto& hiu_mmio = env.hiu_mmio();
    hiu_mmio[0x72 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x700);
    hiu_mmio[0x72 * sizeof(uint32_t)].ExpectRead(0x00000000).ExpectWrite(0x7000000);
  });

  StartDriver(PDEV_PID_AMLOGIC_S905D3);
}

}  // namespace aml_nna
