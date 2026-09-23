// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/testing/cpp/inspect.h>

#include <gtest/gtest.h>

#include "driver_logger_harness.h"
#include "src/graphics/drivers/msd-arm-mali/src/fuchsia_power_manager.h"
#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace {

class FakePowerOwner : public FuchsiaPowerManager::Owner {
 public:
  void PostPowerStateChange(bool enabled,
                            FuchsiaPowerManager::Owner::PowerStateCallback completer) override {
    enabled_calls_.push_back(enabled);
    completer(enabled);
  }
  PowerManager* GetPowerManager() override { return nullptr; }

  const std::vector<bool>& enabled_calls() const { return enabled_calls_; }
  void clear_enabled_calls() { enabled_calls_.clear(); }

 private:
  std::vector<bool> enabled_calls_;
};

class FuchsiaPowerManagerTest : public gtest::RealLoopFixture {
 public:
  FuchsiaPowerManagerTest() : fpm_(&owner_) {}

  void SetUp() override {
    logger_harness_ = DriverLoggerHarness::Create();
    ASSERT_TRUE(fpm_.Initialize(inspector_.GetRoot()));
  }

  const inspect::Inspector& inspector() { return inspector_; }
  FakePowerOwner& owner() { return owner_; }
  FuchsiaPowerManager& fuchsia_power_manager() { return fpm_; }

 private:
  std::unique_ptr<DriverLoggerHarness> logger_harness_;
  inspect::Inspector inspector_;
  FakePowerOwner owner_;
  FuchsiaPowerManager fpm_;
};

TEST_F(FuchsiaPowerManagerTest, Initialize) {
  fpromise::result<inspect::Hierarchy> hierarchy =
      RunPromise(inspect::ReadFromInspector(inspector()));
  ASSERT_TRUE(hierarchy.is_ok());

  auto* is_system_suspending = hierarchy.value().node().get_property<inspect::BoolPropertyValue>(
      FuchsiaPowerManager::kIsSystemSuspendingInspectNode);
  ASSERT_TRUE(is_system_suspending);
  ASSERT_FALSE(is_system_suspending->value());

  auto* powered_on = hierarchy.value().node().get_property<inspect::BoolPropertyValue>(
      FuchsiaPowerManager::kPoweredOnInspectNode);
  ASSERT_TRUE(powered_on);
  ASSERT_FALSE(powered_on->value());

  auto* power_up_requested = hierarchy.value().node().get_property<inspect::BoolPropertyValue>(
      FuchsiaPowerManager::kPowerOnAfterSuspendInspectNode);
  ASSERT_TRUE(power_up_requested);
  ASSERT_FALSE(power_up_requested->value());
}

TEST_F(FuchsiaPowerManagerTest, EnablePower) {
  fuchsia_power_manager().EnablePower();

  ASSERT_EQ(1u, owner().enabled_calls().size());
  ASSERT_TRUE(owner().enabled_calls()[0]);
}

TEST_F(FuchsiaPowerManagerTest, DisablePower) {
  fuchsia_power_manager().EnablePower();
  owner().clear_enabled_calls();

  fuchsia_power_manager().DisablePower();

  ASSERT_EQ(1u, owner().enabled_calls().size());
  ASSERT_FALSE(owner().enabled_calls()[0]);
}

TEST_F(FuchsiaPowerManagerTest, SuspendResume) {
  // Suspend when powered off.
  {
    bool suspend_completed = false;
    fuchsia_power_manager().Suspend([&]() { suspend_completed = true; });
    EXPECT_TRUE(suspend_completed);
    // Suspend calls PowerDown unconditionally, which triggers PostPowerStateChange(false).
    ASSERT_EQ(1u, owner().enabled_calls().size());
    EXPECT_FALSE(owner().enabled_calls()[0]);
  }
  owner().clear_enabled_calls();

  // Resume when it was powered off before suspend.
  {
    bool resume_completed = false;
    fuchsia_power_manager().Resume([&]() { resume_completed = true; });
    EXPECT_TRUE(resume_completed);
    // Should not trigger power change.
    EXPECT_EQ(0u, owner().enabled_calls().size());
  }
  owner().clear_enabled_calls();

  // Power on.
  fuchsia_power_manager().EnablePower();
  ASSERT_EQ(1u, owner().enabled_calls().size());
  EXPECT_TRUE(owner().enabled_calls()[0]);
  owner().clear_enabled_calls();

  // Suspend when powered on.
  {
    bool suspend_completed = false;
    fuchsia_power_manager().Suspend([&]() { suspend_completed = true; });
    EXPECT_TRUE(suspend_completed);
    // Should power down.
    ASSERT_EQ(1u, owner().enabled_calls().size());
    EXPECT_FALSE(owner().enabled_calls()[0]);
  }
  owner().clear_enabled_calls();

  // Resume when it was powered on before suspend.
  {
    bool resume_completed = false;
    fuchsia_power_manager().Resume([&]() { resume_completed = true; });
    EXPECT_TRUE(resume_completed);
    // Should power back on.
    ASSERT_EQ(1u, owner().enabled_calls().size());
    EXPECT_TRUE(owner().enabled_calls()[0]);
  }
}

TEST_F(FuchsiaPowerManagerTest, PowerOnDelayedUntilResume) {
  // Power on.
  fuchsia_power_manager().EnablePower();
  ASSERT_EQ(1u, owner().enabled_calls().size());
  owner().clear_enabled_calls();

  // Suspend.
  fuchsia_power_manager().Suspend([]() {});
  ASSERT_EQ(1u, owner().enabled_calls().size());
  EXPECT_FALSE(owner().enabled_calls()[0]);
  owner().clear_enabled_calls();

  // Enable power during suspend. This should NOT power on immediately.
  fuchsia_power_manager().EnablePower();
  EXPECT_EQ(0u, owner().enabled_calls().size());

  // Resume. This should power on now.
  fuchsia_power_manager().Resume([]() {});
  ASSERT_EQ(1u, owner().enabled_calls().size());
  EXPECT_TRUE(owner().enabled_calls()[0]);
}

}  // namespace
