// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// This defines the entry point in assembly, such that it calls:
//   extern "C" StartLdResult StartLd(zx_handle_t bootstrap, const void* vdso);
//
// The arguments are those provided by zx_process_start.  The return value is
// two words: the user entry point PC, and then a third argument.  With the
// stack unwound back to initial state, control jumps to that entry point with
// the three argument registers in place: the same two arguments received at
// process start, and the third word returned by Startld.

.function _start, global

  // This can assume the sp is already aligned to 16 by the kernel.

#if defined(__aarch64__)

  // The incoming arguments are preserved for the user entry point.
  .cfi_same_value x0
  .cfi_same_value x1

  // Provide a complete FP backtrace in case of crashes in the dynamic linker.
  .prologue.fp 16

  // Save the incoming arguments to pass them again to the user entry point.
  // StartLd still gets these in the original two argument registers.  These
  // could just go into call-saved registers, which are all zero anyway at
  // process startup.  But LdStartupInProcessTests::Run() expects to be able to
  // call this entry point as if it's calling the user entry point as a normal
  // function, where call-saved registers must not be clobbered.
  stp x0, x1, [sp, #16]  // The first two words are the FP, LR pair.
  .cfi_rel_offset x0, 16 // These are above those two, further from the SP.
  .cfi_rel_offset x1, 24

  bl StartLd

  mov x16, x0  // User entry point returned in x0; stash this in x16.
  mov x2, x1   // Third user argument returned in x1.

  // Restore the original first and second user arguments.
  ldp x0, x1, [sp, #16]
  .cfi_same_value x0
  .cfi_same_value x1

  // This restores the incoming FP and x30 (return address), both usually zero.
  .epilogue.fp 16

  // With stack unwound and arguments in place, jump to the user entry point.
  br x16

#elif defined(__riscv)

  // The incoming arguments are preserved for the user entry point.
  .cfi_same_value a0
  .cfi_same_value a1

  // Provide a complete FP backtrace in case of crashes in the dynamic linker.
  // Save the incoming arguments to pass them again to the user entry point.
  // StartLd still gets these in the original two argument registers.
  .prologue.fp 16
  sd a0, 0(sp)
  .cfi_rel_offset a0, 0
  sd a1, 8(sp)
  .cfi_rel_offset a1, 8

  call StartLd

  mv t1, a0  // User entry point returned in a0; stash this in t1.
  mv a2, a1  // Third user argument returned in a1.

  // Restore the original first and second user arguments.
  ld a0, 0(sp)
  .cfi_same_value a0
  ld a1, 8(sp)
  .cfi_same_value a1

  // This restores the incoming fp and ra, both usually zero.
  .epilogue.fp 16

  // With stack unwound and arguments in place, jump to the user entry point.
  jr t1

#elif defined(__x86_64__)

  // The incoming arguments are preserved for the user entry point.
  .cfi_same_value %rdi
  .cfi_same_value %rsi

  // Provide a complete FP backtrace in case of crashes in the dynamic linker.
  .prologue.fp frame_extra_size_for_metadata=16  // Two more pushes below.

  // Save the incoming arguments to pass them again to the user entry point.
  // StartLd still gets these in the original two argument registers.
  push %rdi
  .cfi_adjust_cfa_offset 8
  .cfi_rel_offset %rdi, 0
  push %rsi
  .cfi_adjust_cfa_offset 8
  .cfi_rel_offset %rsi, 0

  call StartLd

  // The user entry point was returned in %rax.  Restore the original first and
  // second user arguments.  The third user argument was returned in %rdx, so
  // it's already where it needs to be.
  pop %rsi
  .cfi_adjust_cfa_offset -8
  .cfi_same_value %rsi
  pop %rdi
  .cfi_adjust_cfa_offset -8
  .cfi_same_value %rdi

  // This restores the incoming FP, usually zero.
  .epilogue.fp

  // With stack unwound and arguments in place, jump to the user entry point.
  jmp *%rax

#else
#error "unsupported machine"
#endif

.end_function
