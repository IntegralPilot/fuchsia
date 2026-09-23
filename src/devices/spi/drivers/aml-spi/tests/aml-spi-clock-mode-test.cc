// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/spi/drivers/aml-spi/tests/aml-spi-test-env.h"

namespace spi {

class AmlSpiNormalClockModeTestEnvironment : public BaseTestEnvironment {
  void SetMetadata(fdf_fake::FakePDev& pdev) override {
    static const fuchsia_hardware_amlogic_metadata::SpiConfig kSpiConfig({
        .bus_id = 0,
        .cs = {5, 3},
        .clock_divider_register_value = 0x5,
        .use_enhanced_clock_mode = false,
        .delay_control = fuchsia_hardware_amlogic_metadata::kDefaultDelayControl,
    });

    EXPECT_OK(pdev.AddFidlMetadata(fuchsia_hardware_amlogic_metadata::SpiConfig::kSerializableName,
                                   kSpiConfig));
  }
};

class AmlSpiNormalClockModeFixtureConfig final {
 public:
  using DriverType = TestAmlSpiDriver;
  using EnvironmentType = AmlSpiNormalClockModeTestEnvironment;
};

class AmlSpiNormalClockModeTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<AmlSpiNormalClockModeFixtureConfig>& driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::ForegroundDriverTest<AmlSpiNormalClockModeFixtureConfig> driver_test_;
};

TEST_F(AmlSpiNormalClockModeTest, Test) {
  auto conreg = ConReg::Get().FromValue(driver_test().driver()->conreg());
  auto enhanced_cntl = EnhanceCntl::Get().FromValue(driver_test().driver()->enhance_cntl());
  auto testreg = TestReg::Get().FromValue(driver_test().driver()->testreg());

  EXPECT_EQ(conreg.data_rate(), 0x5u);
  EXPECT_EQ(conreg.drctl(), 0u);
  EXPECT_EQ(conreg.ssctl(), 0u);
  EXPECT_EQ(conreg.smc(), 0u);
  EXPECT_EQ(conreg.xch(), 0u);
  EXPECT_EQ(conreg.mode(), ConReg::kModeMaster);
  EXPECT_EQ(conreg.en(), 1u);

  EXPECT_EQ(enhanced_cntl.reg_value(), 0u);

  EXPECT_EQ(testreg.dlyctl(), 0x15u);
  EXPECT_EQ(testreg.clk_free_en(), 1u);
}

class AmlSpiEnhancedClockModeEnvironment : public BaseTestEnvironment {
  void SetMetadata(fdf_fake::FakePDev& pdev) override {
    static const fuchsia_hardware_amlogic_metadata::SpiConfig kSpiConfig({
        .bus_id = 0,
        .cs = {5, 3},
        .clock_divider_register_value = 0xa5,
        .use_enhanced_clock_mode = true,
        .delay_control = 0b00'11'00,
    });

    EXPECT_OK(pdev.AddFidlMetadata(fuchsia_hardware_amlogic_metadata::SpiConfig::kSerializableName,
                                   kSpiConfig));
  }
};

class AmlSpiEnhancedClockModeFixtureConfig final {
 public:
  using DriverType = TestAmlSpiDriver;
  using EnvironmentType = AmlSpiEnhancedClockModeEnvironment;
};

class AmlSpiEnhancedClockModeTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<AmlSpiEnhancedClockModeFixtureConfig>& driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::ForegroundDriverTest<AmlSpiEnhancedClockModeFixtureConfig> driver_test_;
};

