// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "psci.h"

#include <lib/arch/arm64/smccc.h>
#include <lib/arch/arm64/system.h>
#include <lib/boot-options/boot-options.h>
#include <lib/zbi-format/driver-config.h>
#include <stdint.h>
#include <zircon/assert.h>

#include <phys/arch/arch-phys-info.h>
#include <phys/main.h>
#include <phys/stdio.h>

#include "smccc.h"

constexpr uint64_t kReset2 = static_cast<uint64_t>(arch::ArmSmcccFunction::kPsciSystemReset2);

void ArmPsciSetup(const zbi_dcfg_arm_psci_driver_t* cfg) {
  gArchPhysInfo->psci_reset_registers = {
      static_cast<uint64_t>(arch::ArmSmcccFunction::kPsciSystemReset),
  };

  if (!cfg) {
    gArchPhysInfo->smccc_disabled = true;
    debugf("%s: No ZBI_KERNEL_DRIVER_ARM_PSCI item found in ZBI.  Early PSCI disabled.\n",
           ProgramName());
    return;
  }

  gArchPhysInfo->smccc_disabled = false;
  gArchPhysInfo->smccc_use_hvc = cfg->use_hvc && arch::ArmCurrentEl::Read().el() < 2;

  const uint64_t* reset_args = nullptr;
  switch (BootOptions::Get()->phys_psci_reset) {
    case Arm64PhysPsciReset::kDisabled:
      gArchPhysInfo->smccc_disabled = true;
      debugf("%s: Early PSCI disabled by boot option.\n", ProgramName());
      return;
    case Arm64PhysPsciReset::kShutdown:
      reset_args = cfg->shutdown_args;
      break;
    case Arm64PhysPsciReset::kReboot:
      reset_args = cfg->reboot_args;
      break;
    case Arm64PhysPsciReset::kRebootBootloader:
      reset_args = cfg->reboot_bootloader_args;
      break;
    case Arm64PhysPsciReset::kRebootRecovery:
      reset_args = cfg->reboot_recovery_args;
      break;
    default:
      ZX_PANIC("impossible phys_psci_reset value %#x",
               static_cast<uint32_t>(BootOptions::Get()->phys_psci_reset));
  }

  gArchPhysInfo->psci_reset_registers[1] = reset_args[0];
  gArchPhysInfo->psci_reset_registers[2] = reset_args[1];
  gArchPhysInfo->psci_reset_registers[3] = reset_args[2];

  arch::ArmSmcccVersion version =
      arch::ArmSmcccVersionResult(ArmSmcccCall(arch::ArmSmcccFunction::kPsciPsciVersion), {0, 0});
  gArchPhysInfo->have_psci_features = version.major >= 1;

  const bool have_reset2 = gArchPhysInfo->have_psci_features &&
                           ArmSmcccCall(arch::ArmSmcccFunction::kPsciPsciFeatures, kReset2) == 0;

  if (have_reset2) {
    gArchPhysInfo->psci_reset_registers.front() = kReset2;
  }

  const char* insn = gArchPhysInfo->smccc_use_hvc ? "HVC" : "SMC";
  const char* cmd = have_reset2 ? "RESET2" : "RESET";
  debugf("%s: Early PSCI via %s insn and %s with arguments: {%#zx, %#zx, %#zx}\n", ProgramName(),
         insn, cmd, gArchPhysInfo->psci_reset_registers[1], gArchPhysInfo->psci_reset_registers[2],
         gArchPhysInfo->psci_reset_registers[3]);
}
