// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.hardware.platform.device/cpp/fidl.h>
#include <lib/async_patterns/testing/cpp/dispatcher_bound.h>
#include <lib/driver/fake-mmio-reg/cpp/fake-mmio-reg.h>
#include <lib/driver/fake-platform-device/cpp/fake-pdev.h>
#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/driver/mmio/testing/cpp/test-helper.h>
#include <lib/driver/testing/cpp/driver_test.h>
#include <lib/zx/vmo.h>

#include <cstdint>

#include <gtest/gtest.h>
#include <soc/aml-a311d/a311d-hw.h>
#include <soc/aml-meson/g12b-clk.h>
#include <soc/aml-s905d2/s905d2-hiu-regs.h>

#include "vim3_clk.h"

namespace vim3_clock {

namespace fclockimpl = fuchsia_hardware_clockimpl;
namespace fpdev = fuchsia_hardware_platform_device;

class FakeMmio {
 public:
  FakeMmio(size_t count) : count_{count}, regs_{sizeof(uint32_t), count} {
    for (size_t reg = 0; reg < count_; reg++) {
      regs_[reg * sizeof(uint32_t)].SetReadCallback(
          [reg, this]() { return values_.find(reg) == values_.end() ? 0 : values_.at(reg); });

      regs_[reg * sizeof(uint32_t)].SetWriteCallback(
          [reg, this](uint64_t value) { values_[reg] = value; });
    }
  }

  fdf::MmioBuffer mmio() { return regs_.GetMmioBuffer(); }
  std::map<size_t, uint64_t>& values() { return values_; }
  const std::map<size_t, uint64_t>& values() const { return values_; }

 private:
  size_t count_;  // of registers.
  fake_mmio::FakeMmioRegRegion regs_;
  std::map<size_t, uint64_t> values_;
};

class TestEnvironment : public fdf_testing::Environment {
 public:
  TestEnvironment() {
    auto config = fdf_fake::FakePDev::Config{};
    config.mmios[kHiuMmioIndex] = hiu_regs_.mmio();
    config.mmios[kDosMmioIndex] = dos_regs_.mmio();
    config.use_fake_bti = true;
    config.use_fake_irq = true;

    pdev_.SetConfig(std::move(config));

    hiu_regs_.mmio().Write32(HHI_PLL_LOCK, HHI_GP0_PLL_CNTL0);
  }

  zx::result<> Serve(fdf::OutgoingDirectory& incoming) override {
    async_dispatcher_t* dispatcher = fdf::Dispatcher::GetCurrent()->async_dispatcher();

    EXPECT_TRUE(incoming.AddService<fpdev::Service>(pdev_.GetInstanceHandler(dispatcher)).is_ok());

#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
    fclockimpl::ClockIdsMetadata metadata{{.clock_nodes {} }};
    EXPECT_EQ(ZX_OK, pdev_.AddFidlMetadata(fclockimpl::ClockIdsMetadata::kSerializableName,
                                           std::move(metadata)));
#else
    fclockimpl::InitMetadata metadata{{.steps{}}};
    EXPECT_EQ(ZX_OK, incoming.AddFidlMetadata(
        fclockimpl::InitMetadata::kSerializableName, std::move(metadata));
#endif

    return zx::ok();
  }

  void ClearHiu() { ClearMmioBuffer(hiu_regs_); }
  void ClearDos() { ClearMmioBuffer(dos_regs_); }
  bool IsHiuDirty() { return IsMmioBufferDirty(hiu_regs_); }
  bool IsDosDirty() { return IsMmioBufferDirty(dos_regs_); }

 private:
  static void ClearMmioBuffer(FakeMmio& mmio) { mmio.values().clear(); }

  static bool IsMmioBufferDirty(const FakeMmio& mmio) {
    if (mmio.values().empty()) {
      return false;
    }

    for (const auto& [k, v] : mmio.values()) {
      if (v != 0) {
        return true;
      }
    }
    return false;
  }

  static constexpr size_t kRegSize = sizeof(uint32_t);

  fdf_fake::FakePDev pdev_;
  FakeMmio hiu_regs_{A311D_HIU_LENGTH / kRegSize};
  FakeMmio dos_regs_{A311D_DOS_LENGTH / kRegSize};
};

class FixtureConfig final {
 public:
  using DriverType = Vim3Clock;
  using EnvironmentType = TestEnvironment;
};

class DriverTest : public ::testing::Test {
 public:
  void SetUp() override {
    zx::result<> result = driver_test().StartDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
    zx::result device_result = driver_test().Connect<fuchsia_hardware_clockimpl::Service::Device>();
    ASSERT_EQ(device_result.status_value(), ZX_OK);
    client_.Bind(std::move(device_result.value()));
  }

