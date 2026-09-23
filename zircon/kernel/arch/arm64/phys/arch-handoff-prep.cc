// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/boot-options/boot-options.h>
#include <lib/code-patching/code-patches.h>
#include <lib/zbi-format/driver-config.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/assert.h>

#include <ktl/byte.h>
#include <ktl/optional.h>
#include <ktl/span.h>
#include <ktl/variant.h>
#include <phys/arch/arch-handoff.h>
#include <phys/arch/arch-phys-info.h>
#include <phys/handoff.h>
#include <phys/main.h>
#include <phys/stdio.h>

#include "handoff-prep.h"
#include "smccc.h"

#include <ktl/enforce.h>

namespace {

// Auto-detection logic for what firmware features are supported:
// SMCCC_ARCH_FEATURES on the boot CPU (or any CPU) can tell us which functions
// the firmware knows about such that per-CPU SMCCC_ARCH_FEATURES queries
// indicate whether that CPU needs to use it.  Here in physboot we need to
// determine how to patch the firmware-using alternate vector implementations,
// and direct the kernel proper's arch.cc code to then decide which
// implementation to use for each CPU.
//
// TODO(https://fxbug.dev/322202704): This only detects SMCCC 1.1 firmware with
// workaround support and enables that.  In future, there should be some
// fallback logic to decide what to enable.  Possibly there should be some
// additional logic for non-firmware workarounds to be selected by some means.

// Policy when SMCCC / PSCI is explicitly disabled or the
// ZBI_KERNEL_DRIVER_ARM_PSCI item was missing.
constexpr Arm64AlternateVbar kNoSmccc = Arm64AlternateVbar::kNone;

// Policy when PSCI_FEATURES is not available to detect SMCCC_VERSION.
constexpr Arm64AlternateVbar kNoPsciFeatures = Arm64AlternateVbar::kNone;

// Policy when SMCCC_VERSION is not available to detect 1.0 vs 1.1 support.
constexpr Arm64AlternateVbar kNoSmcccVersion = Arm64AlternateVbar::kNone;

// Policy when SMCCC_ARCH_FEATURES is not available to detect each function.
constexpr Arm64AlternateVbar kNoSmcccArchFeatures = Arm64AlternateVbar::kNone;

// Policy when no SMCCC_ARCH_WORKAROUND_* function is available.
constexpr Arm64AlternateVbar kNoSmcccFunction = Arm64AlternateVbar::kNone;

bool SmcccAvailable(arch::ArmSmcccFunction function) {
  int32_t value = static_cast<int32_t>(
      ArmSmcccCall(arch::ArmSmcccFunction::kSmcccArchFeatures, static_cast<uint32_t>(function)));
  return value >= 0;
}

Arm64AlternateVbar AutoAlternateVbar() {
  if (gArchPhysInfo->smccc_disabled) {
    debugf("%s: SMCCC disabled, cannot query for SMCCC_ARCH_WORKAROUND functions\n", ProgramName());
    return kNoSmccc;
  }
  if (!gArchPhysInfo->have_psci_features) {
    debugf(
        "%s: SMCCC 1.1 not available,"
        " cannot query for SMCCC_ARCH_WORKAROUND functions\n",
        ProgramName());
    return kNoPsciFeatures;
  }

  // PSCI_FEATURES is available.  Use it to check for SMCCC_VERSION.
  if (arch::ArmSmcccGetReturnCode(
          ArmSmcccCall(arch::ArmSmcccFunction::kPsciPsciFeatures,
                       static_cast<uint32_t>(arch::ArmSmcccFunction::kSmcccVersion))) !=
      arch::kArmSmcccSuccess) {
    debugf(
        "%s: SMCCC_VERSION not available from PSCI_FEATURES,"
        " cannot query for SMCCC_ARCH_WORKAROUND functions\n",
        ProgramName());
    return kNoSmcccVersion;
  }

  // SMCCC_VERSION is available.  Use it to check for SMCCC_ARCH_FEATURES.
  arch::ArmSmcccVersion version =
      arch::ArmSmcccVersionResult(ArmSmcccCall(arch::ArmSmcccFunction::kSmcccVersion));
  if (version < arch::ArmSmcccVersion{1, 1}) {
    debugf("%s: SMCCC %" PRIu16 ".%" PRIu16
           " < 1.1 does not support SMCCC_ARCH_FEATURES,"
           " cannot query for SMCCC_ARCH_WORKAROUND functions\n",
           ProgramName(), version.major, version.minor);
    return kNoSmcccArchFeatures;
  }

  // SMCCC_ARCH_FEATURES is available.  Use it to check for workarounds.
  if (SmcccAvailable(arch::ArmSmcccFunction::kSmcccArchWorkaround3)) {
    debugf("%s: SMCCC %" PRIu16 ".%" PRIu16 " reports SMCCC_ARCH_WORKAROUND_3 (%#" PRIx32
           ") available\n",
           ProgramName(), version.major, version.minor,
           static_cast<uint32_t>(arch::ArmSmcccFunction::kSmcccArchWorkaround3));
    return Arm64AlternateVbar::kArchWorkaround3;
  }
  if (SmcccAvailable(arch::ArmSmcccFunction::kSmcccArchWorkaround1)) {
    debugf("%s: SMCCC %" PRIu16 ".%" PRIu16 " reports SMCCC_ARCH_WORKAROUND_1 (%#" PRIx32
           ") available\n",
           ProgramName(), version.major, version.minor,
           static_cast<uint32_t>(arch::ArmSmcccFunction::kSmcccArchWorkaround1));
    return Arm64AlternateVbar::kArchWorkaround1;
  }

  debugf("%s: SMCCC %" PRIu16 ".%" PRIu16 " reports no SMCCC_ARCH_WORKAROUND functions available\n",
         ProgramName(), version.major, version.minor);
  return kNoSmcccFunction;
}

Arm64AlternateVbar GetAlternateVbar() {
  if (BootOptions::Get()->arm64_alternate_vbar == Arm64AlternateVbar::kAuto) {
    return AutoAlternateVbar();
  }
  return BootOptions::Get()->arm64_alternate_vbar;
}

}  // namespace

