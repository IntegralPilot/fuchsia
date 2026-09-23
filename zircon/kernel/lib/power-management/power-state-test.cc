
// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/power-management/power-state.h"

#include <lib/power-management/energy-model.h>
// TODO(https://fxbug.dev/415033686): Stop using `syscalls-next.h` on host.
#define FUCHSIA_UNSUPPORTED_ALLOW_SYSCALLS_NEXT_ON_HOST
#include <zircon/syscalls-next.h>
#undef FUCHSIA_UNSUPPORTED_ALLOW_SYSCALLS_NEXT_ON_HOST

#include <cstdint>

#include <gtest/gtest.h>

#include "test-helper.h"

namespace {

using power_management::ControlInterface;
using power_management::PowerDomain;

using power_management::PowerDomainSet;
using power_management::PowerLevelUpdateRequest;
using power_management::PowerState;
using power_management::Utilization;

constexpr uint32_t kModelId = 123;
constexpr uint32_t kTotalPowerLevels = 8;

constexpr Utilization kZeroUtilization{0};
constexpr Utilization kOneHalfUtilization = ffl::FromRatio(1, 2);
constexpr Utilization kOneQuarterUtilization = ffl::FromRatio(1, 4);

TEST(PowerStateTest, Default) {
  PowerState state;
  EXPECT_EQ(state.domain(), nullptr);
  EXPECT_FALSE(state.is_serving());
  EXPECT_FALSE(state.active_power_level());
  EXPECT_FALSE(state.desired_active_power_level());
  EXPECT_EQ(0u, state.active_power_coefficient_nw());
  EXPECT_FALSE(state.max_idle_power_level());
  EXPECT_EQ(0u, state.max_idle_power_coefficient_nw());
  EXPECT_FALSE(state.max_idle_power_level_interface());
  EXPECT_EQ(kZeroUtilization, state.normalized_utilization());
}

TEST(PowerStateTest, UpdateDomainSetsModel) {
  auto domain = MakePowerDomainHelper(kModelId, 0, 1, 2, 3, 4, 5);

  PowerState state;
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);

  EXPECT_EQ(state.domain(), domain);
  EXPECT_FALSE(state.active_power_level());
  EXPECT_FALSE(state.desired_active_power_level());
  EXPECT_EQ(0u, state.active_power_coefficient_nw());
  EXPECT_EQ(kMaxIdlePowerLevel, state.max_idle_power_level());
  EXPECT_EQ(kMaxIdlePowerLevel + 1u, state.max_idle_power_coefficient_nw());
  EXPECT_EQ(ControlInterface::kArmWfi, state.max_idle_power_level_interface());
}

TEST(PowerStateTest, UpdateDomainKeepsPowerLevelWhenSameModelId) {
  auto domain = MakePowerDomainHelper(kModelId, 0, 1, 2, 3, 4, 5);
  auto domain2 = MakePowerDomainHelper(kModelId, 0, 1, 2, 3, 4);

  PowerState state;
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  ASSERT_EQ(state.domain(), domain);
  ASSERT_EQ(ZX_OK, state.UpdateActivePowerLevel(kMinActivePowerLevel + 1).status_value());

  static_cast<FakePowerLevelController*>(domain2->controller().get())->current_power_level =
      kMinActivePowerLevel;

  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain2), 0);
  EXPECT_EQ(state.domain(), domain2);
  EXPECT_EQ(state.active_power_level(), kMinActivePowerLevel);
  EXPECT_EQ(state.desired_active_power_level(), kMinActivePowerLevel);
}

TEST(PowerStateTest, UpdateDomainClearsPowerLevelWhenDifferentModelId) {
  auto domain = MakePowerDomainHelper(kModelId, 0, 1, 2, 3, 4, 5);
  auto domain2 = MakePowerDomainHelper(kModelId + 1, 0, 1, 2, 3, 4, 5);

  PowerState state;
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  ASSERT_EQ(state.domain(), domain);
  ASSERT_EQ(ZX_OK, state.UpdateActivePowerLevel(kMinActivePowerLevel + 1).status_value());

  static_cast<FakePowerLevelController*>(domain2->controller().get())->current_power_level =
      kMinActivePowerLevel;

  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain2), 0);
  EXPECT_EQ(state.domain(), domain2);
  EXPECT_EQ(state.active_power_level(), kMinActivePowerLevel);
  EXPECT_EQ(state.desired_active_power_level(), kMinActivePowerLevel);
}

