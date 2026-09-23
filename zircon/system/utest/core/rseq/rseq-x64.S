// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.text

// int SpinUntilMigratedOrAborted(zx_rseq_t* rseq);
//
// rdi = rseq

.function SpinUntilMigratedOrAborted, global
    // Set up the sequence.
    //
    // Store the start_ip.
    lea .Lstart(%rip), %rax
    mov %rax, 8(%rdi)
    // Compute and store the post_commit_offset.
    movq $(.Lend - .Lstart), 16(%rdi)
    // Store the abort_ip.
    lea .Labort(%rip), %rax
    mov %rax, 24(%rdi)

.Lstart:
    // Load the cpu_id from within the sequence.
    mov 0(%rdi), %esi

.Lloop:
    // Check if we've been migrated without being aborted.
    cmp %esi, 0(%rdi)
    jne .Lmigration
    pause
    jmp .Lloop
.Lend:
    ud2

.Lmigration:
    // We observed a change in the cpu_id.  Return 0.
    movq $0, 8(%rdi)
    xor %rax, %rax
    ret

.Labort:
    // We were aborted.  Return 1.
    movq $0, 8(%rdi)
    mov $1, %rax
    ret
.end_function

// int RseqIncrement(uintptr_t* counter, uintptr_t val, zx_rseq_t* rseq, uint32_t cpu);
//
// rdi=counter, rsi=val, rdx=rseq, rcx=cpu

.function RseqIncrement, global
    // Set up the sequence.
    leaq .Lincr_start(%rip), %rax
    // start_ip
    movq %rax, 8(%rdx)
    // post_commit_offset
    movq $(.Lincr_post - .Lincr_start), 16(%rdx)
    leaq .Lincr_abort(%rip), %rax
    // abort_ip
    movq %rax, 24(%rdx)

.Lincr_start:
    // Load cpu_id
    movl (%rdx), %r8d
    // Compare with expected cpu
    cmpl %ecx, %r8d
    // Abort if not equal
    jne .Lincr_abort

    // Load value from *counter
    movq (%rdi), %r9
    // Increment value
    addq %rsi, %r9

    // Widen the window for interrupts
    .rept 1000
    nop
    .endr

    // Store value back to *counter
    movq %r9, (%rdi)
.Lincr_post:
    // Success. Clear start_ip to deactivate.
    movq $0, 8(%rdx)
    // Return 1 (success)
    movl $1, %eax
    ret

.Lincr_abort:
    // Abort. Clear start_ip to deactivate.
    movq $0, 8(%rdx)
    // Return 0 (failure)
    xorl %eax, %eax
    ret
.end_function
