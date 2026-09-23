// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/spi/drivers/aml-spi/tests/aml-spi-test-env.h"

namespace spi {

class AmlSpiShutdownConfig final {
 public:
  using DriverType = TestAmlSpiDriver;
  using EnvironmentType = BaseTestEnvironment;
};

class AmlSpiShutdownTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }

  fdf_testing::ForegroundDriverTest<AmlSpiShutdownConfig>& driver_test() { return driver_test_; }

 private:
  fdf_testing::ForegroundDriverTest<AmlSpiShutdownConfig> driver_test_;
};

TEST_F(AmlSpiShutdownTest, Shutdown) {
  // Must outlive AmlSpi device.
  bool dmareg_cleared = false;
  bool conreg_cleared = false;

  auto spiimpl_client = driver_test().Connect<fuchsia_hardware_spiimpl::Service::Device>();
  ASSERT_TRUE(spiimpl_client.is_ok());

  fdf::WireClient<fuchsia_hardware_spiimpl::SpiImpl> spiimpl(*std::move(spiimpl_client),
                                                             fdf::Dispatcher::GetCurrent()->get());

  uint8_t buf[16] = {};
  fdf::Arena arena('TEST');
  spiimpl.buffer(arena)
      ->ExchangeVector(0, fidl::VectorView<uint8_t>::FromExternal(buf, sizeof(buf)))
      .Then([&](auto& result) {
        ASSERT_TRUE(result.ok() && result->is_ok());
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();

  driver_test().driver()->mmio()[AML_SPI_DMAREG].SetWriteCallback(
      [&dmareg_cleared](uint64_t value) { dmareg_cleared = value == 0; });

  driver_test().driver()->mmio()[AML_SPI_CONREG].SetWriteCallback(
      [&conreg_cleared](uint64_t value) { conreg_cleared = value == 0; });

  EXPECT_TRUE(driver_test().StopDriver().is_ok());

  // All SPI devices have been released at this point, so no further calls can be made.
  driver_test().RunInEnvironmentTypeContext([](BaseTestEnvironment& env) {
    EXPECT_FALSE(env.ControllerReset());
    EXPECT_EQ(env.cs_toggle_count(), 2u);
  });

  driver_test().ShutdownAndDestroyDriver();

  EXPECT_TRUE(dmareg_cleared);
  EXPECT_TRUE(conreg_cleared);
}

}  // namespace spi