TEST(PowerStateTest, TransitionWhenModelIsUnknown) {
  PowerState state;
  EXPECT_FALSE(state.RequestTransition(1, 8));
}

TEST(PowerStateTest, TransitionWhenPowerLevelIsUnknown) {
  auto energy_model = MakeFakeEnergyModel(kTotalPowerLevels);
  auto domain = MakePowerDomainHelper(kModelId, energy_model, 0, 1, 2, 3, 4, 5);

  PowerState state;
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  ASSERT_EQ(state.domain(), domain);

  EXPECT_FALSE(state.RequestTransition(1, kMinActivePowerLevel + 1));
}

TEST(PowerStateTest, TransitionWhenPowerLevelIsDesiredPowerLevel) {
  auto energy_model = MakeFakeEnergyModel(kTotalPowerLevels);
  auto domain = MakePowerDomainHelper(kModelId, energy_model, 0, 1, 2, 3, 4, 5);

  PowerState state;
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  ASSERT_EQ(state.domain(), domain);
  ASSERT_EQ(ZX_OK, state.UpdateActivePowerLevel(kMinActivePowerLevel).status_value());

  EXPECT_FALSE(state.RequestTransition(1, kMinActivePowerLevel));
}

TEST(PowerStateTest, TransitionWhenPowerLevelIsAlreadyInFlight) {
  auto energy_model = MakeFakeEnergyModel(kTotalPowerLevels);
  auto domain = MakePowerDomainHelper(kModelId, energy_model, 0, 1, 2, 3, 4, 5);

  PowerState state;
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  ASSERT_EQ(state.domain(), domain);
  ASSERT_EQ(ZX_OK, state.UpdateActivePowerLevel(kMinActivePowerLevel).status_value());

  // First request for a higher level succeeds and enters in-flight state.
  EXPECT_TRUE(state.RequestTransition(1, kMinActivePowerLevel + 1));
  EXPECT_EQ(state.desired_active_power_level(), kMinActivePowerLevel + 1);

  // Subsequent request for the same level is suppressed.
  EXPECT_FALSE(state.RequestTransition(1, kMinActivePowerLevel + 1));

  // Request for an even higher level updates the desired level and succeeds.
  EXPECT_TRUE(state.RequestTransition(1, kMinActivePowerLevel + 2));
  EXPECT_EQ(state.desired_active_power_level(), kMinActivePowerLevel + 2);

  // Acknowledging the transition updates active and desired levels.
  ASSERT_EQ(ZX_OK, state.UpdateActivePowerLevel(kMinActivePowerLevel + 2).status_value());
  EXPECT_EQ(state.active_power_level(), kMinActivePowerLevel + 2);
  EXPECT_EQ(state.desired_active_power_level(), kMinActivePowerLevel + 2);
  EXPECT_FALSE(state.RequestTransition(1, kMinActivePowerLevel + 2));
}

TEST(PowerStateTest, TransitionWhenPowerLevelIsTooHigh) {
  auto energy_model = MakeFakeEnergyModel(kTotalPowerLevels);
  auto domain = MakePowerDomainHelper(kModelId, energy_model, 0, 1, 2, 3, 4, 5);

  PowerState state;
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  ASSERT_EQ(state.domain(), domain);
  ASSERT_EQ(ZX_OK, state.UpdateActivePowerLevel(kMinActivePowerLevel).status_value());

  EXPECT_DEATH(state.RequestTransition(1, kTotalPowerLevels), "ASSERT FAILED");
}

