// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch/asm_macros.h>
#include <dev/psci.h>
#include <lib/arch/asm.h>

// TODO(https://fxbug.dev/417553411): See about saving/restoring other
// architectural state including, sctlr2_el1 and tcr2_el1.

// int64_t psci_do_suspend(psci_call_proc psci_call_routine,
//                         uint32_t power_state,
//                         paddr_t entry,
//                         psci_cpu_resume_context* context);
.function psci_do_suspend, global
  // x0 - psci_call_routine
  // w1 - power_state
  // x2 - entry
  // x3 - context
  mrs x4, daif
  mrs x5, sctlr_el1
  mrs x6, tcr_el1
  mrs x7, vbar_el1
  mrs x8, mdscr_el1
  mrs x9, cpacr_el1
  mrs x10, osdlr_el1
  mov x11, sp
  mrs x12, sp_el0
  mrs x13, contextidr_el1
  mrs x14, tpidr_el1
  mrs x15, tpidrro_el0
  mrs x16, tpidr_el0

  // Note, we're not saving ttbr0_el1 or ttbr1_el1 because we are relying on the
  // caller of |psci_do_resume| to restore those from global variables.
  stp x4, x5, [x3, (0 * 16)]
  stp x6, x7, [x3, (1 * 16)]
  stp x8, x9, [x3, (2 * 16)]
  stp x10, x11, [x3, (3 * 16)]
  stp x12, x13, [x3, (4 * 16)]
  stp x14, x15, [x3, (5 * 16)]
  stp x16, xzr, [x3, (6 * 16)]

  stp x18, x19, [x3, (7 * 16)]
  stp x20, x21, [x3, (8 * 16)]
  stp x22, x23, [x3, (9 * 16)]
  stp x24, x25, [x3, (10 * 16)]
  stp x26, x27, [x3, (11 * 16)]
  stp x28, x29, [x3, (12 * 16)]
  stp lr, xzr, [x3, (13 * 16)]

  // We're about to call psci_call_routine(PSCI64_CPU_SUSPEND, power_state,
  // entry, context), which will either return or branch to |entry|.  Before
  // issuing the call, stash the lr on the stack so that we can restore it after
  // the PSCI call returns and then return to *our* caller.
  mov x5, x0
  movlit w0, PSCI64_CPU_SUSPEND
  push_regs lr, xzr
  blr x5
  pop_regs lr, xzr
  ret
.end_function

// void psci_do_resume(psci_cpu_resume_context* context);
//
// Caller is expected to have enabled caches and mmu (including setting up
// ttbr0_el1, ttbr1_el1, mair_el1, tcr_el1), as well as spsel.
.function psci_do_resume, global
  // x0 - context

  ldp x4, x5, [x0, (0 * 16)]
  ldp x6, x7, [x0, (1 * 16)]
  ldp x8, x9, [x0, (2 * 16)]
  ldp x10, x11, [x0, (3 * 16)]
  ldp x12, x13, [x0, (4 * 16)]
  ldp x14, x15, [x0, (5 * 16)]
  ldp x16, xzr, [x0, (6 * 16)]

  ldp x18, x19, [x0, (7 * 16)]
  ldp x20, x21, [x0, (8 * 16)]
  ldp x22, x23, [x0, (9 * 16)]
  ldp x24, x25, [x0, (10 * 16)]
  ldp x26, x27, [x0, (11 * 16)]
  ldp x28, x29, [x0, (12 * 16)]
  ldp lr, xzr, [x0, (13 * 16)]

  msr tpidr_el0, x16
  msr tpidrro_el0, x15
  msr tpidr_el1, x14
  msr contextidr_el1, x13
  msr sp_el0, x12
  mov sp, x11
  msr osdlr_el1, x10
  msr cpacr_el1, x9
  msr mdscr_el1, x8

  msr vbar_el1, x7
  msr tcr_el1, x6
  isb
  msr sctlr_el1, x5
  isb
  msr daif, x4

  // Return 1 to indicate that we resumed from a "power down" state.
  mov x0, 1

  ret
.end_function
