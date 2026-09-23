// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/magma/platform/platform_buffer.h>
#include <lib/magma_service/mock/mock_mmio.h>
#include <zircon/compiler.h>

#include <iterator>

#include <gtest/gtest.h>

#include "src/graphics/drivers/msd-arm-mali/src/power_manager.h"
#include "src/graphics/drivers/msd-arm-mali/src/registers.h"

namespace {
class FakePowerOwner : public PowerManager::Owner {
 public:
  explicit FakePowerOwner(mali::RegisterIo* register_io) : register_io_(register_io) {}

  mali::RegisterIo* register_io() override { return register_io_; }
  void ReportPowerChangeComplete(bool powered_on, bool success) override {
    if (!success) {
      power_change_failure_count_++;
    }

    power_change_complete_count_++;
  }

  uint32_t power_change_failure_count() const { return power_change_failure_count_; }
  uint32_t power_change_complete_count() const { return power_change_complete_count_; }

 private:
  mali::RegisterIo* register_io_;
  uint32_t power_change_failure_count_{0};
  uint32_t power_change_complete_count_{0};
};
}  // namespace

class TestPowerManager {
 public:
  void MockEnable() {
    auto reg_io = std::make_unique<mali::RegisterIo>(MockMmio::Create(1024 * 1024));
    FakePowerOwner power_owner{reg_io.get()};
    auto power_manager = std::make_unique<PowerManager>(&power_owner, 1);

    constexpr uint32_t kShaderOnOffset =
        static_cast<uint32_t>(registers::CoreReadyState::CoreType::kShader) +
        static_cast<uint32_t>(registers::CoreReadyState::ActionType::kActionPowerOn);
    constexpr uint32_t kShaderOnHighOffset = kShaderOnOffset + 4;
    constexpr uint32_t kDummyHighValue = 1500;
    reg_io->Write32(kDummyHighValue, kShaderOnHighOffset);
    power_manager->EnableCores(0xf);
    // Higher word shouldn't be written to because none of them are being
    // enabled.
    EXPECT_EQ(kDummyHighValue, reg_io->Read32(kShaderOnHighOffset));

    registers::CoreReadyState::CoreType actions[] = {registers::CoreReadyState::CoreType::kShader,
                                                     registers::CoreReadyState::CoreType::kL2,
                                                     registers::CoreReadyState::CoreType::kTiler};
    for (size_t i = 0; i < std::size(actions); i++) {
      uint32_t offset =
          static_cast<uint32_t>(actions[i]) +
          static_cast<uint32_t>(registers::CoreReadyState::ActionType::kActionPowerOn);

      if (actions[i] == registers::CoreReadyState::CoreType::kShader)
        EXPECT_EQ(0xfu, reg_io->Read32(offset));
      else
        EXPECT_EQ(1u, reg_io->Read32(offset));
    }
  }

  void MockDisable() {
    auto reg_io = std::make_unique<mali::RegisterIo>(MockMmio::Create(1024 * 1024));
    FakePowerOwner power_owner{reg_io.get()};
    auto power_manager = std::make_unique<PowerManager>(&power_owner, 1);

    constexpr uint64_t kCoresEnabled = 2;
    constexpr uint32_t kShaderReadyOffset =
        static_cast<uint32_t>(registers::CoreReadyState::CoreType::kShader) +
        static_cast<uint32_t>(registers::CoreReadyState::StatusType::kReady);
    reg_io->Write32(kCoresEnabled, kShaderReadyOffset);

    power_manager->DisableShaders();

    registers::CoreReadyState::CoreType actions[] = {registers::CoreReadyState::CoreType::kShader,
                                                     registers::CoreReadyState::CoreType::kL2,
                                                     registers::CoreReadyState::CoreType::kTiler};
    for (size_t i = 0; i < std::size(actions); i++) {
      uint32_t offset =
          static_cast<uint32_t>(actions[i]) +
          static_cast<uint32_t>(registers::CoreReadyState::ActionType::kActionPowerOff);

      if (actions[i] == registers::CoreReadyState::CoreType::kShader)
        EXPECT_EQ(kCoresEnabled, reg_io->Read32(offset));
      else
        EXPECT_EQ(0u, reg_io->Read32(offset));
    }
    power_manager->DisableL2();
    for (size_t i = 0; i < std::size(actions); i++) {
      uint32_t offset =
          static_cast<uint32_t>(actions[i]) +
          static_cast<uint32_t>(registers::CoreReadyState::ActionType::kActionPowerOff);

      if (actions[i] == registers::CoreReadyState::CoreType::kShader)
        EXPECT_EQ(kCoresEnabled, reg_io->Read32(offset));
      else
        EXPECT_EQ(1u, reg_io->Read32(offset));
    }
  }

  void TimeCoalesce() {
    auto reg_io = std::make_unique<mali::RegisterIo>(MockMmio::Create(1024 * 1024));
    FakePowerOwner power_owner{reg_io.get()};
    PowerManager power_manager(&power_owner, 1);

    for (int i = 0; i < 100; i++) {
      power_manager.UpdateGpuActive(true, false);
      usleep(5000);
      power_manager.UpdateGpuActive(false, false);
      usleep(5000);
    }

    auto time_periods = power_manager.time_periods();
    // There can be 4 time periods containing the last 100ms - for example 45
    // ms (oldest), 45 ms, 45 ms, 5 ms (most recent). More than that and either
    // one ends more than 100ms ago or one could be combined with the one
    // previous to make a chunk that's < 50 ms.
    EXPECT_GE(4u, time_periods.size());
  }
};

