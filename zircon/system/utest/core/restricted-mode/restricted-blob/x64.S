// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// Do something to all the registers so we can read the state on the way out.
.macro twiddle_registers
  inc  %rax
  inc  %rbx
  inc  %rcx
  inc  %rdx
  inc  %rsi
  inc  %rdi
  inc  %rbp
  inc  %rsp
  inc  %r8
  inc  %r9
  inc  %r10
  inc  %r11
  inc  %r12
  inc  %r13
  inc  %r14
  inc  %r15

  // Write rcx to the pointer stored in fs base and write r11 to the pointer
  // stored in gs base. We must do this because both of these registers are
  // trashed by the syscall. This also tests that fs and gs base are set to
  // valid pointer values.
  mov   %rcx, %fs:0
  mov   %r11, %gs:0

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
// The local symbol is used by the jmp instruction below to return the PC back
// to syscall_bounce. The exported symbol is used by the test code to read the
// address of the symbol and set the PC in the restricted state VMO.
.function syscall_bounce, export
.Lsyscall_bounce:
  twiddle_registers
  syscall
.label syscall_bounce_post_syscall, export
  jmp .Lsyscall_bounce
.end_function

.function exception_bounce, export
  twiddle_registers
.label exception_bounce_exception_address, export
.Lexception_bounce_exception_address:
  ud2
  jmp .Lexception_bounce_exception_address
.end_function

// Stores 1 to *rdi in a loop.
.function store_one, export
  inc %rsi
.Lstore_one_loop:
  movl     $1, (%rdi)
  jmp     .Lstore_one_loop
.end_function

// Atomically adds 1 to *rdi, then loops until *rsi is nonzero and then issues a syscall.
.function wait_then_syscall, export
  lock incl    (%rdi)
.Lwait_then_syscall_loop:
  movl    (%rsi), %eax
  testl   %eax, %eax
  je      .Lwait_then_syscall_loop
  syscall
  ud2  // Should never be reached
.end_function