  void TearDown() override {
    zx::result<> result = driver_test().StopDriver();
    ASSERT_EQ(ZX_OK, result.status_value());
  }
  fdf_testing::BackgroundDriverTest<FixtureConfig>& driver_test() { return driver_test_; }
  fdf_testing::BackgroundDriverTest<FixtureConfig> driver_test_;

  fdf::WireSyncClient<fuchsia_hardware_clockimpl::ClockImpl> client_;
};

TEST_F(DriverTest, EnableDisableMesonGate) {
  fdf::Arena arena('TEST');

  // Pick an arbitrary Meson Gate to Enable and Disable. Make sure it works.
  auto enable_result = client_.buffer(arena)->Enable(g12b_clk::G12B_CLK_AUDIO);
  ASSERT_TRUE(enable_result.ok());
  ASSERT_TRUE(enable_result->is_ok());

  auto disable_result = client_.buffer(arena)->Disable(g12b_clk::G12B_CLK_AUDIO);
  ASSERT_TRUE(disable_result.ok());
  ASSERT_TRUE(disable_result->is_ok());
}

TEST_F(DriverTest, EnableDisableMesonPll) {
  fdf::Arena arena('TEST');

  // Pick an arbitrary Meson Gate to Disable. Make sure it works.
  auto disable_result = client_.buffer(arena)->Disable(g12b_clk::CLK_PCIE_PLL);
  ASSERT_TRUE(disable_result.ok());
  ASSERT_TRUE(disable_result->is_ok());
}

TEST_F(DriverTest, EnableDisableInvalid) {
  fdf::Arena arena('TEST');

  {
    // CPU Clocks don't support Enable
    auto result = client_.buffer(arena)->Enable(g12b_clk::CLK_SYS_CPU_BIG_CLK);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
  }

  {
    // CPU Clocks don't support Disable
    auto result = client_.buffer(arena)->Disable(g12b_clk::CLK_SYS_CPU_BIG_CLK);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
  }

  {
    // Invent a new Meson Clock and try to enable it
    constexpr uint32_t kFakeMesonClockID =
        AmlClkId(0xBEEF, aml_clk_common::aml_clk_type::kMesonGate);
    auto result = client_.buffer(arena)->Enable(kFakeMesonClockID);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
  }

  {
    // Invent a new Meson Clock and try to disable it
    constexpr uint32_t kFakeMesonClockID =
        AmlClkId(0xBEEF, aml_clk_common::aml_clk_type::kMesonGate);
    auto result = client_.buffer(arena)->Disable(kFakeMesonClockID);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
  }
}

TEST_F(DriverTest, ClkMuxUnsupported) {
  // These are placeholder tests just to exercise the mux interfaces even though they are
  // unsupporeted.
  fdf::Arena arena('TEST');
  {
    auto result = client_.buffer(arena)->SetInput(0, 0);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
    ASSERT_EQ(result->error_value(), ZX_ERR_NOT_SUPPORTED);
  }

  {
    auto result = client_.buffer(arena)->GetNumInputs(0);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
    ASSERT_EQ(result->error_value(), ZX_ERR_NOT_SUPPORTED);
  }

  {
    auto result = client_.buffer(arena)->GetInput(0);
    ASSERT_TRUE(result.ok());
    ASSERT_FALSE(result->is_ok());
    ASSERT_EQ(result->error_value(), ZX_ERR_NOT_SUPPORTED);
  }
}

TEST_F(DriverTest, ClkEnablePll) {
  fdf::Arena arena('TEST');

  {
    auto result = client_.buffer(arena)->Enable(g12b_clk::CLK_GP0_PLL);
    ASSERT_TRUE(result.ok());
    ASSERT_TRUE(result->is_ok());
  }
}

TEST_F(DriverTest, ClkTestHiuRegRegion) {
  /// Make sure that HIU clocks are actually touching the HIU registers.
  fdf::Arena arena('TEST');

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    env.ClearDos();
    env.ClearHiu();
  });

  auto enable_result = client_.buffer(arena)->Enable(g12b_clk::G12B_CLK_AUDIO);
  ASSERT_TRUE(enable_result.ok());

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    ASSERT_TRUE(env.IsHiuDirty());
    ASSERT_FALSE(env.IsDosDirty());
  });
}

TEST_F(DriverTest, ClkTestDosRegRegion) {
  /// Make sure that DOS clocks are actually touching the DOS registers.
  fdf::Arena arena('TEST');

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    env.ClearDos();
    env.ClearHiu();
  });

  auto enable_result = client_.buffer(arena)->Enable(g12b_clk::G12B_CLK_DOS_GCLK_VDEC);
  ASSERT_TRUE(enable_result.ok());

  driver_test().RunInEnvironmentTypeContext([](TestEnvironment& env) {
    ASSERT_FALSE(env.IsHiuDirty());
    ASSERT_TRUE(env.IsDosDirty());
  });
}

}  // namespace vim3_clock
