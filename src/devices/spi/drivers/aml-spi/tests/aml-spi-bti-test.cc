// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/driver/fake-bti/cpp/fake-bti.h>

#include "src/devices/spi/drivers/aml-spi/tests/aml-spi-test-env.h"

namespace spi {

namespace {
bool IsBytesEqual(const uint8_t* expected, const uint8_t* actual, size_t len) {
  return memcmp(expected, actual, len) == 0;
}
}  // namespace

class AmlSpiBtiPaddrEnvironment : public BaseTestEnvironment {
 public:
  static constexpr zx_paddr_t kDmaPaddrs[] = {0x1212'0000, 0xabab'000};

  std::optional<zx::bti> CreateBti() override {
    zx::result bti = fake_bti::CreateFakeBtiWithPaddrs(kDmaPaddrs);
    EXPECT_OK(bti);
    bti_local_ = bti->borrow();
    return std::move(bti.value());
  }

  zx::unowned_bti GetBtiLocal() { return bti_local_->borrow(); }

 private:
  zx::unowned_bti bti_local_;
};

class AmlSpiBtiPaddrFixtureConfig final {
 public:
  using DriverType = TestAmlSpiDriver;
  using EnvironmentType = AmlSpiBtiPaddrEnvironment;
};

class AmlSpiBtiPaddrTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<AmlSpiBtiPaddrFixtureConfig>& driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::ForegroundDriverTest<AmlSpiBtiPaddrFixtureConfig> driver_test_;
};