ArchPatchInfo ArchPreparePatchInfo() {
  return ArchPatchInfo{
      .alternate_vbar = GetAlternateVbar(),
  };
}

void HandoffPrep::ArchSummarizeMiscZbiItem(const zbi_header_t& header,
                                           ktl::span<const ktl::byte> payload) {
  ZX_DEBUG_ASSERT(handoff_);
  ArchPhysHandoff& arch_handoff = handoff_->arch_handoff;

  switch (header.type) {
    case ZBI_TYPE_KERNEL_DRIVER: {
      switch (header.extra) {
        // TODO(https://fxbug.dev/42169136): Move me to userspace.
        case ZBI_KERNEL_DRIVER_AMLOGIC_HDCP:
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_amlogic_hdcp_driver_t));
          arch_handoff.amlogic_hdcp_driver =
              *reinterpret_cast<const zbi_dcfg_amlogic_hdcp_driver_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_AMLOGIC_RNG:
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_amlogic_rng_driver_t));
          arch_handoff.amlogic_rng_driver =
              *reinterpret_cast<const zbi_dcfg_amlogic_rng_driver_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER:
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_arm_generic_timer_driver_t));
          arch_handoff.generic_timer_driver =
              *reinterpret_cast<const zbi_dcfg_arm_generic_timer_driver_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_ARM_GENERIC_TIMER_MMIO:
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_arm_generic_timer_mmio_driver_t));
          arch_handoff.generic_timer_mmio_driver =
              *reinterpret_cast<const zbi_dcfg_arm_generic_timer_mmio_driver_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_ARM_GIC_V2:
          ZX_ASSERT_MSG(!arch_handoff.apple_aic3_driver, "Conflicting AIC and GIC records");
          // Defer to the newer hardware: v3 configs win out over v2.
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_arm_gic_v2_driver_t));
          if (!ktl::holds_alternative<zbi_dcfg_arm_gic_v3_driver_t>(arch_handoff.gic_driver)) {
            arch_handoff.gic_driver =
                *reinterpret_cast<const zbi_dcfg_arm_gic_v2_driver_t*>(payload.data());
          }
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_ARM_GIC_V3:
          ZX_ASSERT_MSG(!arch_handoff.apple_aic3_driver, "Conflicting AIC and GIC records");
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_arm_gic_v3_driver_t));
          arch_handoff.gic_driver =
              *reinterpret_cast<const zbi_dcfg_arm_gic_v3_driver_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_APPLE_AIC3:
#ifndef EXPERIMENTAL_APPLE
          ZX_PANIC("Apple AIC requires an experimental Apple kernel");