TEST(PowerManager, MockEnable) {
  TestPowerManager test;
  test.MockEnable();
}

TEST(PowerManager, MockDisable) {
  TestPowerManager test;
  test.MockDisable();
}

TEST(PowerManager, TimeAccumulation) {
  auto reg_io = std::make_unique<mali::RegisterIo>(MockMmio::Create(1024 * 1024));
  FakePowerOwner power_owner{reg_io.get()};
  PowerManager power_manager(&power_owner, 1);
  power_manager.UpdateGpuActive(true, false);
  usleep(150 * 1000);

  std::chrono::steady_clock::duration total_time;
  std::chrono::steady_clock::duration active_time;
  power_manager.GetGpuActiveInfo(&total_time, &active_time);
  EXPECT_LE(100u, std::chrono::duration_cast<std::chrono::milliseconds>(total_time).count());
  EXPECT_EQ(total_time, active_time);

  usleep(150 * 1000);

  uint64_t before_time_ns = std::chrono::duration_cast<std::chrono::nanoseconds>(
                                std::chrono::steady_clock::now().time_since_epoch())
                                .count();
  uint32_t time_buffer;
  EXPECT_TRUE(power_manager.GetTotalTime(&time_buffer));
  magma_total_time_query_result result;
  uint64_t after_time_ns = std::chrono::duration_cast<std::chrono::nanoseconds>(
                               std::chrono::steady_clock::now().time_since_epoch())
                               .count();

  auto buffer = magma::PlatformBuffer::Import(time_buffer);
  EXPECT_TRUE(buffer);
  EXPECT_TRUE(buffer->Read(&result, 0, sizeof(result)));

  EXPECT_LE(before_time_ns, result.monotonic_time_ns);
  EXPECT_LE(result.monotonic_time_ns, after_time_ns);

  // GetGpuActiveInfo should throw away old information, but the GetTotalTime count should be able
  // to go higher. We slept a total of 300ms above, so the time should be well over 250ms.
  constexpr uint32_t k250MsInNs = 250'000'000;
  EXPECT_LE(k250MsInNs, result.gpu_time_ns);
}

TEST(PowerManager, TimeCoalesce) {
  TestPowerManager test;
  test.TimeCoalesce();
}

TEST(PowerManager, PowerDownOnIdle) {
  auto mock_mmio = MockMmio::Create(1024 * 1024);

  constexpr uint32_t kShaderReadyOffset =
      static_cast<uint32_t>(registers::CoreReadyState::CoreType::kShader) +
      static_cast<uint32_t>(registers::CoreReadyState::StatusType::kReady);
  constexpr uint32_t kShaderPowerOffOffset =
      static_cast<uint32_t>(registers::CoreReadyState::CoreType::kShader) +
      static_cast<uint32_t>(registers::CoreReadyState::ActionType::kActionPowerOff);
  constexpr uint32_t kShaderPowerOnOffset =
      static_cast<uint32_t>(registers::CoreReadyState::CoreType::kShader) +
      static_cast<uint32_t>(registers::CoreReadyState::ActionType::kActionPowerOn);

  class Hook : public magma::RegisterIo::Hook {
   public:
    explicit Hook(MockMmio* mock_mmio) : mock_mmio_(mock_mmio) {}
    ~Hook() override {}
    void Write32(uint32_t val, uint32_t offset) override {
      if (offset == kShaderPowerOffOffset) {
        mock_mmio_->Write32(0, kShaderReadyOffset);
      }
      if (offset == kShaderPowerOnOffset) {
        mock_mmio_->Write32(val, kShaderReadyOffset);
      }
    }
    void Read32(uint32_t val, uint32_t offset) override {}
    void Read64(uint64_t val, uint32_t offset) override {}

   private:
    MockMmio* mock_mmio_;
  };

  auto hook = std::make_unique<Hook>(mock_mmio.get());
  auto reg_io = std::make_unique<mali::RegisterIo>(std::move(mock_mmio));
  reg_io->InstallHook(std::move(hook));

  FakePowerOwner power_owner{reg_io.get()};
  auto power_manager = std::make_unique<PowerManager>(&power_owner, 2);

  constexpr uint64_t kCoresEnabled = 2;
  reg_io->Write32(kCoresEnabled, kShaderReadyOffset);

  power_manager->UpdateGpuActive(true, false);
  power_manager->PowerDownOnIdle();
  EXPECT_EQ(reg_io->Read32(kShaderPowerOffOffset), 0u);
  EXPECT_EQ(0u, power_owner.power_change_complete_count());

  power_manager->UpdateGpuActive(false, true);
  EXPECT_EQ(reg_io->Read32(kShaderPowerOffOffset), kCoresEnabled);
  EXPECT_EQ(power_manager->GetGpuPowerdownTimeout(), PowerManager::Clock::time_point::max());

  EXPECT_EQ(0u, power_owner.power_change_failure_count());
  EXPECT_EQ(1u, power_owner.power_change_complete_count());

  power_manager->PowerUpAfterIdle();

  EXPECT_EQ(0u, power_owner.power_change_failure_count());
  EXPECT_EQ(2u, power_owner.power_change_complete_count());

  EXPECT_EQ(reg_io->Read32(kShaderPowerOnOffset), kCoresEnabled);
}
