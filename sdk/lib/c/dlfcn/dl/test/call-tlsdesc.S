// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "call-tlsdesc.h"

#include <lib/arch/asm.h>

// ptrdiff_t CallTlsdesc(const TlsDescGot& got, Regs& expected, Regs& actual);
//
// This is a testing wrapper around calling TLSDESC entry points.  It's
// called with the normal C(++) calling convention for that signature.
//
// The TlsDescGot argument is used both to find the TLSDESC entry point (in
// its first slot), and as its argument (the TlsDescGot pointer; the second
// slot is what's used as the "value").
//
// The Regs type is a simple array of REGS_COUNT register-size integers,
// see call-tlsdesc.h for the indices.  The Regs arrays represent the
// call-used registers other than the TLSDESC argument / return-value
// register itself, plus the key fixed registers that are likely to be
// modified mistakenly.  They don't cover all the call-saved registers that
// any normal C function would be expected to preserve, since it's easier
// to be confident by inspection that the TLSDESC entry point code won't
// touch them.
//
// The caller must fill in most of the expected Regs with canary values.
// Only the fixed registers (SP, etc.) whose values shouldn't be set to
// arbitrary things are filled in on entry here to their local values after
// prologue setup.  After the TLSDESC entry point returns, the actual Regs
// are filled in from the state it left them in.  The return value is the
// TLSDESC entry point's return value.  Then the caller can check that the
// actual Regs match the expected Regs.

.function CallTlsdesc, global

