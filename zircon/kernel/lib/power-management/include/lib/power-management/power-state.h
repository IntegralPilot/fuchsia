// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_POWER_STATE_H_
#define ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_POWER_STATE_H_

#include <stdint.h>
#include <zircon/errors.h>

#include <atomic>
#include <cstdint>
#include <optional>

#include <fbl/intrusive_single_list.h>
#include <fbl/ref_counted.h>
#include <fbl/ref_ptr.h>

#include "energy-model.h"

namespace power_management {

// Represents a requested power level update to be executed at a later stage.
struct PowerLevelUpdateRequest {
  // Domain ID where the request is originating from.
  uint32_t domain_id;

  // Target ID of the device or domain that should be transitioned.
  uint32_t target_id;

  // Interface this request should be routed to.
  ControlInterface control;

  // Argument used for transitioning through the control interface.
  uint64_t control_argument;

  // Options that determine what `target_id` is. This is determined by the
  // power level being transitioned to.
  uint32_t options;
};

// PowerState encapsulates the current power level, power domain, and energy model for an individual
// processor.
//
// Instances of PowerState are not safe for concurrent use and must be protected by an external lock
// associated with the owning processor.
class PowerState {
 public:
  PowerState() = default;

  // Returns true if the power domain is set.
  bool is_enabled() const { return !!domain(); }

  // Domain the PowerState is being modeled after. Returns nullptr is there is
  // no power domain configured for processor owning this power state.
  const fbl::RefPtr<PowerDomain>& domain() const { return domain_; }

  // Returns the id of the domain this power state belongs to.
  std::optional<uint32_t> domain_id() const {
    if (domain()) {
      return domain()->id();
    }
    return std::nullopt;
  }

  // Returns whether the domain's controller is serving requests. Returns false if there is no
  // domain.
  bool is_serving() const { return domain() && domain()->controller()->is_serving(); }

  // Returns whether the kernel scheduler should send power level update requests through the
  // control interface to handle utilization changes.
  bool scheduler_control_enabled() const {
    return domain() && domain()->scheduler_control_enabled();
  }

  // Returns the active power level when device is not idle.
  constexpr std::optional<uint8_t> active_power_level() const { return active_power_level_; }

  // Returns the active power level the device is in the processo of transitioning to.
  constexpr std::optional<uint8_t> desired_active_power_level() const {
    return desired_active_power_level_;
  }

  // Returns the maximum (i.e. highest power, lowest latency) idle power level.
  std::optional<uint8_t> max_idle_power_level() const {
    if (domain()) {
      return domain()->model().max_idle_power_level();
    }
    return std::nullopt;
  }

  // Returns the power coefficient of the current active power level. Returns zero if there is no
  // active power level or no domain is set.
  uint64_t active_power_coefficient_nw() const {
    if (domain() && active_power_level_.has_value()) {
      return domain()->model().levels()[*active_power_level_].power_coefficient_nw();
    }
    return 0;
  }

  // Returns the power coefficient of the maximum idle power level. Returns zero if there is no
  // idle power level or no domain is set.
  uint64_t max_idle_power_coefficient_nw() const {
    if (domain()) {
      return domain()->model().max_idle_power_coefficient_nw().value_or(0);
    }
    return 0;
  }

  // Returns the control interface of the maximum idle power level. Returns nullopt if there is no
  // idle power level or no domain is set.
  std::optional<ControlInterface> max_idle_power_level_interface() const {
    if (domain()) {
      return domain()->model().max_idle_power_level_interface();
    }
    return std::nullopt;
  }

  // Returns true if the given power level is valid active power level.
  bool IsValidActivePowerLevel(uint8_t power_level) const {
    if (domain()) {
      return power_level >= domain()->model().idle_levels().size() &&
             power_level < domain()->model().levels().size();
    }
    return false;
  }

  // Returns the processing rate of the fastest power level in this domain.
  ProcessingRate max_processing_rate() const {
    if (domain()) {
      return domain()->model().max_processing_rate();
    }
    return ProcessingRate{0};
  }

  // Returns the processing rate of the current active power level. Returns zero if there is no
  // active power level or no domain is set.
  ProcessingRate active_processing_rate() const {
    if (domain() && active_power_level_.has_value()) {
      return domain()->model().levels()[*active_power_level_].processing_rate();
    }
    return ProcessingRate{0};
  }

