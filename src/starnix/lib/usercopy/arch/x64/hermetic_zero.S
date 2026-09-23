// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Reference: https://godbolt.org/z/9d6qndrvv

.globl hermetic_zero
.globl hermetic_zero_end

hermetic_zero:
    // Save rbp (frame pointer) on the stack immediately.
    //
    // If a fault exception is encountered during this routine, control will
    // be passed to `hermetic_copy_error` which expects to unwind the stack
    // fully by popping a single frame.
    pushq   %rbp
    movq    %rsp, %rbp
    movq    %rdi, %rax
    movq    %rdi, %rdx
    andq    $7, %rdx
    je      .check_if_aligned_u64s_to_zero
    movl    $8, %ecx
    subq    %rdx, %rcx
    cmpq    $8, %rsi
    cmovbq  %rsi, %rcx
    subq    %rcx, %rsi
    testq   %rcx, %rcx
    je      .check_if_aligned_u64s_to_zero
    movq    %rcx, %rdx
    andq    $7, %rdx
    je      .check_if_head_u8s_to_zero_until_aligned_or_done
    xorl    %edi, %edi
.loop_zero_head_u8s_until_u64_aligned_or_done:
    movb    $0, (%rax,%rdi)
    incq    %rdi
    cmpq    %rdi, %rdx
    jne     .loop_zero_head_u8s_until_u64_aligned_or_done
    movq    %rcx, %rdx
    subq    %rdi, %rdx
    addq    %rdi, %rax
    cmpq    $8, %rcx
    jb      .check_if_aligned_u64s_to_zero
.loop_zero_head_8x_u8s_until_u64_aligned_or_done:
    movb    $0, (%rax)
    movb    $0, 1(%rax)
    movb    $0, 2(%rax)
    movb    $0, 3(%rax)
    movb    $0, 4(%rax)
    movb    $0, 5(%rax)
    movb    $0, 6(%rax)
    movb    $0, 7(%rax)
    addq    $8, %rax
    addq    $-8, %rdx
    jne     .loop_zero_head_8x_u8s_until_u64_aligned_or_done
.check_if_aligned_u64s_to_zero:
    cmpq    $8, %rsi
    jb      .check_if_any_tail_u8s_left_to_zero
    leaq    -8(%rsi), %rcx
    movl    %ecx, %edx
    shrl    $3, %edx
    incl    %edx
    andl    $7, %edx
    je      .check_if_aligned_8x_u64s_to_zero
    shll    $3, %edx
    xorl    %edi, %edi
.loop_zero_aligned_u64s:
    movq    $0, (%rax,%rdi)
    addq    $8, %rdi
    cmpq    %rdi, %rdx
    jne     .loop_zero_aligned_u64s
    subq    %rdi, %rsi
    addq    %rdi, %rax
.check_if_aligned_8x_u64s_to_zero:
    cmpq    $56, %rcx
    jb      .check_if_any_tail_u8s_left_to_zero
.loop_zero_8x_aligned_u64s:
    movq    $0, (%rax)
    movq    $0, 8(%rax)
    movq    $0, 16(%rax)
    movq    $0, 24(%rax)
    movq    $0, 32(%rax)
    movq    $0, 40(%rax)
    movq    $0, 48(%rax)
    movq    $0, 56(%rax)
    addq    $64, %rax
    addq    $-64, %rsi
    cmpq    $7, %rsi
    ja      .loop_zero_8x_aligned_u64s
.check_if_any_tail_u8s_left_to_zero:
    testq   %rsi, %rsi
    je      .done
    movq    %rsi, %rcx
    andq    $7, %rcx
    je      .check_if_any_tail_8x_u8s_left_to_zero
    xorl    %edx, %edx
.loop_zero_tail_u8s_tail:
    movb    $0, (%rax,%rdx)
    incq    %rdx
    cmpq    %rdx, %rcx
    jne     .loop_zero_tail_u8s_tail
    movq    %rsi, %rcx
    subq    %rdx, %rcx
    addq    %rdx, %rax
    cmpq    $8, %rsi
    jb      .done
.loop_zero_tail_8x_u8s_tail:
    movb    $0, (%rax)
    movb    $0, 1(%rax)
    movb    $0, 2(%rax)
    movb    $0, 3(%rax)
    movb    $0, 4(%rax)
    movb    $0, 5(%rax)
    movb    $0, 6(%rax)
    movb    $0, 7(%rax)
    addq    $8, %rax
    addq    $-8, %rcx
    jne     .loop_zero_tail_8x_u8s_tail
.done:
    popq    %rbp
    retq
.check_if_any_tail_8x_u8s_left_to_zero:
    movq    %rsi, %rcx
    cmpq    $8, %rsi
    jae     .loop_zero_tail_8x_u8s_tail
    jmp     .done
.check_if_head_u8s_to_zero_until_aligned_or_done:
    movq    %rcx, %rdx
    cmpq    $8, %rcx
    jae     .loop_zero_head_8x_u8s_until_u64_aligned_or_done
    jmp     .check_if_aligned_u64s_to_zero
hermetic_zero_end:
    int3
