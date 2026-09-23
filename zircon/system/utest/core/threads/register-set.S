// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

#include "register-set-asm.h"


// If vector instructions are not already enabled, do so at the assembler level.
#if defined(__riscv) && !defined(__riscv_v)
.option arch, +v
#endif

.macro on_32 macro
  \macro 0
  \macro 1
  \macro 2
  \macro 3
  \macro 4
  \macro 5
  \macro 6
  \macro 7
  \macro 8
  \macro 9
  \macro 10
  \macro 11
  \macro 12
  \macro 13
  \macro 14
  \macro 15
  \macro 16
  \macro 17
  \macro 18
  \macro 19
  \macro 20
  \macro 21
  \macro 22
  \macro 23
  \macro 24
  \macro 25
  \macro 26
  \macro 27
  \macro 28
  \macro 29
  \macro 30
  \macro 31
.endm

.macro spin_body
#if defined(__aarch64__)
  b spin_address
#elif defined(__riscv)
  j spin_address
#elif defined(__x86_64__)
  jmp spin_address
#else
#error "what machine?"
#endif
.endm

.function spin_address, global
  spin_body
.end_function

.function spin_with_general_regs, global

#if defined(__x86_64__)

  // Set flags using POPF.  Note that we use POPF rather than SAHF
  // because POPF is able to set more flags than SAHF.
  pushq REGS_RFLAGS(%rdi)
  .cfi_adjust_cfa_offset 8
  popfq
  .cfi_adjust_cfa_offset -8

  // Load general purpose registers.
  mov REGS_RAX(%rdi), %rax
  mov REGS_RBX(%rdi), %rbx
  mov REGS_RCX(%rdi), %rcx
  mov REGS_RDX(%rdi), %rdx
  mov REGS_RSI(%rdi), %rsi
  // Skip assigning %rdi here and assign it last.
  mov REGS_RBP(%rdi), %rbp
  mov REGS_RSP(%rdi), %rsp
  mov REGS_R8(%rdi), %r8
  mov REGS_R9(%rdi), %r9
  mov REGS_R10(%rdi), %r10
  mov REGS_R11(%rdi), %r11
  mov REGS_R12(%rdi), %r12
  mov REGS_R13(%rdi), %r13
  mov REGS_R14(%rdi), %r14
  mov REGS_R15(%rdi), %r15
  mov REGS_RDI(%rdi), %rdi

  jmp spin_address