TEST_F(AmlSpiEnhancedClockModeTest, Test) {
  auto conreg = ConReg::Get().FromValue(driver_test().driver()->conreg());
  auto enhanced_cntl = EnhanceCntl::Get().FromValue(driver_test().driver()->enhance_cntl());
  auto testreg = TestReg::Get().FromValue(driver_test().driver()->testreg());

  EXPECT_EQ(conreg.data_rate(), 0u);
  EXPECT_EQ(conreg.drctl(), 0u);
  EXPECT_EQ(conreg.ssctl(), 0u);
  EXPECT_EQ(conreg.smc(), 0u);
  EXPECT_EQ(conreg.xch(), 0u);
  EXPECT_EQ(conreg.mode(), ConReg::kModeMaster);
  EXPECT_EQ(conreg.en(), 1u);

  EXPECT_EQ(enhanced_cntl.main_clock_always_on(), 0u);
  EXPECT_EQ(enhanced_cntl.clk_cs_delay_enable(), 1u);
  EXPECT_EQ(enhanced_cntl.cs_oen_enhance_enable(), 1u);
  EXPECT_EQ(enhanced_cntl.clk_oen_enhance_enable(), 1u);
  EXPECT_EQ(enhanced_cntl.mosi_oen_enhance_enable(), 1u);
  EXPECT_EQ(enhanced_cntl.spi_clk_select(), 1u);
  EXPECT_EQ(enhanced_cntl.enhance_clk_div(), 0xa5u);
  EXPECT_EQ(enhanced_cntl.clk_cs_delay(), 0u);

  EXPECT_EQ(testreg.dlyctl(), 0b00'11'00u);
  EXPECT_EQ(testreg.clk_free_en(), 1u);
}

class AmlSpiNormalClockModeInvalidDividerEnvironment : public BaseTestEnvironment {
  void SetMetadata(fdf_fake::FakePDev& pdev) override {
    static const fuchsia_hardware_amlogic_metadata::SpiConfig kSpiConfig({
        .bus_id = 0,
        .cs = {5, 3},
        .clock_divider_register_value = 0xa5,
        .use_enhanced_clock_mode = false,
        .delay_control = fuchsia_hardware_amlogic_metadata::kDefaultDelayControl,
    });

    EXPECT_OK(pdev.AddFidlMetadata(fuchsia_hardware_amlogic_metadata::SpiConfig::kSerializableName,
                                   kSpiConfig));
  }
};

class AmlSpiNormalClockModeInvalidDividerFixtureConfig final {
 public:
  using DriverType = TestAmlSpiDriver;
  using EnvironmentType = AmlSpiNormalClockModeInvalidDividerEnvironment;
};

class AmlSpiNormalClockModeInvalidDividerTest : public ::testing::Test {
 public:
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<AmlSpiNormalClockModeInvalidDividerFixtureConfig>&
  driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::ForegroundDriverTest<AmlSpiNormalClockModeInvalidDividerFixtureConfig> driver_test_;
};

TEST_F(AmlSpiNormalClockModeInvalidDividerTest, Test) {
  EXPECT_TRUE(driver_test().StartDriver().is_error());
}

class AmlSpiEnhancedClockModeInvalidDividerEnvironment : public BaseTestEnvironment {
 public:
  void SetMetadata(fdf_fake::FakePDev& pdev) override {
    static const fuchsia_hardware_amlogic_metadata::SpiConfig kSpiConfig({
        .bus_id = 0,
        .cs = {5, 3},
        .clock_divider_register_value = 0x1a5,
        .use_enhanced_clock_mode = true,
        .delay_control = fuchsia_hardware_amlogic_metadata::kDefaultDelayControl,
    });

    EXPECT_OK(pdev.AddFidlMetadata(fuchsia_hardware_amlogic_metadata::SpiConfig::kSerializableName,
                                   kSpiConfig));
  }
};

class AmlSpiEnhancedClockModeInvalidDividerFixtureConfig final {
 public:
  using DriverType = TestAmlSpiDriver;
  using EnvironmentType = AmlSpiEnhancedClockModeInvalidDividerEnvironment;
};

class AmlSpiEnhancedClockModeInvalidDividerTest : public ::testing::Test {
 public:
  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::ForegroundDriverTest<AmlSpiEnhancedClockModeInvalidDividerFixtureConfig>&
  driver_test() {
    return driver_test_;
  }

 private:
  fdf_testing::ForegroundDriverTest<AmlSpiEnhancedClockModeInvalidDividerFixtureConfig>
      driver_test_;
};

TEST_F(AmlSpiEnhancedClockModeInvalidDividerTest, Test) {
  EXPECT_TRUE(driver_test().StartDriver().is_error());
}

}  // namespace spi
