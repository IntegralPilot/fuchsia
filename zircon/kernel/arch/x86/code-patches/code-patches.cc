// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/x86/boot-cpuid.h>
#include <lib/arch/x86/bug.h>
#include <lib/boot-options/boot-options.h>
#include <lib/code-patching/code-patches.h>
#include <zircon/assert.h>

#include <arch/x86/cstring/selection.h>
#include <arch/x86/retpoline/selection.h>
#include <arch/x86/user-copy/selection.h>
#include <hwreg/x86msr.h>

// Declared in <lib/code-patching/code-patches.h>.
bool ArchPatchCode(code_patching::Patcher& patcher, const ArchPatchInfo& patch_info,
                   ktl::span<ktl::byte> insns, CodePatchId case_id,
                   fit::inline_function<void(ktl::initializer_list<ktl::string_view>)> print) {
  arch::BootCpuidIo cpuid;
  hwreg::X86MsrIo msr;

  auto do_alternative = [&patcher, insns, &print](ktl::string_view name,
                                                  ktl::string_view alternative) {
    patcher.MandatoryPatchWithAlternative(insns, alternative);
    print({"using ", name, " alternative \"", alternative, "\""});
    return true;
  };

  switch (case_id) {
    case CodePatchId::kSelfTest:
      patcher.NopFill(insns);
      print({"'smoke test' trap patched"});
      return true;
    case CodePatchId::kSwapgsMitigation: {
      // `nop` out the mitigation if the bug is not present, if we could not
      // mitigate it even if it was, or if we generally want mitigations off.
      const bool present = arch::HasX86SwapgsBug(cpuid);
      if (!present || BootOptions::Get()->x86_disable_spec_mitigations) {
        patcher.NopFill(insns);
        ktl::string_view qualifier = !present ? "bug not present"sv : "all mitigations disabled"sv;
        print({"swapgs bug mitigation disabled (", qualifier, ")"});
      } else {
        print({"swapgs bug mitigation enabled"});
      }
      return true;
    }
    case CodePatchId::kMdsTaaMitigation: {
      // `nop` out the mitigation if the bug is not present, if we could not
      // mitigate it even if it was, or if we generally want mitigations off.
      const bool present = arch::HasX86MdsTaaBugs(cpuid, msr);
      const bool can_mitigate = arch::CanMitigateX86MdsTaaBugs(cpuid);
      if (!present || !can_mitigate || BootOptions::Get()->x86_disable_spec_mitigations) {
        patcher.NopFill(insns);
        ktl::string_view qualifier = !present        ? "bug not present"
                                     : !can_mitigate ? "unable to mitigate"
                                                     : "all mitigations disabled";
        print({"MDS/TAA bug mitigation disabled (", qualifier, ")"});
      } else {
        print({"MDS/TAA bug mitigation enabled"});
      }
      return true;
    }
    case CodePatchId::k_X86CopyToOrFromUser:
      return do_alternative("user-copy", SelectX86UserCopyAlternative(cpuid));
    case CodePatchId::k__X86IndirectThunkR11:
      return do_alternative("retpoline",
                            SelectX86RetpolineAlternative(cpuid, msr, *BootOptions::Get()));
    case CodePatchId::k__UnsanitizedMemcpy:
      return do_alternative("memcpy", SelectX86MemcpyAlternative(cpuid));
    case CodePatchId::k__UnsanitizedMemset:
      return do_alternative("memset", SelectX86MemsetAlternative(cpuid));
  }

  return false;
}
