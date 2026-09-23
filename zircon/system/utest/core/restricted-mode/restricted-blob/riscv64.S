// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// Do something to all the registers so we can read the state on the way out.
.macro twiddle_registers
  addi ra, ra, 1
  addi sp, sp, 1
  addi gp, gp, 1
  // Skip TP as we want to write to TLS later.
  addi t0, t0, 1
  addi t1, t1, 1
  addi t2, t2, 1
  addi s0, s0, 1
  addi s1, s1, 1
  addi a0, a0, 1
  addi a1, a1, 1
  addi a2, a2, 1
  addi a3, a3, 1
  addi a4, a4, 1
  addi a5, a5, 1
  addi a6, a6, 1
  addi a7, a7, 1
  addi s2, s2, 1
  addi s3, s3, 1
  addi s4, s4, 1
  addi s5, s5, 1
  addi s6, s6, 1
  addi s7, s7, 1
  addi s8, s8, 1
  addi s9, s9, 1
  addi s10, s10, 1
  addi s11, s11, 1
  addi t3, t3, 1
  addi t4, t4, 1
  addi t5, t5, 1
  addi t6, t6, 1

  // Save the contents of t0 to TLS prior to running a syscall.
  sd t0, (tp)
.endm

// This binary exports several of the symbols below for use in the restricted
// mode core-test found in ../restricted-mode.cc.
//
// 1. syscall_bounce: This symbol is the entry point to a function that
//    modifies all of the general registers in the system in a way that is
//    observable to the test harness. It then makes a syscall to return control
//    to normal mode.
// 2. syscall_bounce_post_syscall: This symbol is a label that points to the
//    statement after the syscall issued in syscall_bounce. It is used by the
//    test code to verify that the PC is in the right place after entering
//    restricted mode at syscall_bounce.
// 3. exception_bounce: This symbol is the entry point to a function that
//    modifies all general registers and then triggers an architectural
//    exception, which will return control to normal mode.
// 4. exception_bounce_exception_address: This symbol is a label that points
//    to the location at which the architectural exception will be triggered.
//    It is used by the test harness when twiddling the registers is not
//    necessary, such as when benchmarking in-thread exception handling.
// 5. store_one: This symbol points to a function that writes 1 to an address
//    stored in the first argument in an infinite loop. This is used by the
//    test code to verify that we can "kick" a thread out of restricted mode.
// 6. wait_then_syscall: This symbol points to a function that will wait until
//    a given pointer contains a nonzero value and then issue a syscall. This
//    is used by the test code to verify that kicking a thread right before it
//    makes a syscall correctly returns the restricted exit reason as
//    ZX_RESTRICTED_REASON_KICK.

// We must declare both an exported and a local symbol here.
// The local symbol is used by the j instruction below to return the PC back
// to syscall_bounce. The exported symbol is used by the test code to read the
// address of the symbol and set the PC in the restricted state VMO.
.function syscall_bounce, export
.Lsyscall_bounce:
  twiddle_registers
  mv t0, zero
  addi t0, t0, 64
  ecall
.label syscall_bounce_post_syscall, export
  j .Lsyscall_bounce
.end_function

.function exception_bounce, export
  twiddle_registers
.label exception_bounce_exception_address, export
.Lexception_bounce_exception_address:
  unimp
  j .Lexception_bounce_exception_address
.end_function

// Stores 1 to *a0 in a loop.
.function store_one, export
  add a1, a1, 1
  li t0, 1
.Lstore_one_loop:
  sw t0, (a0)
  j .Lstore_one_loop
.end_function

// Atomically adds 1 to *a0, then loops until *a1 is nonzero and then issues a syscall.
.function wait_then_syscall, export
  li t0, 1
  amoadd.w t1, t0, (a0)
.Lwait_then_syscall_loop:
  lw t0, (a1)
  beqz t0, .Lwait_then_syscall_loop
  ecall
  unimp // Should never be reached
.end_function