  // Returns the processing rate of the preceding active power level (i.e. the
  // lower bound of the current active power level). Returns zero if there is no
  // preceding active power level, no active power level is set, or no domain
  // is set.
  ProcessingRate preceding_active_processing_rate() const {
    if (domain() && active_power_level_.has_value()) {
      const PowerLevel* power_level = &domain()->model().levels()[*active_power_level_];
      if (power_level != &domain()->model().active_levels().front()) {
        return (power_level - 1)->processing_rate();
      }
    }
    return ProcessingRate{0};
  }

  // Returns the targeted active power level (the in-flight desired level if set,
  // otherwise the current active level).
  constexpr std::optional<uint8_t> target_active_power_level() const {
    return desired_active_power_level_.has_value() ? desired_active_power_level_
                                                   : active_power_level_;
  }

  // Returns the processing rate of the targeted active power level (the in-flight
  // desired level if set, otherwise the current active level).
  ProcessingRate target_processing_rate() const {
    if (domain()) {
      if (const auto level = target_active_power_level()) {
        return domain()->model().levels()[*level].processing_rate();
      }
    }
    return ProcessingRate{0};
  }

  // Returns the processing rate of the active power level immediately preceding
  // the targeted active power level (i.e. the lower bound of the targeted active
  // power level).
  ProcessingRate preceding_target_processing_rate() const {
    if (domain()) {
      if (const auto level = target_active_power_level()) {
        const PowerLevel* power_level = &domain()->model().levels()[*level];
        if (power_level != &domain()->model().active_levels().front()) {
          return (power_level - 1)->processing_rate();
        }
      }
    }
    return ProcessingRate{0};
  }

  // Returns true if the targeted active power level has an active power level
  // preceding it (i.e. is not the lowest active power level).
  bool has_preceding_target_power_level() const {
    if (domain()) {
      if (const auto level = target_active_power_level()) {
        return &domain()->model().levels()[*level] != &domain()->model().active_levels().front();
      }
    }
    return false;
  }

  // Returns the current utilization of the processor.
  Utilization normalized_utilization() const { return normalized_utilization_; }

  // Returns the total normalized utilization of the domain this processor belongs to.
  Utilization total_normalized_utilization() const {
    if (domain()) {
      return domain()->total_normalized_utilization();
    }
    return Utilization{0};
  }

  // Updates the power domain set and associates this power state with the domain in the set
  // matching the given CPU number. Returns the previous power domain set.
  PowerDomainSet UpdatePowerDomainSet(PowerDomainSet power_domain_set, uint32_t cpu_num);

  const PowerDomainSet& power_domain_set() const { return power_domain_set_; }

  // Posts a request to transition the power domain associated with this power state to a given
  // active power level from the current energy model.
  //
  // Ignored when no domain is set. Asserts that the requested power level is an active power level.
  //
  // `cpu_num` is the CPU associated with this power state.
  // `level` is the desired power level.
  //
  std::optional<PowerLevelUpdateRequest> RequestTransition(uint32_t cpu_num,
                                                           uint8_t active_power_level);

  // Updates the active power level of this power state. Typically called in response to the power
  // level controller completing a power level transition request.
  zx::result<> UpdateActivePowerLevel(uint8_t active_power_level);

  // Update the utilization of this processor and the total utilization of its domain.
  Utilization UpdateUtilization(Utilization utilization_delta) {
    if (domain()) {
      domain()->total_normalized_utilization_.fetch_add(utilization_delta.raw_value(),
                                                        std::memory_order_relaxed);
    }
    return normalized_utilization_ += utilization_delta;
  }

 private:
  // The power domain set configured for this processor.
  PowerDomainSet power_domain_set_;

  // The power domain the processor belongs to.
  fbl::RefPtr<PowerDomain> domain_{nullptr};

  // The power level when the processor is actively running.
  std::optional<uint8_t> active_power_level_;

  // Represents the power level that needs be to transitioned to.
  std::optional<uint8_t> desired_active_power_level_;

  // The current normalized utilization of the processor.
  Utilization normalized_utilization_{0};
};

}  // namespace power_management

#endif  // ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_INCLUDE_LIB_POWER_MANAGEMENT_POWER_STATE_H_
