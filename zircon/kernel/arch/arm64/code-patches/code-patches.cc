// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/code-patching/code-patches.h>
#include <zircon/assert.h>

#include <cstdint>

#include <arch/code-patches/case-id.h>
#include <phys/arch/arch-handoff.h>
#include <phys/arch/arch-phys-info.h>
#include <phys/symbolize.h>

#include "../smccc.h"

// Declared in <lib/code-patching/code-patches.h>.
bool ArchPatchCode(code_patching::Patcher& patcher, const ArchPatchInfo& info,
                   ktl::span<ktl::byte> insns, CodePatchId case_id,
                   fit::inline_function<void(ktl::initializer_list<ktl::string_view>)> print) {
  switch (case_id) {
    case CodePatchId::kSelfTest:
      patcher.NopFill(insns);
      print({"'smoke test' trap patched"});
      return true;

    case CodePatchId::kSmcccConduit: {
      // This patches in the smccc_conduit macro defined in ../smccc.h.
      ZX_ASSERT(insns.size_bytes() == sizeof(arch::ArmSmcccConduit));
      auto& insn = *reinterpret_cast<arch::ArmSmcccConduit*>(insns.data());
      ZX_ASSERT(insn == arch::ArmSmcccConduit::kSmc);
      if (gArchPhysInfo->smccc_use_hvc) {
        insn = arch::ArmSmcccConduit::kHvc;
      }
      print({
          "SMCCC conduit patched to use "sv,
          gArchPhysInfo->smccc_use_hvc ? "HVC"sv : "SMC"sv,
      });
      return true;
    }

    case CodePatchId::kSmcccWorkaroundFunction: {
      // This patches in the smccc_workaround_function_w0 macro defined in
      // ../smccc.h, used in in the alternate vector table in ../exceptions.S.
      // The same SMCCC functions are available on all CPUs and the best one
      // available is determined by ArchPreparePatchInfo.  arm64_select_vbar in
      // arch.cc will determine whether a specific CPU needs to use the SMCCC
      // workaround at all.
      ZX_ASSERT(insns.size_bytes() == sizeof(uint32_t));
      uint32_t& insn = *reinterpret_cast<uint32_t*>(insns.data());
      ktl::string_view choice = "unused";
      switch (info.alternate_vbar) {
        case Arm64AlternateVbar::kArchWorkaround3:
          choice = "SMCCC_ARCH_WORKAROUND_3"sv;
          insn = kMovW0SmcccArchWorkaround3;
          break;
        case Arm64AlternateVbar::kArchWorkaround1:
          choice = "SMCCC_ARCH_WORKAROUND_1"sv;
          insn = kMovW0SmcccArchWorkaround1;
          break;
        case Arm64AlternateVbar::kPsciVersion:
          choice = "SMCCC 1.1 PSCI_VERSION"sv;
          insn = kMovW0PsciVersion;
          break;
        case Arm64AlternateVbar::kSmccc10:
          choice = "SMCCC 1.0 PSCI_VERSION"sv;
          insn = kMovW0PsciVersion;
          break;
        case Arm64AlternateVbar::kNone:
          // The code won't actually be used.  To ensure that, the unpatched
          // instruction is a fatal trap.
          break;
        case Arm64AlternateVbar::kAuto:
          ZX_PANIC("should have been decided already");
          break;
      }
      print({"CPU workaround SMCCC function is "sv, choice});
      return true;
    }
  }
  return false;
}
