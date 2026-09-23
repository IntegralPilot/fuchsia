// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/power-management/energy-model.h"

#include <lib/stdcompat/utility.h>
#include <zircon/errors.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <algorithm>
#include <array>
#include <cstddef>

#include <fbl/ref_ptr.h>
#include <gtest/gtest.h>

#include "test-helper.h"

namespace {

using power_management::ControlInterface;
using power_management::EnergyModel;
using power_management::kPowerLevelOptionsDomainIndependent;
using power_management::PowerDomain;

using power_management::PowerDomainSet;
using power_management::PowerLevel;
using power_management::PowerLevelTransition;
using power_management::ProcessingRate;
using power_management::ProcessorPowerLevel;
using power_management::ProcessorPowerLevelTransition;
using power_management::Utilization;

// Utility to test whether the given ranged object contains the given element.
template <typename Ranged, typename Element>
bool InRange(const Ranged& ranged, const Element& element) {
  return std::ranges::find(ranged, element) != std::cend(ranged);
}

TEST(PowerLevelTest, ToFromRate) {
  const ProcessingRate min_rate{0};
  const ProcessingRate max_rate{1};
  const uint64_t min_raw_rate{0};
  const uint64_t max_raw_rate{1024};

  EXPECT_EQ(min_rate, PowerLevel::ToProcessingRate(min_raw_rate, max_raw_rate));
  EXPECT_EQ(max_rate, PowerLevel::ToProcessingRate(max_raw_rate, max_raw_rate));
  EXPECT_EQ(min_raw_rate, PowerLevel::FromProcessingRate(min_rate, max_raw_rate));
  EXPECT_EQ(max_raw_rate, PowerLevel::FromProcessingRate(max_rate, max_raw_rate));
}

TEST(PowerLevelTest, Ctor) {
  constexpr ProcessorPowerLevel kLevel = {
      .options = 0,
      .processing_rate = 456,
      .power_coefficient_nw = 789,
      .control_interface = ControlInterface::kArmPsci,
      .control_argument = 12345,
      .diagnostic_name = "foobar one two three",
  };

  PowerLevel level(0, kLevel, PowerLevel::kUserProcessingRateScale);

  EXPECT_EQ(level.level(), 0);
  EXPECT_EQ(
      level.processing_rate(),
      PowerLevel::ToProcessingRate(kLevel.processing_rate, PowerLevel::kUserProcessingRateScale));
  EXPECT_EQ(level.power_coefficient_nw(), kLevel.power_coefficient_nw);
  EXPECT_EQ(level.control(), kLevel.control_interface);
  EXPECT_EQ(level.control_argument(), kLevel.control_argument);
  EXPECT_EQ(level.type(), PowerLevel::Type::kActive);
  EXPECT_EQ(level.name(), std::string_view(kLevel.diagnostic_name));
  EXPECT_TRUE(level.TargetsPowerDomain());
  EXPECT_FALSE(level.TargetsCpus());
}

TEST(PowerLevelTest, Ctor2) {
  constexpr ProcessorPowerLevel kLevel = {
      .options = kPowerLevelOptionsDomainIndependent,
      .processing_rate = 123,
      .power_coefficient_nw = 789,
      .control_interface = ControlInterface::kArmPsci,
      .control_argument = 12345,
      .diagnostic_name = "foobar one two three",
  };

  PowerLevel level(123, kLevel, PowerLevel::kUserProcessingRateScale);

  EXPECT_EQ(level.level(), 123);
  EXPECT_EQ(
      level.processing_rate(),
      PowerLevel::ToProcessingRate(kLevel.processing_rate, PowerLevel::kUserProcessingRateScale));
  EXPECT_EQ(level.power_coefficient_nw(), kLevel.power_coefficient_nw);
  EXPECT_EQ(level.control(), kLevel.control_interface);
  EXPECT_EQ(level.control_argument(), kLevel.control_argument);
  EXPECT_EQ(level.type(), PowerLevel::Type::kActive);
  EXPECT_EQ(level.name(), std::string_view(kLevel.diagnostic_name));
  EXPECT_FALSE(level.TargetsPowerDomain());
  EXPECT_TRUE(level.TargetsCpus());
}

TEST(PowerLevelTransitionTest, Ctor) {
  static constexpr ProcessorPowerLevelTransition kTransition = {
      .latency = 456,
      .energy_nj = 1234,
      .from = 0,
      .to = 1,
  };

  PowerLevelTransition transition(kTransition);

  EXPECT_EQ(transition.latency(), kTransition.latency);
  EXPECT_EQ(transition.energy_cost_nj(), kTransition.energy_nj);
}

TEST(PowerModelTest, Create) {
  static constexpr auto kPowerLevels = std::to_array<ProcessorPowerLevel>({
      {
          .options = 0,
          .processing_rate = 0,
          .power_coefficient_nw = 1,
          .control_interface = ControlInterface::kArmPsci,
          .control_argument = 1,
          .diagnostic_name = "0",
      },
      {
          .options = 0,
          .processing_rate = 4,
          .power_coefficient_nw = 8,
          .control_interface = ControlInterface::kCpuDriver,
          .control_argument = 3,
          .diagnostic_name = "1",
      },
      {
          .options = 0,
          .processing_rate = 0,
          .power_coefficient_nw = 2,
          .control_interface = ControlInterface::kArmWfi,
          .control_argument = 0,
          .diagnostic_name = "2",
      },
      {
          .options = 0,
          .processing_rate = 4,
          .power_coefficient_nw = 10,
          .control_interface = ControlInterface::kCpuDriver,
          .control_argument = 1,
          .diagnostic_name = "3",
      },
  });

  static constexpr auto kTransitions = std::to_array<ProcessorPowerLevelTransition>({
      {
          .latency = 1,
          .energy_nj = 2,
          .from = 1,
          .to = 0,
      },
      {
          .latency = 2,
          .energy_nj = 3,
          .from = 2,
          .to = 0,
      },
      {
          .latency = 3,
          .energy_nj = 4,
          .from = 3,
          .to = 0,
      },
      {
          .latency = 1,
          .energy_nj = 2,
          .from = 0,
          .to = 1,
      },
      {
          .latency = 3,
          .energy_nj = 4,
          .from = 2,
          .to = 1,
      },
      {
          .latency = 4,
          .energy_nj = 5,
          .from = 3,
          .to = 1,
      },
      {
          .latency = 2,
          .energy_nj = 3,
          .from = 0,
          .to = 2,
      },
      {
          .latency = 3,
          .energy_nj = 4,
          .from = 1,
          .to = 2,
      },
      {
          .latency = 5,
          .energy_nj = 6,
          .from = 3,
          .to = 2,
      },
      {
          .latency = 3,
          .energy_nj = 4,
          .from = 0,
          .to = 3,
      },
      {
          .latency = 4,
          .energy_nj = 5,
          .from = 1,
          .to = 3,
      },
      {
          .latency = 5,
          .energy_nj = 6,
          .from = 2,
          .to = 3,
      },
  });

  auto energy_model = EnergyModel::Create(kPowerLevels, kTransitions);
  ASSERT_TRUE(energy_model.is_ok());

  // Proper transformation of the model and the transition table.
  auto check_level = [](const PowerLevel& actual, const PowerLevel& expected) {
    EXPECT_EQ(actual.level(), expected.level());
    EXPECT_EQ(actual.control(), expected.control());
    EXPECT_EQ(actual.control_argument(), expected.control_argument());
    EXPECT_EQ(actual.name(), expected.name());
    EXPECT_EQ(actual.power_coefficient_nw(), expected.power_coefficient_nw());
    EXPECT_EQ(actual.processing_rate(), expected.processing_rate());
    EXPECT_EQ(actual.type(), expected.type());
    EXPECT_EQ(actual.TargetsCpus(), expected.TargetsCpus());
    EXPECT_EQ(actual.TargetsPowerDomain(), expected.TargetsPowerDomain());
  };

  ASSERT_EQ(energy_model->levels().size(), 4u);
  auto levels = energy_model->levels();
  for (size_t i = 0; i < levels.size() - 1; ++i) {
    size_t j = i + 1;
    EXPECT_LE(levels[i].processing_rate(), levels[i].processing_rate());
    if (levels[i].processing_rate() == levels[j].processing_rate()) {
      EXPECT_LE(levels[i].power_coefficient_nw(), levels[j].power_coefficient_nw());
    }
    check_level(levels[i], PowerLevel(levels[i].level(), kPowerLevels[levels[i].level()]));
    check_level(levels[j], PowerLevel(levels[j].level(), kPowerLevels[levels[j].level()]));
  }

  auto get_original_transition = [&levels](size_t i, size_t j) {
    size_t og_i = levels[i].level();
    size_t og_j = levels[j].level();
    for (const auto& transition : kTransitions) {
      if (transition.from == og_i && transition.to == og_j) {
        return PowerLevelTransition(transition);
      }
    }
    return PowerLevelTransition::Invalid();
  };

  auto transitions = energy_model->transitions();
  for (size_t i = 0; i < levels.size(); ++i) {
    for (size_t j = 0; j < levels.size(); ++j) {
      auto transition = transitions[i][j];
      auto og_transition = get_original_transition(i, j);
      EXPECT_EQ(transition.latency(), og_transition.latency());
      EXPECT_EQ(transition.energy_cost_nj(), og_transition.energy_cost_nj());
    }
  }

  // Properly partitioned.
  ASSERT_EQ(energy_model->idle_levels().size(), 2u);
  EXPECT_EQ(energy_model->idle_levels()[0].level(), 0u);
  EXPECT_EQ(energy_model->idle_levels()[1].level(), 2u);

  ASSERT_EQ(energy_model->active_levels().size(), 2u);
  EXPECT_EQ(energy_model->active_levels()[0].level(), 1u);
  EXPECT_EQ(energy_model->active_levels()[1].level(), 3u);

  // Sorter by tuple <Control Interface, Control Argument>
  for (size_t i = 0; i < levels.size(); ++i) {
    auto& level = levels[i];
    EXPECT_EQ(energy_model->FindPowerLevel(level.control(), level.control_argument()), i);
  }
  EXPECT_FALSE(energy_model->FindPowerLevel(ControlInterface::kArmPsci, 495));
  EXPECT_FALSE(energy_model->FindPowerLevel(static_cast<ControlInterface>(495), 0));
}

TEST(PowerModelTest, CreateWithEmptyTransitionsIsOk) {
  static constexpr auto kPowerLevels = std::to_array<ProcessorPowerLevel>({
      {
          .options = 0,
          .processing_rate = 0,
          .power_coefficient_nw = 1,
          .control_interface = ControlInterface::kArmPsci,
          .control_argument = 1,
          .diagnostic_name = "0",
      },
      {
          .options = 0,
          .processing_rate = 4,
          .power_coefficient_nw = 8,
          .control_interface = ControlInterface::kCpuDriver,
          .control_argument = 3,
          .diagnostic_name = "1",
      },
      {
          .options = 0,
          .processing_rate = 0,
          .power_coefficient_nw = 2,
          .control_interface = ControlInterface::kArmWfi,
          .control_argument = 0,
          .diagnostic_name = "2",
      },
      {
          .options = 0,
          .processing_rate = 4,
          .power_coefficient_nw = 10,
          .control_interface = ControlInterface::kCpuDriver,
          .control_argument = 1,
          .diagnostic_name = "3",
      },
  });

  auto energy_model = EnergyModel::Create(kPowerLevels, {});
  ASSERT_TRUE(energy_model.is_ok());

  // Proper transformation of the model and the transition table.
  auto check_level = [](const PowerLevel& actual, const PowerLevel& expected) {
    EXPECT_EQ(actual.level(), expected.level());
    EXPECT_EQ(actual.control(), expected.control());
    EXPECT_EQ(actual.control_argument(), expected.control_argument());
    EXPECT_EQ(actual.name(), expected.name());
    EXPECT_EQ(actual.power_coefficient_nw(), expected.power_coefficient_nw());
    EXPECT_EQ(actual.processing_rate(), expected.processing_rate());
    EXPECT_EQ(actual.type(), expected.type());
    EXPECT_EQ(actual.TargetsCpus(), expected.TargetsCpus());
    EXPECT_EQ(actual.TargetsPowerDomain(), expected.TargetsPowerDomain());
  };

  ASSERT_EQ(energy_model->levels().size(), 4u);
  auto levels = energy_model->levels();
  for (size_t i = 0; i < levels.size() - 1; ++i) {
    size_t j = i + 1;
    EXPECT_LE(levels[i].processing_rate(), levels[i].processing_rate());
    if (levels[i].processing_rate() == levels[j].processing_rate()) {
      EXPECT_LE(levels[i].power_coefficient_nw(), levels[j].power_coefficient_nw());
    }
    check_level(levels[i], PowerLevel(levels[i].level(), kPowerLevels[levels[i].level()]));
    check_level(levels[j], PowerLevel(levels[j].level(), kPowerLevels[levels[j].level()]));
  }

  for (size_t row = 0; row < energy_model->levels().size(); ++row) {
    for (size_t column = 0; column < energy_model->levels().size(); ++column) {
      const PowerLevelTransition& transition = energy_model->transitions()[row][column];
      EXPECT_EQ(transition.energy_cost_nj(), PowerLevelTransition::Zero().energy_cost_nj());
      EXPECT_EQ(transition.latency(), PowerLevelTransition::Zero().latency());
    }
  }

  ASSERT_EQ(energy_model->idle_levels().size(), 2u);
  EXPECT_EQ(energy_model->idle_levels()[0].level(), 0u);
  EXPECT_EQ(energy_model->idle_levels()[1].level(), 2u);

  ASSERT_EQ(energy_model->active_levels().size(), 2u);
  EXPECT_EQ(energy_model->active_levels()[0].level(), 1u);
  EXPECT_EQ(energy_model->active_levels()[1].level(), 3u);

  // Sorter by tuple <Control Interface, Control Argument>
  for (size_t i = 0; i < levels.size(); ++i) {
    auto& level = levels[i];
    EXPECT_EQ(energy_model->FindPowerLevel(level.control(), level.control_argument()), i);
  }
  EXPECT_FALSE(energy_model->FindPowerLevel(ControlInterface::kArmPsci, 495));
  EXPECT_FALSE(energy_model->FindPowerLevel(static_cast<ControlInterface>(495), 0));
}

TEST(EnergyModelTest, HeterogeneousMultiDomainScaling) {
  // Domain 0: Little cores, max raw processing rate = 150.
  static constexpr auto kLittleLevels = std::to_array<ProcessorPowerLevel>({
      {
          .options = 0,
          .processing_rate = 0,
          .power_coefficient_nw = 10,
          .control_interface = ControlInterface::kArmWfi,
          .control_argument = 0,
          .diagnostic_name = "wfi",
      },
      {
          .options = 0,
          .processing_rate = 50,
          .power_coefficient_nw = 100,
          .control_interface = ControlInterface::kCpuDriver,
          .control_argument = 0,
          .diagnostic_name = "opp0",
      },
      {
          .options = 0,
          .processing_rate = 150,
          .power_coefficient_nw = 300,
          .control_interface = ControlInterface::kCpuDriver,
          .control_argument = 1,
          .diagnostic_name = "opp1",
      },
  });

  // Domain 1: Big cores, max raw processing rate = 1000.
  static constexpr auto kBigLevels = std::to_array<ProcessorPowerLevel>({
      {
          .options = 0,
          .processing_rate = 0,
          .power_coefficient_nw = 50,
          .control_interface = ControlInterface::kArmWfi,
          .control_argument = 0,
          .diagnostic_name = "wfi",
      },
      {
          .options = 0,
          .processing_rate = 500,
          .power_coefficient_nw = 1000,
          .control_interface = ControlInterface::kCpuDriver,
          .control_argument = 0,
          .diagnostic_name = "opp0",
      },
      {
          .options = 0,
          .processing_rate = 1000,
          .power_coefficient_nw = 2500,
          .control_interface = ControlInterface::kCpuDriver,
          .control_argument = 1,
          .diagnostic_name = "opp1",
      },
  });

  constexpr uint64_t kSystemMaxProcessingRate = 1000;

  // Create Domain 0 EnergyModel normalized to system peak rate (1000).
  auto little_model = EnergyModel::Create(kLittleLevels, {}, kSystemMaxProcessingRate);
  ASSERT_TRUE(little_model.is_ok());
  EXPECT_EQ(little_model->max_processing_rate_scale(), kSystemMaxProcessingRate);

  // Domain 0 levels should be normalized against 1000 rather than local max (150).
  const auto little_levels = little_model->levels();
  ASSERT_EQ(little_levels.size(), 3u);
  EXPECT_EQ(little_levels[0].processing_rate(), ProcessingRate{0});
  EXPECT_EQ(little_levels[1].processing_rate(),
            PowerLevel::ToProcessingRate(50, kSystemMaxProcessingRate));
  EXPECT_EQ(little_levels[2].processing_rate(),
            PowerLevel::ToProcessingRate(150, kSystemMaxProcessingRate));

  // ToProcessingRate / FromProcessingRate on little model must use denominator 1000.
  EXPECT_EQ(little_model->ToProcessingRate(150),
            PowerLevel::ToProcessingRate(150, kSystemMaxProcessingRate));
  EXPECT_EQ(little_model->FromProcessingRate(little_levels[2].processing_rate()), 150u);

  // Create Domain 1 EnergyModel normalized to system peak rate (1000).
  auto big_model = EnergyModel::Create(kBigLevels, {}, kSystemMaxProcessingRate);
  ASSERT_TRUE(big_model.is_ok());
  EXPECT_EQ(big_model->max_processing_rate_scale(), kSystemMaxProcessingRate);

  const auto big_levels = big_model->levels();
  ASSERT_EQ(big_levels.size(), 3u);
  EXPECT_EQ(big_levels[0].processing_rate(), ProcessingRate{0});
  EXPECT_EQ(big_levels[1].processing_rate(),
            PowerLevel::ToProcessingRate(500, kSystemMaxProcessingRate));
  EXPECT_EQ(big_levels[2].processing_rate(),
            PowerLevel::ToProcessingRate(1000, kSystemMaxProcessingRate));
  EXPECT_EQ(big_levels[2].processing_rate(), ProcessingRate{1});
}

TEST(PowerDomainSetTest, FindDomain) {
  std::array domains_to_register{
      MakePowerDomainHelper(0, 1, 2, 3),
      MakePowerDomainHelper(1, 4, 5, 6),
      MakePowerDomainHelper(2, 7, 8, 9),
  };

  std::array<PowerDomain*, 3> raw_domains;
  for (size_t i = 0; i < domains_to_register.size(); ++i) {
    raw_domains[i] = domains_to_register[i].get();
  }

  auto domain_set_result = PowerDomainSet::Create(domains_to_register);
  ASSERT_TRUE(domain_set_result.is_ok());
  const auto& domain_set = domain_set_result.value();

  for (const auto* domain : raw_domains) {
    EXPECT_EQ(domain_set.FindByDomainId(domain->id()), domain);
  }

  EXPECT_EQ(domain_set.FindByDomainId(112345567), nullptr);
}

TEST(PowerDomainSetTest, CreateValidation) {
  std::array valid_domains = {
      MakePowerDomainHelper(0, 0, 1, 2),
      MakePowerDomainHelper(1, 4, 5, 6),
      MakePowerDomainHelper(2, 8, 9, 10),
  };

  std::array<PowerDomain*, 3> raw_valid_domains;
  for (size_t i = 0; i < valid_domains.size(); ++i) {
    raw_valid_domains[i] = valid_domains[i].get();
  }

  auto domain_set_result = PowerDomainSet::Create(valid_domains);
  ASSERT_TRUE(domain_set_result.is_ok());
  const auto& domain_set = domain_set_result.value();

  EXPECT_EQ(domain_set.count(), valid_domains.size());
  domain_set.Visit([&](const fbl::RefPtr<PowerDomain>& domain) {
    EXPECT_TRUE(InRange(raw_valid_domains, domain.get()));
  });

  // Duplicate domain ID.
  std::array duplicate_id_domains = {
      MakePowerDomainHelper(0, 0, 1),
      MakePowerDomainHelper(0, 2, 3),
  };
  EXPECT_EQ(PowerDomainSet::Create(duplicate_id_domains).status_value(), ZX_ERR_INVALID_ARGS);

  // Overlapping CPU mask.
  std::array overlapping_cpu_domains = {
      MakePowerDomainHelper(0, 0, 1),
      MakePowerDomainHelper(1, 1, 2),
  };
  EXPECT_EQ(PowerDomainSet::Create(overlapping_cpu_domains).status_value(), ZX_ERR_INVALID_ARGS);

  // Null domain reference.
  fbl::RefPtr<PowerDomain> null_domain = nullptr;
  EXPECT_EQ(PowerDomainSet::Create({null_domain}).status_value(), ZX_ERR_INVALID_ARGS);

  // Exceeding kMaxPowerDomains (limit 4).
  std::array too_many_domains = {
      MakePowerDomainHelper(0, 0), MakePowerDomainHelper(1, 1), MakePowerDomainHelper(2, 2),
      MakePowerDomainHelper(3, 3), MakePowerDomainHelper(4, 4),
  };
  EXPECT_EQ(PowerDomainSet::Create(too_many_domains).status_value(), ZX_ERR_OUT_OF_RANGE);
}

}  // namespace
