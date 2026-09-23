// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "test-helper.h"

#include <cstddef>

power_management::EnergyModel MakeFakeEnergyModel(size_t power_levels) {
  fbl::Vector<power_management::ProcessorPowerLevel> levels;
  levels.resize(power_levels);

  fbl::Vector<power_management::ProcessorPowerLevelTransition> transitions;
  transitions.resize(power_levels * power_levels);

  for (size_t i = 0; i < power_levels; ++i) {
    levels[i].control_interface = ControlInterfaceIdForLevel(i);
    levels[i].control_argument = ControlInterfaceArgForLevel(i);
    levels[i].options =
        i <= kMaxIdlePowerLevel ? power_management::kPowerLevelOptionsDomainIndependent : 0;
    levels[i].processing_rate = i <= kMaxIdlePowerLevel ? 0 : i * 10;
    levels[i].power_coefficient_nw = i + 1;
    levels[i].diagnostic_name = "fake";
    for (size_t j = 0; j < power_levels; ++j) {
      transitions[j + i * power_levels].from = static_cast<uint8_t>(i);
      transitions[j + i * power_levels].to = static_cast<uint8_t>(j);
    }
  }

  auto model = power_management::EnergyModel::Create(levels, transitions);
  ZX_ASSERT(model.is_ok());
  return std::move(model).value();
}
