// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ARCH_HANDOFF_H_
#define ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ARCH_HANDOFF_H_

// Note: we refrain from using the ktl namespace as <phys/handoff.h> is
// expected to be compiled in the userboot toolchain.

#include <lib/arch/arm64/smccc.h>
#include <lib/boot-options/arm64.h>
#include <lib/stdbind/optional.h>
#include <lib/zbi-format/driver-config.h>
#include <zircon/tls.h>

#include <optional>
#include <variant>

#include <phys/handoff-ptr.h>

// The minimal memory region needed to encapsulate the C++ compiler thread ABI.
struct ArchTempThreadAbi {
  constexpr const void* tp() const { return static_cast<const void*>(this + 1); }

  uint64_t stack_guard = 0;
  uint64_t unsafe_stack_pointer = 0;
};

static_assert(sizeof(ArchTempThreadAbi) + ZX_TLS_STACK_GUARD_OFFSET ==
              offsetof(ArchTempThreadAbi, stack_guard));

static_assert(sizeof(ArchTempThreadAbi) + ZX_TLS_UNSAFE_SP_OFFSET ==
              offsetof(ArchTempThreadAbi, unsafe_stack_pointer));

struct ArchPatchInfo {
  Arm64AlternateVbar alternate_vbar = Arm64AlternateVbar::kNone;
};

// This holds (or points to) all arm64-specific data that is handed off from
// physboot to the kernel proper at boot time.
struct ArchPhysHandoff {
  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_AMLOGIC_HDCP) payload.
  stdbind::optional<zbi_dcfg_amlogic_hdcp_driver_t> amlogic_hdcp_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_QCOM_RNG) or
  stdbind::optional<zbi_dcfg_qcom_rng_t> qcom_rng_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_AMLOGIC_RNG) payload
  stdbind::optional<zbi_dcfg_amlogic_rng_driver_t> amlogic_rng_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER) payload.
  stdbind::optional<zbi_dcfg_arm_generic_timer_driver_t> generic_timer_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO) payload.
  stdbind::optional<zbi_dcfg_arm_generic_timer_mmio_driver_t> generic_timer_mmio_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_ARM_GIC_V2/ZBI_KERNEL_DRIVER_ARM_GIC_V3) payload.
  std::variant<std::monostate, zbi_dcfg_arm_gic_v2_driver_t, zbi_dcfg_arm_gic_v3_driver_t>
      gic_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_ARM_PSCI) payload.
  stdbind::optional<zbi_dcfg_arm_psci_driver_t> psci_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND_DRIVER) payload.
  PhysHandoffTemporarySpan<const zbi_dcfg_arm_psci_cpu_suspend_state_t> psci_cpu_suspend_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG) payload.
  stdbind::optional<zbi_dcfg_generic32_watchdog_t> generic32_watchdog_driver;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_ARM_SMMU) payload.
  PhysHandoffTemporarySpan<const zbi_dcfg_arm_smmu_driver_t> arm_smmu_drivers;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_MOTMOT_POWER) payload.
  bool motmot_power_driver = false;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_MOONFLOWER_POWER) payload.
  bool moonflower_power_driver = false;

  // (ZBI_TYPE_KERNEL_DRIVER, ZBI_KERNEL_DRIVER_IRIS_POWER) payload.
  PhysHandoffTemporarySpan<const zbi_cpu_energy_model_domain_t> iris_power_driver;

  // See ArchPatchInfo, above.
  Arm64AlternateVbar alternate_vbar = Arm64AlternateVbar::kNone;
};

#endif  // ZIRCON_KERNEL_ARCH_ARM64_PHYS_INCLUDE_PHYS_ARCH_ARCH_HANDOFF_H_