TEST(PowerStateTest, TransitionWhenPowerLevelIsTooLow) {
  auto energy_model = MakeFakeEnergyModel(kTotalPowerLevels);
  auto domain = MakePowerDomainHelper(kModelId, energy_model, 0, 1, 2, 3, 4, 5);

  PowerState state;
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  ASSERT_EQ(state.domain(), domain);
  ASSERT_EQ(ZX_OK, state.UpdateActivePowerLevel(kMinActivePowerLevel).status_value());

  EXPECT_DEATH(state.RequestTransition(1, kMaxIdlePowerLevel), "ASSERT FAILED");
}

TEST(PowerStateTest, UpdateUtilizationReflectsOnDomain) {
  auto energy_model = MakeFakeEnergyModel(kTotalPowerLevels);
  auto domain = MakePowerDomainHelper(kModelId, energy_model, 0, 1, 2, 3, 4, 5);
  auto domain2 = MakePowerDomainHelper(kModelId + 1, energy_model, 0, 1, 2, 3, 4, 5);

  PowerState state;

  EXPECT_EQ(kZeroUtilization, state.normalized_utilization());
  EXPECT_EQ(kZeroUtilization, domain->total_normalized_utilization());
  EXPECT_EQ(kZeroUtilization, domain2->total_normalized_utilization());

  // Update utilization before associating with a domain.
  state.UpdateUtilization(kOneHalfUtilization);

  EXPECT_EQ(kOneHalfUtilization, state.normalized_utilization());
  EXPECT_EQ(kZeroUtilization, domain->total_normalized_utilization());
  EXPECT_EQ(kZeroUtilization, domain2->total_normalized_utilization());

  // Associating with a domain should update the domain total.
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  ASSERT_EQ(state.domain(), domain);

  EXPECT_EQ(kOneHalfUtilization, domain->total_normalized_utilization());
  EXPECT_EQ(kZeroUtilization, domain2->total_normalized_utilization());

  // Update utilization while associated with a domain.
  state.UpdateUtilization(-kOneQuarterUtilization);

  EXPECT_EQ(kOneQuarterUtilization, state.normalized_utilization());
  EXPECT_EQ(kOneQuarterUtilization, domain->total_normalized_utilization());
  EXPECT_EQ(kZeroUtilization, domain2->total_normalized_utilization());

  // Changing to a different domain should move the utilization from the
  // previous domain to the new domain.
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain2), 0);

  EXPECT_EQ(kOneQuarterUtilization, state.normalized_utilization());
  EXPECT_EQ(kZeroUtilization, domain->total_normalized_utilization());
  EXPECT_EQ(kOneQuarterUtilization, domain2->total_normalized_utilization());
}

TEST(PowerStateTest, PrecedingTargetPowerLevel) {
  auto energy_model = MakeFakeEnergyModel(kTotalPowerLevels);
  auto domain = MakePowerDomainHelper(kModelId, energy_model, 0, 1, 2, 3, 4, 5);

  PowerState state;
  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  ASSERT_EQ(state.domain(), domain);
  ASSERT_EQ(ZX_OK, state.UpdateActivePowerLevel(kMinActivePowerLevel).status_value());

  // At the minimum active power level, there is no preceding active level.
  EXPECT_FALSE(state.has_preceding_target_power_level());
  EXPECT_EQ(power_management::ProcessingRate{0}, state.preceding_target_processing_rate());

  // At higher active power levels, there is a preceding active level.
  ASSERT_EQ(ZX_OK, state.UpdateActivePowerLevel(kMinActivePowerLevel + 1).status_value());
  EXPECT_TRUE(state.has_preceding_target_power_level());
  EXPECT_GT(state.preceding_target_processing_rate(), power_management::ProcessingRate{0});
}

TEST(PowerStateTest, SchedulerControlEnabled) {
  auto domain = MakePowerDomainHelper(kModelId, 0, 1, 2, 3, 4, 5);

  PowerState state;
  EXPECT_FALSE(state.scheduler_control_enabled());

  state.UpdatePowerDomainSet(PowerDomainSet::CreateForTest(domain), 0);
  EXPECT_FALSE(state.scheduler_control_enabled());

  domain->SetSchedulerControlEnabled(true);
  EXPECT_TRUE(state.scheduler_control_enabled());

  domain->SetSchedulerControlEnabled(false);
  EXPECT_FALSE(state.scheduler_control_enabled());
}

}  // namespace
