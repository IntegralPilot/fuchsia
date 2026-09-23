// Copyright 2021 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/x86/msr.h>
#include <lib/code-patching/code-patches.h>
#include <lib/zbi-format/graphics.h>
#include <lib/zbi-format/zbi.h>
#include <zircon/assert.h>

#include <ktl/byte.h>
#include <ktl/span.h>
#include <phys/arch/arch-handoff.h>
#include <phys/handoff.h>

#include "handoff-prep.h"

#include <ktl/enforce.h>

ArchPatchInfo ArchPreparePatchInfo() { return {}; }

void HandoffPrep::ArchSummarizeMiscZbiItem(const zbi_header_t& header,
                                           ktl::span<const ktl::byte> payload) {
  switch (header.type) {
    case ZBI_TYPE_FRAMEBUFFER:
      SaveForMexec(header, payload);
      break;
  }
}

void HandoffPrep::ArchConstructKernelAddressSpace() {}

void HandoffPrep::ArchDoHandoff(ZirconAbi abi, const ArchPatchInfo& patch_info) {
  ZX_DEBUG_ASSERT_MSG(!abi.shadow_call_stack_base, "Shadow call stack not supported on x86");

  uint32_t gsbase_low = static_cast<uint32_t>(abi.thread_abi_pointer);
  uint32_t gsbase_high = static_cast<uint32_t>(abi.thread_abi_pointer >> 32);

  __asm__ volatile(
      // We want the kernel's main to be at the root of the call stack, so
      // clear the frame pointer.
      "xor %%ebp, %%ebp\n"

      "mov %[rsp], %%rsp\n"

      // %rax, %rcx, %rdx prepopulated for a write of %gs.base
      "wrmsr\n"

      // The kernel's C++ entrypoint is allowed to assume that it's in the cld
      // state.
      "cld\n"

      "jmpq *%[entry]"
      :
      // Prepare %rax, %rcx, %rdx for the wrmsr.
      : "c"(arch::X86Msr::IA32_GS_BASE),      // "c" -> %rcx
        "a"(gsbase_low),                      // "a" -> %rax
        "d"(gsbase_high),                     // "d" -> %rdx
        [entry] "r"(kernel_virtual_entry()),  //
        "D"(handoff_),                        // "D" -> %rdi
        [rsp] "r"(abi.machine_stack_top),     //
        "m"(*handoff_)  // Ensures no store to the handoff can be regarded as dead
  );
  __UNREACHABLE;
}
