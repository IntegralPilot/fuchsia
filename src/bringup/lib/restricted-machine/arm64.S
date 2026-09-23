// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// This system call is not yet in the stable vdso.
.weak zx_restricted_enter

.function restricted_exit, global
  // Back from restricted mode
  // x0 holds the context, which is the stack pointer
  // x1 holds the reason code

  // Restore the stack pointer at the point of the restricted enter wrapper.
  mov  sp,x0

  // Restore the FPU register pointer
  ldr x0, [sp, #16*7 + 8]

  // Save the FPU register state if we were provided a non-null FPU register pointer
  cbz x0, .after_store_fpu_registers
  bl store_fpu_registers
.after_store_fpu_registers:

  // Load the frame pointer and return address from the wrapper.
  .epilogue.fp

  // Restore the callee saved regs
  ldp x28, x27, [sp, #16*0]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x28
  .cfi_same_value x27
  ldp x26, x25, [sp, #16*1]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x26
  .cfi_same_value x25
  ldp x24, x23, [sp, #16*2]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x24
  .cfi_same_value x23
  ldp x22, x21, [sp, #16*3]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x22
  .cfi_same_value x21
  ldp x20, x19, [sp, #16*4]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x20
  .cfi_same_value x19

  // Restore the reason code pointer and shadow call stack pointer from the stack.
  ldp x2, x18, [sp, #16*5]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x18
  .cfi_same_value x2

  // Pop off all the now-restored state.
  add sp, sp, #16*7

  // Return the reason code from this function by setting the reason code pointer.
  str  x1, [x2]

  // Return back to whatever the address was in the link register.
  // Make it appear as if the wrapper had returned ZX_OK
  mov  x0, xzr
  ret
.end_function

.function restricted_enter_wrapper, global
  // Args 0 - 1 are already in place in X0 and X1.

  // Make space on the stack for the 14 registers saved below.
  sub sp, sp, #16*7

  // Save pointer to the FPU register state and x0
  stp x0, x3, [sp, #16*6]
  .cfi_adjust_cfa_offset 16
  .cfi_rel_offset x0, 0
  .cfi_rel_offset x3, 8

  // Save the reason code pointer and shadow call stack pointer on the stack.
  stp x2, x18, [sp, #16*5]
  .cfi_adjust_cfa_offset 16
  .cfi_rel_offset x2, 0
  .cfi_rel_offset x18, 8

  // Save the callee saved regs since the return from restricted mode
  // will modify all registers.
  stp x20, x19, [sp, #16*4]
  .cfi_adjust_cfa_offset 16
  .cfi_rel_offset x20, 0
  .cfi_rel_offset x19, 8
  stp x22, x21, [sp, #16*3]
  .cfi_adjust_cfa_offset 16
  .cfi_rel_offset x22, 0
  .cfi_rel_offset x21, 8
  stp x24, x23, [sp, #16*2]
  .cfi_adjust_cfa_offset 16
  .cfi_rel_offset x24, 0
  .cfi_rel_offset x23, 8
  stp x26, x25, [sp, #16*1]
  .cfi_adjust_cfa_offset 16
  .cfi_rel_offset x26, 0
  .cfi_rel_offset x25, 8
  stp x28, x27, [sp, #16*0]
  .cfi_adjust_cfa_offset 16
  .cfi_rel_offset x28, 0
  .cfi_rel_offset x27, 8

  .prologue.fp

  // If requested, load FPU register state from [x3]
  cbz x3, .after_load_fpu_registers
  mov x0, x3
  bl load_fpu_registers
  // Restore original x0
  ldr x0, [sp, #16*7]
.after_load_fpu_registers:

  // Pass the stack pointer as the context argument to the syscall.
  mov x2, sp

  bl zx_restricted_enter
  // if we got here it must have failed
  // Restore the registers that we saved so we can return to the caller.

  // Restore the frame pointer and return address from the stack.
  .epilogue.fp

  // Restore the callee saved regs
  ldp x28, x27, [sp, #16*0]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x28
  .cfi_same_value x27
  ldp x26, x25, [sp, #16*1]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x26
  .cfi_same_value x25
  ldp x24, x23, [sp, #16*2]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x24
  .cfi_same_value x23
  ldp x22, x21, [sp, #16*3]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x22
  .cfi_same_value x21
  ldp x20, x19, [sp, #16*4]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x20
  .cfi_same_value x19

  // Restore the reason code pointer and shadow call stack pointer from the stack.
  ldp x2, x18, [sp, #16*5]
  .cfi_adjust_cfa_offset -16
  .cfi_same_value x18
  .cfi_same_value x2

  // Pop all the callee saved registers off the stack.
  add sp, sp, #16*7

  ret
.end_function

// Load the contents of the array in *x0 to the FPU.
.function load_fpu_registers, global
  ldp q0, q1, [x0]
  ldp q2, q3, [x0, #32]
  ldp q4, q5, [x0, #64]
  ldp q6, q7, [x0, #96]
  ldp q8, q9, [x0, #128]
  ldp q10, q11, [x0, #160]
  ldp q12, q13, [x0, #192]
  ldp q14, q15, [x0, #224]
  ldp q16, q17, [x0, #256]
  ldp q18, q19, [x0, #288]
  ldp q20, q21, [x0, #320]
  ldp q22, q23, [x0, #352]
  ldp q24, q25, [x0, #384]
  ldp q26, q27, [x0, #416]
  ldp q28, q29, [x0, #448]
  ldp q30, q31, [x0, #480]
  ret
.end_function

// Store the contents of the FPU into the array in *x0.
.function store_fpu_registers, global
  stp q0, q1, [x0]
  stp q2, q3, [x0, #32]
  stp q4, q5, [x0, #64]
  stp q6, q7, [x0, #96]
  stp q8, q9, [x0, #128]
  stp q10, q11, [x0, #160]
  stp q12, q13, [x0, #192]
  stp q14, q15, [x0, #224]
  stp q16, q17, [x0, #256]
  stp q18, q19, [x0, #288]
  stp q20, q21, [x0, #320]
  stp q22, q23, [x0, #352]
  stp q24, q25, [x0, #384]
  stp q26, q27, [x0, #416]
  stp q28, q29, [x0, #448]
  stp q30, q31, [x0, #480]
  ret
.end_function