TEST_F(AmlSpiBtiPaddrTest, ExchangeDma) {
  constexpr uint8_t kTxData[24] = {
      0x3c, 0xa7, 0x5f, 0xc8, 0x4b, 0x0b, 0xdf, 0xef, 0xb9, 0xa0, 0xcb, 0xbd,
      0xd4, 0xcf, 0xa8, 0xbf, 0x85, 0xf2, 0x6a, 0xe3, 0xba, 0xf1, 0x49, 0x00,
  };
  constexpr uint8_t kExpectedRxData[24] = {
      0xea, 0x2b, 0x8f, 0x8f, 0xea, 0x2b, 0x8f, 0x8f, 0xea, 0x2b, 0x8f, 0x8f,
      0xea, 0x2b, 0x8f, 0x8f, 0xea, 0x2b, 0x8f, 0x8f, 0xea, 0x2b, 0x8f, 0x8f,
  };

  uint8_t reversed_tx_data[24];
  for (size_t i = 0; i < sizeof(kTxData); i += sizeof(uint64_t)) {
    uint64_t tmp;
    memcpy(&tmp, kTxData + i, sizeof(tmp));
    tmp = htobe64(tmp);
    memcpy(reversed_tx_data + i, &tmp, sizeof(tmp));
  }

  uint8_t reversed_expected_rx_data[24];
  for (size_t i = 0; i < sizeof(kExpectedRxData); i += sizeof(uint64_t)) {
    uint64_t tmp;
    memcpy(&tmp, kExpectedRxData + i, sizeof(tmp));
    tmp = htobe64(tmp);
    memcpy(reversed_expected_rx_data + i, &tmp, sizeof(tmp));
  }

  auto spiimpl_client = driver_test().Connect<fuchsia_hardware_spiimpl::Service::Device>();
  ASSERT_TRUE(spiimpl_client.is_ok());

  fdf::WireClient<fuchsia_hardware_spiimpl::SpiImpl> spiimpl(*std::move(spiimpl_client),
                                                             fdf::Dispatcher::GetCurrent()->get());

  std::vector<fake_bti::FakeBtiPinnedVmoInfo> dma_vmos;
  driver_test().RunInEnvironmentTypeContext([&dma_vmos](AmlSpiBtiPaddrEnvironment& env) {
    zx::result vmos = fake_bti::GetPinnedVmo(env.GetBtiLocal());
    EXPECT_OK(vmos);
    dma_vmos = std::move(vmos.value());
  });
  ASSERT_EQ(dma_vmos.size(), 2u);

  zx::vmo tx_dma_vmo{std::move(dma_vmos[0].vmo)};
  zx::vmo rx_dma_vmo{std::move(dma_vmos[1].vmo)};

  // Copy the reversed expected RX data to the RX VMO. The driver should copy this to the user
  // output buffer with the correct endianness.
  rx_dma_vmo.write(reversed_expected_rx_data, 0, sizeof(reversed_expected_rx_data));

  zx_paddr_t tx_paddr = 0;
  zx_paddr_t rx_paddr = 0;

  driver_test().driver()->mmio()[AML_SPI_DRADDR].SetWriteCallback(
      [&tx_paddr](uint64_t value) { tx_paddr = value; });
  driver_test().driver()->mmio()[AML_SPI_DWADDR].SetWriteCallback(
      [&rx_paddr](uint64_t value) { rx_paddr = value; });

  uint8_t buf[24] = {};
  memcpy(buf, kTxData, sizeof(buf));

  fdf::Arena arena('TEST');
  spiimpl.buffer(arena)
      ->ExchangeVector(0, fidl::VectorView<uint8_t>::FromExternal(buf, sizeof(buf)))
      .Then([&](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        ASSERT_EQ(result->value()->rxdata.size(), sizeof(buf));
        EXPECT_TRUE(IsBytesEqual(kExpectedRxData, result->value()->rxdata.data(), sizeof(buf)));
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();

  // Verify that the driver wrote the TX data to the TX VMO.
  EXPECT_OK(tx_dma_vmo.read(buf, 0, sizeof(buf)));
  EXPECT_TRUE(IsBytesEqual(reversed_tx_data, buf, sizeof(buf)));

  EXPECT_EQ(tx_paddr, AmlSpiBtiPaddrEnvironment::kDmaPaddrs[0]);
  EXPECT_EQ(rx_paddr, AmlSpiBtiPaddrEnvironment::kDmaPaddrs[1]);

  driver_test().RunInEnvironmentTypeContext([](BaseTestEnvironment& env) {
    EXPECT_FALSE(env.ControllerReset());
    EXPECT_EQ(env.cs_toggle_count(), 2u);
  });
}

class AmlSpiBtiEmptyEnvironment : public BaseTestEnvironment {
 public:
  static constexpr zx_paddr_t kDmaPaddrs[] = {0x1212'0000, 0xabab'000};

  std::optional<zx::bti> CreateBti() override {
    zx::result bti = fake_bti::CreateFakeBti();
    EXPECT_OK(bti);
    bti_local_ = bti->borrow();
    return std::move(bti.value());
  }

  zx::unowned_bti& GetBtiLocal() { return bti_local_; }

 private:
  zx::unowned_bti bti_local_;
};

class AmlSpiBtiEmptyFixtureConfig final {
 public:
  using DriverType = TestAmlSpiDriver;
  using EnvironmentType = AmlSpiBtiPaddrEnvironment;
};

class AmlSpiBtiEmptyTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<AmlSpiBtiEmptyFixtureConfig>& driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::ForegroundDriverTest<AmlSpiBtiEmptyFixtureConfig> driver_test_;
};

TEST_F(AmlSpiBtiEmptyTest, ExchangeFallBackToPio) {
  constexpr uint8_t kTxData[15] = {
      0x3c, 0xa7, 0x5f, 0xc8, 0x4b, 0x0b, 0xdf, 0xef, 0xb9, 0xa0, 0xcb, 0xbd, 0xd4, 0xcf, 0xa8,
  };
  constexpr uint8_t kExpectedRxData[15] = {
      0xea, 0x2b, 0x8f, 0x8f, 0xea, 0x2b, 0x8f, 0x8f, 0x8f, 0x8f, 0x8f, 0x8f, 0x8f, 0x8f, 0x8f,
  };

  auto spiimpl_client = driver_test().Connect<fuchsia_hardware_spiimpl::Service::Device>();
  ASSERT_TRUE(spiimpl_client.is_ok());

  fdf::WireClient<fuchsia_hardware_spiimpl::SpiImpl> spiimpl(*std::move(spiimpl_client),
                                                             fdf::Dispatcher::GetCurrent()->get());

  std::vector<fake_bti::FakeBtiPinnedVmoInfo> dma_vmos;
  driver_test().RunInEnvironmentTypeContext([&dma_vmos](AmlSpiBtiPaddrEnvironment& env) {
    zx::result vmos = fake_bti::GetPinnedVmo(env.GetBtiLocal());
    EXPECT_OK(vmos);
    dma_vmos = std::move(vmos.value());
  });
  ASSERT_EQ(dma_vmos.size(), 2u);

  zx_paddr_t tx_paddr = 0;
  zx_paddr_t rx_paddr = 0;

  driver_test().driver()->mmio()[AML_SPI_DRADDR].SetWriteCallback(
      [&tx_paddr](uint64_t value) { tx_paddr = value; });
  driver_test().driver()->mmio()[AML_SPI_DWADDR].SetWriteCallback(
      [&rx_paddr](uint64_t value) { rx_paddr = value; });

  driver_test().driver()->mmio()[AML_SPI_RXDATA].SetReadCallback([]() { return 0xea2b'8f8f; });

  uint64_t tx_data = 0;
  driver_test().driver()->mmio()[AML_SPI_TXDATA].SetWriteCallback(
      [&tx_data](uint64_t value) { tx_data = value; });

  uint8_t buf[15] = {};
  memcpy(buf, kTxData, sizeof(buf));

  fdf::Arena arena('TEST');
  spiimpl.buffer(arena)
      ->ExchangeVector(0, fidl::VectorView<uint8_t>::FromExternal(buf, sizeof(buf)))
      .Then([&](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        ASSERT_EQ(result->value()->rxdata.size(), sizeof(buf));
        EXPECT_TRUE(IsBytesEqual(kExpectedRxData, result->value()->rxdata.data(), sizeof(buf)));
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();

  EXPECT_EQ(tx_data, kTxData[14]);

  // Verify that DMA was not used.
  EXPECT_EQ(tx_paddr, 0u);
  EXPECT_EQ(rx_paddr, 0u);

  driver_test().RunInEnvironmentTypeContext([](BaseTestEnvironment& env) {
    EXPECT_FALSE(env.ControllerReset());
    EXPECT_EQ(env.cs_toggle_count(), 2u);
  });
}

class AmlSpiExchangeDmaClientReversesBufferEnvironment : public AmlSpiBtiPaddrEnvironment {
 public:
  void SetMetadata(fdf_fake::FakePDev& pdev) override {
    static const fuchsia_hardware_amlogic_metadata::SpiConfig kSpiConfig({
        .bus_id = 0,
        .cs = {5, 3, fuchsia_hardware_amlogic_metadata::kCsClientManaged},
        .clock_divider_register_value = 0,
        .use_enhanced_clock_mode = false,
        .client_reverses_dma_transfers = true,
        .delay_control = fuchsia_hardware_amlogic_metadata::kDefaultDelayControl,
    });

    EXPECT_OK(pdev.AddFidlMetadata(fuchsia_hardware_amlogic_metadata::SpiConfig::kSerializableName,
                                   kSpiConfig));
  }
};

class AmlSpiExchangeDmaClientReversesBufferConfig final {
 public:
  using DriverType = TestAmlSpiDriver;
  using EnvironmentType = AmlSpiExchangeDmaClientReversesBufferEnvironment;
};

class AmlSpiExchangeDmaClientReversesBufferTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<AmlSpiExchangeDmaClientReversesBufferConfig>& driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::ForegroundDriverTest<AmlSpiExchangeDmaClientReversesBufferConfig> driver_test_;
};

TEST_F(AmlSpiExchangeDmaClientReversesBufferTest, Test) {
  constexpr uint8_t kTxData[24] = {
      0x3c, 0xa7, 0x5f, 0xc8, 0x4b, 0x0b, 0xdf, 0xef, 0xb9, 0xa0, 0xcb, 0xbd,
      0xd4, 0xcf, 0xa8, 0xbf, 0x85, 0xf2, 0x6a, 0xe3, 0xba, 0xf1, 0x49, 0x00,
  };
  constexpr uint8_t kExpectedRxData[24] = {
      0xea, 0x2b, 0x8f, 0x8f, 0xea, 0x2b, 0x8f, 0x8f, 0xea, 0x2b, 0x8f, 0x8f,
      0xea, 0x2b, 0x8f, 0x8f, 0xea, 0x2b, 0x8f, 0x8f, 0xea, 0x2b, 0x8f, 0x8f,
  };

  auto spiimpl_client = driver_test().Connect<fuchsia_hardware_spiimpl::Service::Device>();
  ASSERT_TRUE(spiimpl_client.is_ok());

  fdf::WireClient<fuchsia_hardware_spiimpl::SpiImpl> spiimpl(*std::move(spiimpl_client),
                                                             fdf::Dispatcher::GetCurrent()->get());

  std::vector<fake_bti::FakeBtiPinnedVmoInfo> dma_vmos;
  driver_test().RunInEnvironmentTypeContext([&dma_vmos](AmlSpiBtiPaddrEnvironment& env) {
    zx::result vmos = fake_bti::GetPinnedVmo(env.GetBtiLocal());
    EXPECT_OK(vmos);
    dma_vmos = std::move(vmos.value());
  });
  ASSERT_EQ(dma_vmos.size(), 2u);

  zx::vmo tx_dma_vmo{std::move(dma_vmos[0].vmo)};
  zx::vmo rx_dma_vmo{std::move(dma_vmos[1].vmo)};

  rx_dma_vmo.write(kExpectedRxData, 0, sizeof(kExpectedRxData));

  zx_paddr_t tx_paddr = 0;
  zx_paddr_t rx_paddr = 0;

  driver_test().driver()->mmio()[AML_SPI_DRADDR].SetWriteCallback(
      [&tx_paddr](uint64_t value) { tx_paddr = value; });
  driver_test().driver()->mmio()[AML_SPI_DWADDR].SetWriteCallback(
      [&rx_paddr](uint64_t value) { rx_paddr = value; });

  uint8_t buf[sizeof(kTxData)] = {};
  memcpy(buf, kTxData, sizeof(buf));

  fdf::Arena arena('TEST');
  spiimpl.buffer(arena)
      ->ExchangeVector(0, fidl::VectorView<uint8_t>::FromExternal(buf, sizeof(buf)))
      .Then([&](auto& result) {
        ASSERT_TRUE(result.ok());
        ASSERT_TRUE(result->is_ok());
        ASSERT_EQ(result->value()->rxdata.size(), sizeof(buf));
        EXPECT_TRUE(IsBytesEqual(kExpectedRxData, result->value()->rxdata.data(), sizeof(buf)));
        driver_test().runtime().Quit();
      });
  driver_test().runtime().Run();

  // Verify that the driver wrote the TX data to the TX VMO with the original byte order.
  EXPECT_OK(tx_dma_vmo.read(buf, 0, sizeof(buf)));
  EXPECT_TRUE(IsBytesEqual(kTxData, buf, sizeof(buf)));

  EXPECT_EQ(tx_paddr, AmlSpiBtiPaddrEnvironment::kDmaPaddrs[0]);
  EXPECT_EQ(rx_paddr, AmlSpiBtiPaddrEnvironment::kDmaPaddrs[1]);

  driver_test().RunInEnvironmentTypeContext(
      [](AmlSpiExchangeDmaClientReversesBufferEnvironment& env) {
        EXPECT_FALSE(env.ControllerReset());
        EXPECT_EQ(env.cs_toggle_count(), 2u);
      });
}

}  // namespace spi
