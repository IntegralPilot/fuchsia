// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/code-patching/code-patches.h>
#include <zircon/assert.h>

#include <phys/arch/arch-handoff.h>
#include <phys/arch/arch-phys-info.h>
#include <phys/main.h>

#include "handoff-prep.h"

#include <ktl/enforce.h>

ArchPatchInfo ArchPreparePatchInfo() { return {}; }

void HandoffPrep::ArchSummarizeMiscZbiItem(const zbi_header_t& header,
                                           ktl::span<const ktl::byte> payload) {
  ZX_DEBUG_ASSERT(handoff_);
  ArchPhysHandoff& arch_handoff = handoff_->arch_handoff;

  switch (header.type) {
    case ZBI_TYPE_KERNEL_DRIVER: {
      switch (header.extra) {
        case ZBI_KERNEL_DRIVER_RISCV_PLIC:
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_riscv_plic_driver_t));
          arch_handoff.plic_driver =
              *reinterpret_cast<const zbi_dcfg_riscv_plic_driver_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
        case ZBI_KERNEL_DRIVER_RISCV_GENERIC_TIMER:
          ZX_ASSERT(payload.size() >= sizeof(zbi_dcfg_riscv_generic_timer_driver_t));
          arch_handoff.generic_timer_driver =
              *reinterpret_cast<const zbi_dcfg_riscv_generic_timer_driver_t*>(payload.data());
          SaveForMexec(header, payload);
          break;
      }
      break;
    }
  }
}

void HandoffPrep::ArchConstructKernelAddressSpace() {}

void HandoffPrep::ArchDoHandoff(ZirconAbi abi, const ArchPatchInfo& patch_info) {
  ZX_DEBUG_ASSERT(handoff_);
  ArchPhysHandoff& arch_handoff = handoff_->arch_handoff;

  arch_handoff.boot_hart_id = gArchPhysInfo->boot_hart_id;
  arch_handoff.cpu_features = gArchPhysInfo->cpu_features;

  __asm__ volatile(
      // We want the kernel's main to be at the root of the call stack, so
      // clear the return address and frame pointer.
      "mv ra, zero\n"
      "mv s0, zero\n"

      "mv sp, %[sp]\n"
      "mv gp, %[shadow_call_sp]\n"  // Will clear the gp if shadow call stack is unsupported.
      "mv tp, %[tp]\n"

      "mv a0, %[handoff]\n"
      "jr %[entry]"
      :                                                    //
      : [entry] "r"(kernel_virtual_entry()),               //
        [handoff] "r"(handoff_),                           //
        [sp] "r"(abi.machine_stack_top),                   //
        [shadow_call_sp] "r"(abi.shadow_call_stack_base),  //
        [tp] "r"(abi.thread_abi_pointer),                  //
        "m"(*handoff_)  // Ensures no store to the handoff can be regarded as dead
      : "a0"            //
  );

  __UNREACHABLE;
}