#if defined(__aarch64__)

  // Prologue: make x19, x20 available as call-saved scratch.
  .prologue.shadow_call_sp
  .prologue.fp 16
  stp x19, x20, [sp, #16]
  .cfi_rel_offset x19, 16
  .cfi_rel_offset x20, 24

  // Save the expected_regs and actual_regs arguments in call-saved registers.
  mov x19, x1
  mov x20, x2

  // Store the reference x18, fp and sp values in the expected_regs.
  .macro store_fixed_regs reg
    stp x18, fp, [\reg, #8 * REGS_X(18)]
    mov x16, sp
    str x16, [\reg, #8 * REGS_SP]
  .endm
  store_fixed_regs x19

  // Load the call-used register values from the expected_regs.
  .macro on_call_used ldst, reg
    \ldst\()p x1, x2, [\reg, #8 * REGS_X(1)]
    \ldst\()p x3, x4, [\reg, #8 * REGS_X(3)]
    \ldst\()p x5, x6, [\reg, #8 * REGS_X(5)]
    \ldst\()p x7, x8, [\reg, #8 * REGS_X(7)]
    \ldst\()p x9, x10, [\reg, #8 * REGS_X(9)]
    \ldst\()p x11, x12, [\reg, #8 * REGS_X(11)]
    \ldst\()p x13, x14, [\reg, #8 * REGS_X(13)]
    \ldst\()p x15, x16, [\reg, #8 * REGS_X(15)]
    \ldst\()r x17, [\reg, #8 * REGS_X(17)]
  .endm
  on_call_used ld, x19

  // Call the TLSDESC function.
  ldr x30, [x0]
  blr x30

  // Presume x20 wasn't touched since it's call-saved in the normal ABI.
  // It's the actual_regs argument, so store the observed values there.
  on_call_used st, x20
  store_fixed_regs x20

  // Epilogue.
  ldp x19, x20, [sp, #16]
  .cfi_same_value x19
  .cfi_same_value x20
  .epilogue.fp 16
  .epilogue.shadow_call_sp
  ret

#elif defined(__arm__)

  // Prologue: make r4 available as call-saved scratch.
  push {r4, fp, lr}
  .save {r4, fp, lr}
  .cfi_adjust_cfa_offset 12
  .cfi_rel_offset r4, 0
  .cfi_rel_offset fp, 4
  .cfi_rel_offset lr, 8
  mov fp, sp

  // Save the actual_regs argument pointer in r4 (call-saved).
  mov r4, r2

  // Store the reference fp and sp values in the expected_regs.
  str fp, [r1, #4 * REGS_FP]
  str sp, [r1, #4 * REGS_SP]

  // Load the call-used register values from the expected_regs.
  ldm r1, {r1, r2, r3, r12}

  // Call the TLSDESC function.
  ldr lr, [r0]
  blx lr

  // Presume r4 wasn't touched since it's call-saved in the normal ABI.
  // It's the actual_regs argument, so store the observed values there.
  stm r4, {r1, r2, r3, r12, fp, sp}

  // Epilogue.
  pop {r4, fp, lr}
  .cfi_adjust_cfa_offset -12
  .cfi_same_value r4
  .cfi_same_value fp
  .cfi_same_value lr
  bx lr

#elif defined(__riscv)

  // Prologue: make s1 and s2 available as call-saved scratch.
  .prologue.shadow_call_sp
  .prologue.fp 16
  sd s1, 0(sp)
  .cfi_rel_offset s1, 0
  sd s2, 8(sp)
  .cfi_rel_offset s2, 8

  // Save the argument pointers in s1 and s2 (call-saved).
  mv s1, a1  // expected_regs
  mv s2, a2  // actual_regs

  // Store the reference ra, fp, sp, and gp values in the expected_regs.
  .macro store_fixed_regs reg
    sd ra, 8 * REGS_RA(\reg)
    sd fp, 8 * REGS_FP(\reg)
    sd sp, 8 * REGS_SP(\reg)
    sd gp, 8 * REGS_GP(\reg)
  .endm
  store_fixed_regs s1

  // Load the call-used register values from the expected_regs.
  .macro on_call_used op, reg
    \op t1, 8 * REGS_T(1)(\reg)
    \op t2, 8 * REGS_T(2)(\reg)
    \op t3, 8 * REGS_T(3)(\reg)
    \op t4, 8 * REGS_T(4)(\reg)
    \op t5, 8 * REGS_T(5)(\reg)
    \op t6, 8 * REGS_T(6)(\reg)
    \op a1, 8 * REGS_A(1)(\reg)
    \op a2, 8 * REGS_A(2)(\reg)
    \op a3, 8 * REGS_A(3)(\reg)
    \op a4, 8 * REGS_A(4)(\reg)
    \op a5, 8 * REGS_A(5)(\reg)
    \op a6, 8 * REGS_A(6)(\reg)
    \op a7, 8 * REGS_A(7)(\reg)
  .endm
  on_call_used ld, s1

  // Call the TLSDESC function.
  ld t0, (a0)
  jalr t0, t0

  // Presume s2 wasn't touched since it's call-saved in the normal ABI.
  // It's the actual_regs argument, so store the observed values there.
  on_call_used sd, s2
  store_fixed_regs s2

  // Epilogue.
  ld s1, 0(sp)
  .cfi_same_value s1
  ld s2, 8(sp)
  .cfi_same_value s2
  .epilogue.fp 16
  .epilogue.shadow_call_sp
  ret

#elif defined(__x86_64__)

  .prologue.fp
  push.spill %r12
  push.spill %r13

  // The TLSDESC argument goes into %rax, not the usual argument register.
  mov %rdi, %rax

  // Save the actual_regs argument pointer in %r12 (call-saved).
  mov %rdx, %r12

  // Store the reference fp and sp values in the expected_regs.
  mov %rsp, 8 * 8(%rsi)
  mov %rbp, 9 * 8(%rsi)

  // Load the call-used register values from the expected_regs.
  mov 8 * REGS_RCX(%rsi), %rcx
  mov 8 * REGS_RDX(%rsi), %rdx
  mov 8 * REGS_RDI(%rsi), %rdi
  mov 8 * REGS_R8(%rsi), %r8
  mov 8 * REGS_R9(%rsi), %r9
  mov 8 * REGS_R10(%rsi), %r10
  mov 8 * REGS_R11(%rsi), %r11
  mov 8 * REGS_RSI(%rsi), %rsi

  // Call the TLSDESC function.
  call *(%rax)

  // Presume %r12 wasn't touched since it's call-saved in the normal ABI.
  // It's the actual_regs argument, so store the observed values there.
  mov %rcx, 8 * REGS_RCX(%r12)
  mov %rdx, 8 * REGS_RDX(%r12)
  mov %rdi, 8 * REGS_RDI(%r12)
  mov %rsi, 8 * REGS_RSI(%r12)
  mov %r8, 8 * REGS_R8(%r12)
  mov %r9, 8 * REGS_R9(%r12)
  mov %r10, 8 * REGS_R10(%r12)
  mov %r11, 8 * REGS_R11(%r12)
  mov %rsp, 8 * REGS_RSP(%r12)
  mov %rbp, 8 * REGS_RBP(%r12)

  pop.reload %r13
  pop.reload %r12
  .epilogue.fp
  ret

#else

#error "what machine?"

#endif

.end_function
