// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/power-management/power-state.h"

#include <lib/power-management/energy-model.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/errors.h>

#include <algorithm>
#include <optional>

#include <fbl/ref_ptr.h>

namespace power_management {

PowerDomainSet PowerState::UpdatePowerDomainSet(PowerDomainSet power_domain_set, uint32_t cpu_num) {
  PowerDomain* domain = power_domain_set.FindByCpuNum(cpu_num);

  if (domain_) {
    domain_->total_normalized_utilization_.fetch_sub(normalized_utilization_.raw_value(),
                                                     std::memory_order_relaxed);
  }

  active_power_level_ = desired_active_power_level_ = std::nullopt;
  domain_ = fbl::RefPtr<PowerDomain>(domain);

  if (domain_) {
    const zx::result<uint64_t> control_argument_result =
        domain_->controller()->GetCurrentPowerLevel(domain_->id());
    if (control_argument_result.is_ok()) {
      active_power_level_ = desired_active_power_level_ = domain_->model().FindPowerLevel(
          domain_->controller()->control_interface(), control_argument_result.value());
    }

    domain_->total_normalized_utilization_.fetch_add(normalized_utilization_.raw_value(),
                                                     std::memory_order_relaxed);
  }

  using std::swap;
  swap(power_domain_set, power_domain_set_);

  return power_domain_set;
}

std::optional<PowerLevelUpdateRequest> PowerState::RequestTransition(uint32_t cpu_num,
                                                                     uint8_t active_power_level) {
  if (!domain_) {
    return std::nullopt;
  }

  if (!active_power_level_) {
    return std::nullopt;
  }

  ZX_ASSERT(active_power_level >= domain_->model().idle_levels().size() &&
            active_power_level < domain_->model().levels().size());

  if (desired_active_power_level_ == active_power_level) {
    return std::nullopt;
  }

  desired_active_power_level_ = active_power_level;

  const PowerLevel& level = domain_->model().levels()[active_power_level];

  PowerLevelUpdateRequest transition = {
      .domain_id = domain_->id(),
      .target_id = domain_->id(),
      .control = level.control(),
      .control_argument = level.control_argument(),
      .options = 0,
  };

  if (level.TargetsCpus()) {
    transition.target_id = cpu_num;
    transition.options |= kPowerLevelOptionsDomainIndependent;
  }

  return transition;
}

zx::result<> PowerState::UpdateActivePowerLevel(uint8_t level) {
  if (!domain_) {
    return zx::error(ZX_ERR_BAD_STATE);
  }

  if (level < domain_->model().idle_levels().size() || level >= domain_->model().levels().size()) {
    return zx::error(ZX_ERR_OUT_OF_RANGE);
  }

  const bool was_at_desired = desired_active_power_level_ == active_power_level_;
  active_power_level_ = level;
  if (was_at_desired || desired_active_power_level_ == level) {
    desired_active_power_level_ = level;
  }
  return zx::ok();
}

}  // namespace power_management
