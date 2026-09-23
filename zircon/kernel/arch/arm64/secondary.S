// Copyright 2016 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch/arm64/asm.h>
#include <arch/arm64/mmu.h>
#include <lib/arch/asm.h>

#ifndef __has_feature
#define __has_feature(x) 0
#endif

//
// Register use:
//   x0-x3   Arguments
//   x9-x14  Scratch
//   x21-x28 Globals
//
tmp                     .req x9

page_table0             .req x21
page_table1             .req x22
kernel_vaddr            .req x23
resume_context          .req x24

// This code is purely position-independent and generates no relocations
// that need boot-time fixup.

// void arm64_secondary_resume_start(void* resume_context);
.function arm64_secondary_resume_start, global
  // Save the resume_context arg so we can later check whether this is a boot up
  // or a resume.
  mov resume_context, x0

  b .Lsecondary_start_common
.end_function

// void arm64_secondary_start(void* stack_pointer)
.function arm64_secondary_start, global
  // Make sure we dont take the resume path if we came in this way.
  mov resume_context, xzr

.Lsecondary_start_common:
  // If we entered at a higher EL than 1, drop to EL1.
  //
  // Make sure to clear the stack pointer first. The call will reuse in EL1
  // whatever is currently installed, but on a cold boot this might be
  // garbage and run afoul of EL1 sp alignment checks if blindly installed.
  mov tmp, xzr
  mov sp, tmp
  bl ArmDropToEl1WithoutEl2Monitor

  // Make sure no old exception handlers are installed.
  msr vbar_el1, xzr

  // Enable caches so atomics and spinlocks work
  mrs tmp, sctlr_el1
  and tmp, tmp, #~(1<<19) // Clear WXN
  orr tmp, tmp, #(1<<12) // Enable icache
  orr tmp, tmp, #(1<<2)  // Enable dcache/ucache
  msr sctlr_el1, tmp

  // The stored virtual address for the .Lmmu_on_vaddr was relocated at boot.
  // Load the virtual address to jump to when the MMU is ready.
  ldr_global kernel_vaddr, kMmuOnVaddr

  // Load the base of the translation tables.
  ldr_global page_table0, root_lower_page_table_phys
  ldr_global page_table1, root_kernel_page_table_phys

.Lmmu_enable:
  // Set up the mmu

  // Invalidate the entire TLB
  tlbi vmalle1is
  dsb sy
  isb

  // Initialize Memory Attribute Indirection Register
  //
  // In the case of the boot CPU, care is taken to ensure that currently
  // active attributes and indices are the same as those in MMU_MAIR_VAL.
  movlit tmp, MMU_MAIR_VAL
  msr mair_el1, tmp

  // Initialize TCR_EL1
  // set cacheable attributes on translation walk
  // (SMP extensions) non-shareable, inner write-back write-allocate
  // both aspaces active, current ASID in TTBR1
  movlit tmp, MMU_TCR_FLAGS_IDENT
  msr tcr_el1, tmp
  isb

  // Write the ttbrs with phys addr of the translation table
  msr ttbr0_el1, page_table0
  // Or in 0x1 (GLOBAL_ASID) bits. Keep in sync with mmu.h
  orr tmp, page_table1, #(0x1 << 48)
  msr ttbr1_el1, tmp
  isb

  // Read SCTLR
  mrs tmp, sctlr_el1

  // Turn on the MMU
  orr tmp, tmp, #(1<<0)

  // Write back SCTLR
  msr sctlr_el1, tmp
.Lmmu_on_pc:
  isb

  br kernel_vaddr

.Lmmu_on_vaddr:
  // Disable trampoline page-table in ttbr0
  movlit tmp, MMU_TCR_FLAGS_KERNEL
  msr tcr_el1, tmp
  isb

  // Invalidate the entire TLB
  tlbi vmalle1
  dsb sy
  isb

  // Before we set sp, ensure that it's 'linked' to the current SP_ELX and
  // not SP_EL0.
  msr SPSel, #1

  // Now that we've set up the most basic architectural state, it's time to
  // determine if we're booting up or resuming from suspend and finish the job.
  cbnz resume_context,.Lresume

  // We're booting up rather than resuming from suspend.
  // x0 points to the arm64_sp_info struct for this CPU, passed through PSCI_CPU_ON.
#if __has_feature(shadow_call_stack)
  ldr shadow_call_sp, [x0] // arm64_sp_info.shadow_call_sp
#endif
  add x1, x0, #32 // compute the top of the structure for TPIDR (see arch.cc)
  msr tpidr_el1, x1

  // SP will start at the bottom of this structure growing down.
  mov sp, x0

  bl arm64_secondary_entry
  b .Lunsupported_cpu_trap

.Lresume:
  // We're resuming.  The resume_context will have our stack pointer and
  // everything else we need.
  mov x0, resume_context
  bl psci_do_resume

.Lunsupported_cpu_trap:
  wfe
  b .Lunsupported_cpu_trap
.end_function

// This gets relocated to the virtual address before boot.  The secondary CPU
// that loads it will do so by its physical address to discover where to jump.
.object kMmuOnVaddr, relro, align=8
  .quad .Lmmu_on_vaddr
.end_object
