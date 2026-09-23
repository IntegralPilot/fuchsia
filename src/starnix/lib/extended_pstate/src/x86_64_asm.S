// Copyright 2026 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <starnix_asm_utils/asm.h>

// XSaveArea size is 832
// LINT.IfChange(x86_64_state_offsets)
#define STRATEGY_OFFSET 832
// LINT.ThenChange(x86_64.rs:x86_64_state_offsets)

// LINT.IfChange(strategy_discriminants)
#define STRATEGY_XSAVEOPT 0
#define STRATEGY_XSAVE 1
#define STRATEGY_FXSAVE 2
// LINT.ThenChange(x86_64.rs:strategy_discriminants)

FUNCTION(save_extended_pstate)
    // state_addr is in %rdi
    movq (%rdi), %rsi // %rsi is state_ptr
    movl STRATEGY_OFFSET(%rsi), %ecx
    cmpl $STRATEGY_XSAVEOPT, %ecx
    je .L_xsaveopt
    cmpl $STRATEGY_XSAVE, %ecx
    je .L_xsave
    cmpl $STRATEGY_FXSAVE, %ecx
    je .L_fxsave
    retq

    // XSAVEOPT: Save Processor Extended States Optimized, Intel Manual Volume 2D Chapter 6
    // Request mask in EDX:EAX. EAX=7 (x87, SSE, AVX state), EDX=0.
    // Operand: Memory address to which to save state.
.L_xsaveopt:
    movl $7, %eax
    movl $0, %edx
    xsaveopt (%rsi)
    retq

    // XSAVE: Save Processor Extended States, Intel Manual Volume 2D Chapter 6
    // Request mask in EDX:EAX. EAX=7 (x87, SSE, AVX state), EDX=0.
    // Operand: Memory address to which to save state.
.L_xsave:
    movl $7, %eax
    movl $0, %edx
    xsave (%rsi)
    retq

    // FXSAVE: Save x87 FPU, MMX Technology, and SSE State, Intel Manual Volume 2A Chapter 3
    // Operand: Memory address to which to save state.
.L_fxsave:
    fxsave (%rsi)
    retq
END_FUNCTION(save_extended_pstate)

FUNCTION(restore_extended_pstate)
    // state_addr is in %rdi
    movq (%rdi), %rsi
    movl STRATEGY_OFFSET(%rsi), %ecx
    cmpl $STRATEGY_FXSAVE, %ecx
    je .L_frstor

    // XRSTOR: Restore Processor Extended States, Intel Manual Volume 2D Chapter 6
    // Request mask in EDX:EAX. EAX=7 (x87, SSE, AVX state), EDX=0.
    // Operand: Memory address from which to restore state.
    movl $7, %eax
    movl $0, %edx
    xrstor (%rsi)
    retq

    // FXRSTOR: Restore x87 FPU, MMX Technology, and SSE State, Intel Manual Volume 2A Chapter 3
    // Operand: Memory address from which to restore state.
.L_frstor:
    fxrstor (%rsi)
    retq
END_FUNCTION(restore_extended_pstate)
