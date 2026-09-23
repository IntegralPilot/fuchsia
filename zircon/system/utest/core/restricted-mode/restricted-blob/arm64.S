// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// Do something to all the registers so we can read the state on the way out.
.macro twiddle_registers
  add x0, x0, #1
  add x1, x1, #1
  add x2, x2, #1
  add x3, x3, #1
  add x4, x4, #1
  add x5, x5, #1
  add x6, x6, #1
  add x7, x7, #1
  add x8, x8, #1
  add x9, x9, #1
  add x10, x10, #1
  add x11, x11, #1
  add x12, x12, #1
  add x13, x13, #1
  add x14, x14, #1
  add x15, x15, #1
  add x16, x16, #1
  add x17, x17, #1
  add x18, x18, #1
  add x19, x19, #1
  add x20, x20, #1
  add x21, x21, #1
  add x22, x22, #1
  add x23, x23, #1
  add x24, x24, #1
  add x25, x25, #1
  add x26, x26, #1
  add x27, x27, #1
  add x28, x28, #1
  add x29, x29, #1
  add x30, x30, #1
  // Keep the SP 16-byte aligned, as required by the spec.
  add sp, sp, #16

  // Save the contents of x16 to TLS prior to running a syscall.
  mrs x0, tpidr_el0
  str x16, [x0]
.endm

.macro twiddle_fpu_registers
  shl v0.16b, v0.16b, #1
  shl v1.16b, v1.16b, #1
  shl v2.16b, v2.16b, #1
  shl v3.16b, v3.16b, #1
  shl v4.16b, v4.16b, #1
  shl v5.16b, v5.16b, #1
  shl v6.16b, v6.16b, #1
  shl v7.16b, v7.16b, #1
  shl v8.16b, v8.16b, #1
  shl v9.16b, v9.16b, #1
  shl v10.16b, v10.16b, #1
  shl v11.16b, v11.16b, #1
  shl v12.16b, v12.16b, #1
  shl v13.16b, v13.16b, #1
  shl v14.16b, v14.16b, #1
  shl v15.16b, v15.16b, #1
  shl v16.16b, v16.16b, #1
  shl v17.16b, v17.16b, #1
  shl v18.16b, v18.16b, #1
  shl v19.16b, v19.16b, #1
  shl v20.16b, v20.16b, #1
  shl v21.16b, v21.16b, #1
  shl v22.16b, v22.16b, #1
  shl v23.16b, v23.16b, #1
  shl v24.16b, v24.16b, #1
  shl v25.16b, v25.16b, #1
  shl v26.16b, v26.16b, #1
  shl v27.16b, v27.16b, #1
  shl v28.16b, v28.16b, #1
  shl v29.16b, v29.16b, #1
  shl v30.16b, v30.16b, #1
  shl v31.16b, v31.16b, #1
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
// The local symbol is used by the bl instruction below to return the PC back
// to syscall_bounce. The exported symbol is used by the test code to read the
// address of the symbol and set the PC in the restricted state VMO.
.function syscall_bounce, export
.Lsyscall_bounce:
  twiddle_registers
  twiddle_fpu_registers
  mov x16, xzr
  add x16, x16, #64
  svc #0
.label syscall_bounce_post_syscall, export
  bl .Lsyscall_bounce
.end_function

.function exception_bounce, export
  twiddle_registers
  twiddle_fpu_registers
.label exception_bounce_exception_address, export
.Lexception_bounce_exception_address:
  udf #0
  b .Lexception_bounce_exception_address
.end_function

// Stores 1 to *x0 in a loop.
.function store_one, export
  add     x1, x1, #1
  mov     w8, #1
.Lstore_one_loop:
  str     w8, [x0]
  b       .Lstore_one_loop
.end_function

// Atomically adds 1 to *x0, then loops until *x1 is nonzero and then issues a syscall.
// We need the local label to ensure that the cbnz below can jump back here when the
// atomic store fails without having to go through the PLT.
.function wait_then_syscall, export
.Lwait_then_syscall:
  ldaxr   w8, [x0]
  add     w8, w8, #1
  stlxr   w9, w8, [x0]
  cbnz    w9, .Lwait_then_syscall
.Lwait_then_syscall_loop:
  ldar    w8, [x1]
  cbz     w8, .Lwait_then_syscall_loop
  twiddle_fpu_registers
  svc     #0
  brk     #0x1
.end_function

.function bad_increment, export
  ldr x0, =0xfffffffffffffff8
  ldr x1, =0xaaaaaaaaaaaaaaa0
  ldr x2, =0xbbbbbbbbbbbbbbb0

  // Store a pair of registers with post-index addressing
  // x0 is the base register, #16 is the offset
  stp x1, x2, [x0], #16
  svc     #0
  brk     #0x1
.end_function