#endif
          ZX_ASSERT_MSG(ktl::holds_alternative<ktl::monostate>(arch_handoff.gic_driver) &&
                            !arch_handoff.apple_aic3_driver,
                        "Conflicting interrupt-controller records");
          ZX_ASSERT(payload.size() == sizeof(zbi_dcfg_apple_aic3_t));
          arch_handoff.apple_aic3_driver =
              *reinterpret_cast<const zbi_dcfg_apple_aic3_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_ARM_PSCI:
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_arm_psci_driver_t));
          arch_handoff.psci_driver =
              *reinterpret_cast<const zbi_dcfg_arm_psci_driver_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND: {
          fbl::AllocChecker ac;
          size_t num_ele = payload.size() / sizeof(zbi_dcfg_arm_psci_cpu_suspend_state_t);
          ZX_ASSERT_MSG(
              payload.size() % sizeof(zbi_dcfg_arm_psci_cpu_suspend_state_t) == 0,
              "Payload size of ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND was %zu, must be multiple of %zu",
              payload.size(), sizeof(zbi_dcfg_arm_psci_cpu_suspend_state_t));
          ktl::span buffer = New(arch_handoff.psci_cpu_suspend_driver, ac, num_ele);
          ZX_ASSERT_MSG(ac.check(),
                        "cannot allocate %zu bytes for ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND",
                        payload.size());
          memcpy(buffer.data(), payload.data(), payload.size_bytes());
        } break;
        case ZBI_KERNEL_DRIVER_GENERIC32_WATCHDOG:
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_generic32_watchdog_t));
          arch_handoff.generic32_watchdog_driver =
              *reinterpret_cast<const zbi_dcfg_generic32_watchdog_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_MOTMOT_POWER:
          // Ignore the payload and just use the presence of this node.
          arch_handoff.motmot_power_driver = true;
          break;
        case ZBI_KERNEL_DRIVER_MOONFLOWER_POWER:
          // Ignore the payload and just use the presence of this node.
          arch_handoff.moonflower_power_driver = true;
          break;
        case ZBI_KERNEL_DRIVER_IRIS_POWER: {
          fbl::AllocChecker ac;
          constexpr size_t elem_size = sizeof(zbi_cpu_energy_model_domain_t);
          const size_t num_ele = payload.size() / elem_size;
          ZX_ASSERT_MSG(
              payload.size() % elem_size == 0,
              "Payload size of ZBI_KERNEL_DRIVER_IRIS_POWER was %zu, must be multiple of %zu",
              payload.size(), elem_size);
          ktl::span buffer = New(arch_handoff.iris_power_driver, ac, num_ele);
          ZX_ASSERT_MSG(ac.check(), "cannot allocate %zu bytes for ZBI_KERNEL_DRIVER_IRIS_POWER",
                        payload.size());
          memcpy(buffer.data(), payload.data(), payload.size_bytes());
          SaveForMexec(header, payload);
        } break;
        case ZBI_KERNEL_DRIVER_QCOM_RNG:
          arch_handoff.qcom_rng_driver =
              *reinterpret_cast<const zbi_dcfg_qcom_rng_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_ARM_SMMU:
          fbl::AllocChecker ac;
          constexpr size_t elem_size = sizeof(zbi_dcfg_arm_smmu_driver_t);
          const size_t num_ele = payload.size() / elem_size;
          ZX_ASSERT_MSG(
              payload.size() % elem_size == 0,
              "Payload size of ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND was %zu, must be multiple of %zu",
              payload.size(), elem_size);
          ktl::span buffer = New(arch_handoff.arm_smmu_drivers, ac, num_ele);
          ZX_ASSERT_MSG(ac.check(),
                        "cannot allocate %zu bytes for ZBI_KERNEL_DRIVER_ARM_PSCI_CPU_SUSPEND",
                        payload.size());
          memcpy(buffer.data(), payload.data(), payload.size_bytes());
      }
      break;
    }
  }
}

void HandoffPrep::ArchConstructKernelAddressSpace() {}

void HandoffPrep::ArchDoHandoff(ZirconAbi abi, const ArchPatchInfo& patch_info) {
  ZX_DEBUG_ASSERT(handoff_);
  ArchPhysHandoff& arch_handoff = handoff_->arch_handoff;

  arch_handoff.alternate_vbar = patch_info.alternate_vbar;

  __asm__ volatile(
      // We want the kernel's main to be at the root of the call stack, so
      // clear the return address and frame pointer.
      "mov x29, xzr\n"
      "mov x30, xzr\n"

      "mov sp, %[sp]\n"
      "mov x18, %[shadow_call_sp]\n"
      "msr tpidr_el1, %[tp]\n"

      "mov x0, %[handoff]\n"
      "br %[entry]"
      :                                                    //
      : [entry] "r"(kernel_virtual_entry()),               //
        [handoff] "r"(handoff_),                           //
        [tp] "r"(abi.thread_abi_pointer),                  //
        [sp] "r"(abi.machine_stack_top),                   //
        [shadow_call_sp] "r"(abi.shadow_call_stack_base),  //
        "m"(*handoff_)  // Ensures no store to the handoff can be regarded as dead
      : "x0"            //
  );

  __UNREACHABLE;
}
