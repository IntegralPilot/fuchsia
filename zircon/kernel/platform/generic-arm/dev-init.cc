// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>

#include <dev/arm_smmu/smmu.h>
#include <dev/arm_smmu/smmu_mode.h>
#include <dev/clocks_and_pmic/moonflower/init.h>
#include <dev/hdcp/amlogic_s912/init.h>
#include <dev/hw_rng/amlogic_rng/init.h>
#include <dev/hw_rng/qcom_rng/init.h>
#include <dev/hw_watchdog/generic32/init.h>
#include <dev/init.h>
#include <dev/interrupt/arm_gicv2_init.h>
#include <dev/interrupt/arm_gicv3_init.h>
#include <dev/power/iris/init.h>
#include <dev/power/moonflower/init.h>
#include <dev/power/motmot/init.h>
#include <dev/psci.h>
#include <dev/timer/arm_generic.h>
#include <dev/timer/armv7_mmio_timer.h>
#include <ktl/type_traits.h>
#include <ktl/variant.h>
#include <phys/arch/arch-handoff.h>

#include <ktl/enforce.h>

namespace {

// Overloads to satisfy the degenerate 'no config present' case in
// `ktl::visit(..., arch_handoff.gic_driver)` below. Related overloads defined
// in <dev/interrupt/arm_gicv{2,3}_init.h>.
void ArmGicInitEarly(const ktl::monostate& no_config) {}
void ArmGicInitPostVm(const ktl::monostate& no_config) {}
void ArmGicInitLate(const ktl::monostate& no_config) {}

}  // namespace

void PlatformDriverHandoffEarly(const ArchPhysHandoff& arch_handoff) {
  // Configure the GIC first so that the remaining drivers can freely register
  // interrupt handlers.
  ktl::visit([](const auto& config) { ArmGicInitEarly(config); }, arch_handoff.gic_driver);

  if (arch_handoff.generic32_watchdog_driver) {
    generic_32bit_watchdog_early_init(*arch_handoff.generic32_watchdog_driver.to_std());
  }

  if (arch_handoff.generic_timer_driver) {
    ArmGenericTimerInit(*arch_handoff.generic_timer_driver.to_std());
  }

  // Initialize psci before the other power drivers, as they may decide to
  // override the psci's power registration.
  if (arch_handoff.psci_driver) {
    PsciInit(*arch_handoff.psci_driver.to_std(), arch_handoff.psci_cpu_suspend_driver.get());
  }

  if (arch_handoff.motmot_power_driver) {
    motmot_power_init_early();
  }

  if (arch_handoff.moonflower_power_driver) {
    moonflower_power_init_early();
  }

  if (!arch_handoff.iris_power_driver.empty()) {
    iris_power_init_early();
  }

  if (BootOptions::Get()->allow_debug_uart_suspend) {
    // TODO(johngro): make this more generic.  IOW - make sure to do the right
    // thing for non-moonflower HW as well.
    moonflower_clocks_and_pmic_init_early();
  }
}

void PlatformDriverHandoffPostVm(const ArchPhysHandoff& arch_handoff) {
  // Initialize the GIC post VM so it can map its own mmio registers
  ktl::visit([](const auto& config) { ArmGicInitPostVm(config); }, arch_handoff.gic_driver);

  if (arch_handoff.generic_timer_driver) {
    ArmGenericTimerInitPostVm(*arch_handoff.generic_timer_driver.to_std());
  }

  if (arch_handoff.generic32_watchdog_driver) {
    generic_32bit_watchdog_init_post_vm(*arch_handoff.generic32_watchdog_driver.to_std());
  }
}

void PlatformDriverHandoffLate(const ArchPhysHandoff& arch_handoff) {
  // First, as above.
  ktl::visit([](const auto& config) { ArmGicInitLate(config); }, arch_handoff.gic_driver);

  // TODO(johngro): we are currently initing the mmio timers rather late in the
  // boot because we currently use the heap in the driver.  Consider moving to
  // static allocation so we can init early instead.
  if (arch_handoff.generic_timer_mmio_driver) {
    Armv7MmioTimer::Init(*arch_handoff.generic_timer_mmio_driver.to_std());
  }

  // If we have any SMMU descriptions in the ZBI, check the kernel.arm-smmu-mode
  // boot option for any parse errors and print a warning if we see any.  The
  // SMMU driver will tolerate these errors, falling back on default values for
  // the operational mode, but we should print a warning.
  if (const char* const mode_string = BootOptions::Get()->arm_smmu_mode.data();
      (arch_handoff.arm_smmu_drivers.size() > 0) &&
      !::arm_smmu::ValidateSmmuModeString(mode_string)) {
    dprintf(INFO, "WARNING - Parse errors present in kernel.arm-smmu-mode string \"%s\"\n",
            mode_string);
  }

  for (const zbi_dcfg_arm_smmu_driver_t& config : arch_handoff.arm_smmu_drivers.get()) {
    zx::result<fbl::RefPtr<iommu::Iommu>> result = ArmSmmu::Create(config);
    if (result.is_error()) {
      dprintf(INFO, "WARNING - Failed to construct ARM SMMU @ 0x%08lx result %d\n",
              config.mmio_phys, result.error_value());
    }
  }

  if (arch_handoff.amlogic_hdcp_driver) {
    AmlogicS912HdcpInit(*arch_handoff.amlogic_hdcp_driver.to_std());
  }

  if (arch_handoff.amlogic_rng_driver) {
    AmlogicRngInit(*arch_handoff.amlogic_rng_driver.to_std());
  }

  if (arch_handoff.qcom_rng_driver) {
    QcomRngInit(*arch_handoff.qcom_rng_driver.to_std());
  }

  if (arch_handoff.generic32_watchdog_driver) {
    generic_32bit_watchdog_late_init();
  }

  if (arch_handoff.moonflower_power_driver) {
    moonflower_power_init();
  }

  if (!arch_handoff.iris_power_driver.empty()) {
    iris_power_init(arch_handoff.iris_power_driver.data(), arch_handoff.iris_power_driver.size());
  }
}
