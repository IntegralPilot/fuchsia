// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_TEST_HELPER_H_
#define ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_TEST_HELPER_H_

#include <lib/power-management/energy-model.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>

#include <fbl/ref_ptr.h>

constexpr uint8_t kDomainIndependantPowerLevel = 0;
constexpr uint8_t kDefaultMaxPowerLevels = 10;
constexpr uint8_t kMaxIdlePowerLevel = 2;
constexpr uint8_t kMinActivePowerLevel = kMaxIdlePowerLevel + 1;

// Makes a power model of `power_levels` using the helpers below to determine
// the level properties. In many cases costs are defined in an arbitrary way based on
// the level indexes.
power_management::EnergyModel MakeFakeEnergyModel(size_t power_levels);

template <typename... Cpus>
cpu_mask_t MakeCpuMask(Cpus... cpus) {
  cpu_mask_t mask = 0;
  auto set_bit = [&mask](size_t num_cpu) {
    mask |= cpu_num_to_mask(static_cast<uint32_t>(num_cpu));
    return true;
  };
  (set_bit(cpus) && ...);
  return mask;
}

struct FakePowerLevelController : public power_management::PowerLevelController {
  FakePowerLevelController()
      : PowerLevelController(power_management::ControlInterface::kCpuDriver) {}

  zx::result<uint32_t> Post(const power_management::PowerLevelUpdateRequest& pending) final {
    return zx::ok(0);
  }

  zx::result<uint64_t> GetCurrentPowerLevel(uint32_t domain_id) const {
    if (current_power_level.has_value()) {
      return zx::ok(current_power_level.value());
    }
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }

  uint64_t id() const final { return 0; }

  std::optional<uint64_t> current_power_level;
};

#include <fbl/ref_counted.h>

inline fbl::RefPtr<power_management::PowerDomain> MakePowerDomain(
    uint32_t id, power_management::EnergyModel& model, cpu_mask_t cpus) {
  return fbl::MakeRefCounted<power_management::PowerDomain>(
      id, cpus, std::move(model), fbl::MakeRefCounted<FakePowerLevelController>());
}

template <typename... Cpus>
inline auto MakePowerDomainHelper(uint32_t id, Cpus... cpus) {
  auto model = MakeFakeEnergyModel(kDefaultMaxPowerLevels);
  return MakePowerDomain(id, model, MakeCpuMask(cpus...));
}

template <typename... Cpus>
inline auto MakePowerDomainHelper(uint32_t id, power_management::EnergyModel& model, Cpus... cpus) {
  return MakePowerDomain(id, model, MakeCpuMask(cpus...));
}

template <typename CpuVisitor>
void ForEachCpuIn(cpu_mask_t cpus, CpuVisitor&& visitor) {
  uint32_t cpu_num = 0;
  while (cpus > 0) {
    if (cpus & 1) {
      visitor(cpu_num);
    }
    cpus >>= 1;
    cpu_num++;
  }
}

constexpr power_management::ControlInterface ControlInterfaceIdForLevel(size_t i) {
  // PSCI retention and powerdown levels.
  if (i < kMaxIdlePowerLevel) {
    return power_management::ControlInterface::kArmPsci;
  }

  // WFI power level. This idle state can also be entered via PSCI standby, which is redundant.
  if (i == kMaxIdlePowerLevel) {
    return power_management::ControlInterface::kArmWfi;
  }

  // Active power levels.
  return power_management::ControlInterface::kCpuDriver;
}

constexpr uint64_t ControlInterfaceArgForLevel(size_t i) { return i; }

#endif  // ZIRCON_KERNEL_LIB_POWER_MANAGEMENT_TEST_HELPER_H_