#elif defined(__aarch64__)

  // Load sp via a temporary register.
  ldr x1, [x0, #REGS_SP]
  mov sp, x1

  // Load NZCV flags, a subset of the PSTATE/CPSR register.
  ldr x1, [x0, #REGS_CPSR]
  msr nzcv, x1

  // Load general purpose registers.
  // Skip assigning x0 and x1 here and assign them last.
  ldp x2, x3, [x0, REGS_X(2)]
  ldp x4, x5, [x0, REGS_X(4)]
  ldp x6, x7, [x0, REGS_X(6)]
  ldp x8, x9, [x0, REGS_X(8)]
  ldp x10, x11, [x0, REGS_X(10)]
  ldp x12, x13, [x0, REGS_X(12)]
  ldp x14, x15, [x0, REGS_X(14)]
  ldp x16, x17, [x0, REGS_X(16)]
  ldp x18, x19, [x0, REGS_X(18)]
  ldp x20, x21, [x0, REGS_X(20)]
  ldp x22, x23, [x0, REGS_X(22)]
  ldp x24, x25, [x0, REGS_X(24)]
  ldp x26, x27, [x0, REGS_X(26)]
  ldp x28, x29, [x0, REGS_X(28)]
  ldr x30, [x0, REGS_X(30)]
  ldp x0, x1, [x0, REGS_X(0)]

  b spin_address

#elif defined(__riscv)

  ld ra, REGS_RA(a0)    // x1
  ld sp, REGS_SP(a0)    // x2
  ld gp, REGS_GP(a0)    // x3
  ld tp, REGS_TP(a0)    // x4
  // t0 is used for the jump.
  ld t1, REGS_T1(a0)    // x6
  ld t2, REGS_T2(a0)    // x7
  ld s0, REGS_S0(a0)    // x8
  ld s1, REGS_S1(a0)    // x9
  // The pointer is in a0, so load that last.
  ld a1, REGS_A1(a0)    // x11
  ld a2, REGS_A2(a0)    // x12
  ld a3, REGS_A3(a0)    // x13
  ld a4, REGS_A4(a0)    // x14
  ld a5, REGS_A5(a0)    // x15
  ld a6, REGS_A6(a0)    // x16
  ld a7, REGS_A7(a0)    // x17
  ld s2, REGS_S2(a0)    // x18
  ld s3, REGS_S3(a0)    // x19
  ld s4, REGS_S4(a0)    // x20
  ld s5, REGS_S5(a0)    // x21
  ld s6, REGS_S6(a0)    // x22
  ld s7, REGS_S7(a0)    // x23
  ld s8, REGS_S8(a0)    // x24
  ld s9, REGS_S9(a0)    // x25
  ld s10, REGS_S10(a0)  // x26
  ld s11, REGS_S11(a0)  // x27
  ld t3, REGS_T3(a0)    // x28
  ld t4, REGS_T4(a0)    // x29
  ld t5, REGS_T5(a0)    // x30
  ld t6, REGS_T6(a0)    // x31
  ld a0, REGS_A0(a0)    // x10

  // A plain `j` has limited range.  The `tail` pseudo-instruction gets relaxed
  // to just `j` when possible.  But when it can't be relaxed, it clobbers a
  // temporary register.  So just always do something that clobbers a known
  // register with a known value instead.
  lla t0, spin_address
  jr t0

#else

#error Unsupported architecture

#endif

.end_function

.function spin_with_fp_regs, global

#if defined(__x86_64__)

  mov $0x9999, %rax
  movq %rax, %xmm0

  movq REGS_ST(0)(%rdi), %mm0
  movq REGS_ST(1)(%rdi), %mm1
  movq REGS_ST(2)(%rdi), %mm2
  movq REGS_ST(3)(%rdi), %mm3
  movq REGS_ST(4)(%rdi), %mm4
  movq REGS_ST(5)(%rdi), %mm5
  movq REGS_ST(6)(%rdi), %mm6
  movq REGS_ST(7)(%rdi), %mm7

  jmp spin_address

#elif defined(__aarch64__)

  // Just spins and does nothing. ARM64 doesn't define a separate FP state,
  // but doing this allows the rest of the code to be platform-independent.
  b spin_address

#elif defined(__riscv)

  .macro load_fp n
    fld f\n, REGS_F(\n)(a0)
  .endm
  on_32 load_fp

  tail spin_address

#else

#error Unsupported architecture

#endif

.end_function

.function spin_with_vector_regs, global

#if defined(__x86_64__)

  // This only loads XMM registers which are guaranteed to exist.
  movdqu REGS_ZMM(0)(%rdi), %xmm0
  movdqu REGS_ZMM(1)(%rdi), %xmm1
  movdqu REGS_ZMM(2)(%rdi), %xmm2
  movdqu REGS_ZMM(3)(%rdi), %xmm3
  movdqu REGS_ZMM(4)(%rdi), %xmm4
  movdqu REGS_ZMM(5)(%rdi), %xmm5
  movdqu REGS_ZMM(6)(%rdi), %xmm6
  movdqu REGS_ZMM(7)(%rdi), %xmm7
  movdqu REGS_ZMM(8)(%rdi), %xmm8
  movdqu REGS_ZMM(9)(%rdi), %xmm9
  movdqu REGS_ZMM(10)(%rdi), %xmm10
  movdqu REGS_ZMM(11)(%rdi), %xmm11
  movdqu REGS_ZMM(12)(%rdi), %xmm12
  movdqu REGS_ZMM(13)(%rdi), %xmm13
  movdqu REGS_ZMM(14)(%rdi), %xmm14
  movdqu REGS_ZMM(15)(%rdi), %xmm15

  jmp spin_address

#elif defined(__aarch64__)

  // FPCR and FPSR are first.
#if REGS_FPSR != REGS_FPCR + 4
#error "bad layout"
#endif
  ldp w1, w2, [x0, REGS_FPCR]
  msr fpcr, x1
  msr fpsr, x2

  // Each register is 128 bits = 16 bytes, so each pair is 32 bytes.
  add x0, x0, REGS_Q(0)
  ldp q0, q1, [x0], #32
  ldp q2, q3, [x0], #32
  ldp q4, q5, [x0], #32
  ldp q6, q7, [x0], #32
  ldp q8, q9, [x0], #32
  ldp q10, q11, [x0], #32
  ldp q12, q13, [x0], #32
  ldp q14, q15, [x0], #32
  ldp q16, q17, [x0], #32
  ldp q18, q19, [x0], #32
  ldp q20, q21, [x0], #32
  ldp q22, q23, [x0], #32
  ldp q24, q25, [x0], #32
  ldp q26, q27, [x0], #32
  ldp q28, q29, [x0], #32
  ldp q30, q31, [x0]

  b spin_address

#elif defined(__riscv)

  // For indexing into the memory from which we want to load 8 vector
  // registers at a time, compute 8 * VLENB.
  csrr  a1, vlenb
  slli  a1, a1, 3

  addi    t0, a0, RISCV64_VECTOR_STATE_V
  vl8r.v  v0, (t0)
  add     t0, t0, a1
  vl8r.v  v8, (t0)
  add     t0, t0, a1
  vl8r.v  v16, (t0)
  add     t0, t0, a1
  vl8r.v  v24, (t0)

  lw      a1, RISCV64_VECTOR_STATE_VCSR(a0)
  csrw    vcsr, a1

  lw      a1, RISCV64_VECTOR_STATE_VL(a0)
  lw      a2, RISCV64_VECTOR_STATE_VTYPE(a0)
  vsetvl  zero, a1, a2

  // Any vector operation will reset `vstart`, so be sure to install this
  // value last.
  lw      a1, RISCV64_VECTOR_STATE_VSTART(a0)
  csrw    vstart, a1

  tail spin_address

#else

#error Unsupported architecture

#endif

.end_function

.function spin_with_debug_regs, global

  // Do nothing.  The register state will be set through syscalls because
  // setting the debug registers is a privileged instruction.
  spin_body

.end_function

.function save_general_regs_and_exit_thread, global

#if defined(__x86_64__)

  mov %rax, REGS_RAX(%rsp)
  mov %rbx, REGS_RBX(%rsp)
  mov %rcx, REGS_RCX(%rsp)
  mov %rdx, REGS_RDX(%rsp)
  mov %rsi, REGS_RSI(%rsp)
  mov %rdi, REGS_RDI(%rsp)
  mov %rbp, REGS_RBP(%rsp)
  mov %rsp, REGS_RSP(%rsp)
  mov %r8, REGS_R8(%rsp)
  mov %r9, REGS_R9(%rsp)
  mov %r10, REGS_R10(%rsp)
  mov %r11, REGS_R11(%rsp)
  mov %r12, REGS_R12(%rsp)
  mov %r13, REGS_R13(%rsp)
  mov %r14, REGS_R14(%rsp)
  mov %r15, REGS_R15(%rsp)

  // Save the flags register.
  pushfq
  pop %rax
  mov %rax, REGS_RFLAGS(%rsp)

  // Fill out the %rip field with known value.
  lea save_general_regs_and_exit_thread(%rip), %rax
  mov %rax, REGS_RIP(%rsp)

  jmp zx_thread_exit@PLT
  ud2

#elif defined(__aarch64__)

  stp x0, x1, [sp, REGS_X(0)]
  stp x2, x3, [sp, REGS_X(2)]
  stp x4, x5, [sp, REGS_X(4)]
  stp x6, x7, [sp, REGS_X(6)]
  stp x8, x9, [sp, REGS_X(8)]
  stp x10, x11, [sp, REGS_X(10)]
  stp x12, x13, [sp, REGS_X(12)]
  stp x14, x15, [sp, REGS_X(14)]
  stp x16, x17, [sp, REGS_X(16)]
  stp x18, x19, [sp, REGS_X(18)]
  stp x20, x21, [sp, REGS_X(20)]
  stp x22, x23, [sp, REGS_X(22)]
  stp x24, x25, [sp, REGS_X(24)]
  stp x26, x27, [sp, REGS_X(26)]
  stp x28, x29, [sp, REGS_X(28)]
  str x30, [sp, #REGS_X(30)]

  // Save the sp register.
  mov x0, sp
  str x0, [sp, REGS_SP]

  // Fill out the pc field with a known value.
  adr x0, save_general_regs_and_exit_thread
  str x0, [sp, REGS_PC]

  // Save NZCV flags, a subset of the PSTATE/CPSR register.
  mrs x0, nzcv
  str x0, [sp, REGS_CPSR]
  bl zx_thread_exit
  brk 0

#elif defined(__riscv)

  sd ra, REGS_RA(sp)
  sd sp, REGS_SP(sp)
  sd gp, REGS_GP(sp)
  sd tp, REGS_TP(sp)
  sd t0, REGS_T0(sp)
  sd t1, REGS_T1(sp)
  sd t2, REGS_T2(sp)
  sd s0, REGS_S0(sp)
  sd s1, REGS_S1(sp)
  sd a0, REGS_A0(sp)
  sd a1, REGS_A1(sp)
  sd a2, REGS_A2(sp)
  sd a3, REGS_A3(sp)
  sd a4, REGS_A4(sp)
  sd a5, REGS_A5(sp)
  sd a6, REGS_A6(sp)
  sd a7, REGS_A7(sp)
  sd s2, REGS_S2(sp)
  sd s3, REGS_S3(sp)
  sd s4, REGS_S4(sp)
  sd s5, REGS_S5(sp)
  sd s6, REGS_S6(sp)
  sd s7, REGS_S7(sp)
  sd s8, REGS_S8(sp)
  sd s9, REGS_S9(sp)
  sd s10, REGS_S10(sp)
  sd s11, REGS_S11(sp)
  sd t3, REGS_T3(sp)
  sd t4, REGS_T4(sp)
  sd t5, REGS_T5(sp)
  sd t6, REGS_T6(sp)

  // Fill out the pc field with a known value.
  lla a0, save_general_regs_and_exit_thread
  sd a0, REGS_PC(sp)

  call zx_thread_exit
  unimp

#else

#error Unsupported architecture

#endif

.end_function

.function save_fp_regs_and_exit_thread, global

#if defined(__x86_64__)

  movq %mm0, REGS_ST(0)(%rsp)
  movq %mm1, REGS_ST(1)(%rsp)
  movq %mm2, REGS_ST(2)(%rsp)
  movq %mm3, REGS_ST(3)(%rsp)
  movq %mm4, REGS_ST(4)(%rsp)
  movq %mm5, REGS_ST(5)(%rsp)
  movq %mm6, REGS_ST(6)(%rsp)
  movq %mm7, REGS_ST(7)(%rsp)

  jmp zx_thread_exit@PLT
  ud2

#elif defined(__aarch64__)

  // Does nothing (no FP values).

  bl zx_thread_exit
  brk 0

#elif defined(__riscv)

  // Save a structure that directly matches zx_riscv64_thread_state_fp_regs_t.
  li      a0, -1
  .macro save_fp_on_stack n
    fsd   f\n, REGS_F(\n)(sp)
    sd    a0, (REGS_F(\n)+8)(sp)
  .endm
  on_32 save_fp_on_stack
  csrr    a0, fcsr
  sw      a0, REGS_F(32)(sp)

  call zx_thread_exit
  unimp

#else

#error Unsupported architecture

#endif

.end_function

.function save_vector_regs_and_exit_thread, global

#if defined(__x86_64__)

  // Each vector is 512 bits (64 bytes).
  // We only read the first 128 (XMM registers).
  movdqu %xmm0, REGS_ZMM(0)(%rsp)
  movdqu %xmm1, REGS_ZMM(1)(%rsp)
  movdqu %xmm2, REGS_ZMM(2)(%rsp)
  movdqu %xmm3, REGS_ZMM(3)(%rsp)
  movdqu %xmm4, REGS_ZMM(4)(%rsp)
  movdqu %xmm5, REGS_ZMM(5)(%rsp)
  movdqu %xmm6, REGS_ZMM(6)(%rsp)
  movdqu %xmm7, REGS_ZMM(7)(%rsp)
  movdqu %xmm8, REGS_ZMM(8)(%rsp)
  movdqu %xmm9, REGS_ZMM(9)(%rsp)
  movdqu %xmm10, REGS_ZMM(10)(%rsp)
  movdqu %xmm11, REGS_ZMM(11)(%rsp)
  movdqu %xmm12, REGS_ZMM(12)(%rsp)
  movdqu %xmm13, REGS_ZMM(13)(%rsp)
  movdqu %xmm14, REGS_ZMM(14)(%rsp)
  movdqu %xmm15, REGS_ZMM(15)(%rsp)

  jmp zx_thread_exit@PLT
  ud2

#elif defined(__aarch64__)

  // FPCR and FPSR.
  mrs x0, fpcr
  mrs x1, fpsr
  stp w0, w1, [sp, REGS_FPCR]

  add x0, sp, REGS_Q(0)
  stp q0, q1, [x0], #32
  stp q2, q3, [x0], #32
  stp q4, q5, [x0], #32
  stp q6, q7, [x0], #32
  stp q8, q9, [x0], #32
  stp q10, q11, [x0], #32
  stp q12, q13, [x0], #32
  stp q14, q15, [x0], #32
  stp q16, q17, [x0], #32
  stp q18, q19, [x0], #32
  stp q20, q21, [x0], #32
  stp q22, q23, [x0], #32
  stp q24, q25, [x0], #32
  stp q26, q27, [x0], #32
  stp q28, q29, [x0], #32
  stp q30, q31, [x0]

  bl zx_thread_exit
  brk 0

  bl zx_thread_exit
  brk 0

#elif defined(__riscv)

  // Any vector operation will reset `vstart`, so be sure to save this
  // value first.
  csrr    a0, vstart
  sd      a0, RISCV64_VECTOR_STATE_VSTART(sp)

  csrr    a0, vcsr
  sd      a0, RISCV64_VECTOR_STATE_VCSR(sp)

  csrr    a0, vl
  sd      a0, RISCV64_VECTOR_STATE_VL(sp)

  csrr    a0, vtype
  sd      a0, RISCV64_VECTOR_STATE_VTYPE(sp)

  // For indexing into the memory at which we want to store 8 vector registers
  // at a time, compute 8 * VLENB.
  csrr  a1, vlenb
  slli  a1, a1, 3

  addi    a0, sp, RISCV64_VECTOR_STATE_V
  vs8r.v  v0, (a0)
  add     a0, a0, a1
  vs8r.v  v8, (a0)
  add     a0, a0, a1
  vs8r.v  v16, (a0)
  add     a0, a0, a1
  vs8r.v  v24, (a0)

  call zx_thread_exit
  unimp

#else

#error Unsupported architecture

#endif

.end_function

.function save_thread_local_regs_and_exit_thread, global

#if defined(__x86_64__)

  // Read from %fs.base and %gs.base into the output.
  // The test will assert the correct values were read.
  movq %fs:0, %rax
  movq %rax, (%rsp)
  movq %gs:0, %rax
  movq %rax, 8(%rsp)

  // Write constants %fs.base and %gs_base.
  // The test will assert the correct values were written.
  // **NOTE:** This doesn't really set much since it doesn't
  // change the registers themselves, but not all CPUs support
  // the wrfsbase and wrgsbase instructions to do that from user ode.
  movq $0x12345678, %fs:0
  movq $0x7890abcd, %gs:0

  jmp zx_thread_exit@PLT
  ud2

#elif defined(__aarch64__)

  mrs x1, tpidr_el0
  ldr x2, [x1]
  str x2, [sp]
  movlit x2, 0x12345678
  str x2, [x1]

  bl zx_thread_exit
  brk 0

#elif defined(__riscv)

  ld a0, (tp)
  sd a0, (sp)
  li a0, 0x12345678
  sd a0, (tp)

  call zx_thread_exit
  unimp

#else

#error Unsupported architecture

#endif

.end_function
