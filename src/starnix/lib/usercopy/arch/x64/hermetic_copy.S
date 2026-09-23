// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Reference: https://godbolt.org/z/1qc317rf8

.globl hermetic_copy
.globl hermetic_copy_end

hermetic_copy:
    // Save rbp (frame pointer) on the stack immediately.
    //
    // If a fault exception is encountered during this routine, control will
    // be passed to `hermetic_copy_error` which expects to unwind the stack
    // fully by popping a single frame.
    pushq   %rbp
    movq    %rsp, %rbp
    movq    %rsi, %rax
    orl     %edi, %esi
    testb   $7, %sil
    je      .check_if_aligned_u64s_to_copy
    movl    %eax, %r8d
    xorl    %edi, %r8d
    movl    %edi, %r9d
    andl    $7, %r9d
    movl    $8, %esi
    subq    %r9, %rsi
    testb   $7, %r8b
    cmovneq %rdx, %rsi
    cmpq    $8, %rdx
    cmovbq  %rdx, %rsi
    subq    %rsi, %rdx
    testq   %rsi, %rsi
    je      .check_if_aligned_u64s_to_copy
    movq    %rsi, %r8
    andq    $7, %r8
    je      .check_if_head_u8s_to_copy_until_aligned_or_done
    xorl    %r9d, %r9d
.loop_copy_head_u8s_until_u64_aligned_or_done:
    movzbl  (%rax,%r9), %r10d
    movb    %r10b, (%rdi,%r9)
    incq    %r9
    cmpq    %r9, %r8
    jne     .loop_copy_head_u8s_until_u64_aligned_or_done
    addq    %r9, %rdi
    addq    %r9, %rax
    movq    %rsi, %r8
    subq    %r9, %r8
    cmpq    $8, %rsi
    jb      .check_if_aligned_u64s_to_copy
.loop_copy_head_8x_u8s_until_u64_aligned_or_done:
    movzbl  (%rax), %esi
    movb    %sil, (%rdi)
    movzbl  1(%rax), %esi
    movb    %sil, 1(%rdi)
    movzbl  2(%rax), %esi
    movb    %sil, 2(%rdi)
    movzbl  3(%rax), %esi
    movb    %sil, 3(%rdi)
    movzbl  4(%rax), %esi
    movb    %sil, 4(%rdi)
    movzbl  5(%rax), %esi
    movb    %sil, 5(%rdi)
    movzbl  6(%rax), %esi
    movb    %sil, 6(%rdi)
    movzbl  7(%rax), %esi
    addq    $8, %rax
    movb    %sil, 7(%rdi)
    addq    $8, %rdi
    addq    $-8, %r8
    jne     .loop_copy_head_8x_u8s_until_u64_aligned_or_done
.check_if_aligned_u64s_to_copy:
    cmpq    $8, %rdx
    jb      .check_if_any_tail_u8s_left_to_copy
    leaq    -8(%rdx), %rsi
    movl    %esi, %r8d
    shrl    $3, %r8d
    incl    %r8d
    andl    $7, %r8d
    je      .check_if_aligned_8x_u64s_to_copy
    shll    $3, %r8d
    xorl    %r9d, %r9d
.loop_copy_aligned_u64s:
    movq    (%rax,%r9), %r10
    movq    %r10, (%rdi,%r9)
    addq    $8, %r9
    cmpq    %r9, %r8
    jne     .loop_copy_aligned_u64s
    addq    %r9, %rdi
    addq    %r9, %rax
    subq    %r9, %rdx
.check_if_aligned_8x_u64s_to_copy:
    cmpq    $56, %rsi
    jb      .check_if_any_tail_u8s_left_to_copy
.loop_copy_8x_aligned_u64s:
    movq    (%rax), %rsi
    movq    %rsi, (%rdi)
    movq    8(%rax), %rsi
    movq    %rsi, 8(%rdi)
    movq    16(%rax), %rsi
    movq    %rsi, 16(%rdi)
    movq    24(%rax), %rsi
    movq    %rsi, 24(%rdi)
    movq    32(%rax), %rsi
    movq    %rsi, 32(%rdi)
    movq    40(%rax), %rsi
    movq    %rsi, 40(%rdi)
    movq    48(%rax), %rsi
    movq    %rsi, 48(%rdi)
    movq    56(%rax), %rsi
    movq    %rsi, 56(%rdi)
    addq    $64, %rdi
    addq    $64, %rax
    addq    $-64, %rdx
    cmpq    $7, %rdx
    ja      .loop_copy_8x_aligned_u64s
.check_if_any_tail_u8s_left_to_copy:
    testq   %rdx, %rdx
    je      .done
    movq    %rdx, %rsi
    andq    $7, %rsi
    je      .check_if_any_tail_8x_u8s_left_to_copy
    xorl    %r8d, %r8d
.loop_copy_tail_u8s_tail:
    movzbl  (%rax,%r8), %r9d
    movb    %r9b, (%rdi,%r8)
    incq    %r8
    cmpq    %r8, %rsi
    jne     .loop_copy_tail_u8s_tail
    addq    %r8, %rdi
    addq    %r8, %rax
    movq    %rdx, %rsi
    subq    %r8, %rsi
    cmpq    $8, %rdx
    jb      .done
.loop_copy_tail_8x_u8s_tail:
    movzbl  (%rax), %edx
    movb    %dl, (%rdi)
    movzbl  1(%rax), %edx
    movb    %dl, 1(%rdi)
    movzbl  2(%rax), %edx
    movb    %dl, 2(%rdi)
    movzbl  3(%rax), %edx
    movb    %dl, 3(%rdi)
    movzbl  4(%rax), %edx
    movb    %dl, 4(%rdi)
    movzbl  5(%rax), %edx
    movb    %dl, 5(%rdi)
    movzbl  6(%rax), %edx
    movb    %dl, 6(%rdi)
    movzbl  7(%rax), %edx
    addq    $8, %rax
    movb    %dl, 7(%rdi)
    addq    $8, %rdi
    addq    $-8, %rsi
    jne     .loop_copy_tail_8x_u8s_tail
.done:
    testb   %cl, %cl
    cmovneq %rdi, %rax
    popq    %rbp
    retq
.check_if_any_tail_8x_u8s_left_to_copy:
    movq    %rdx, %rsi
    cmpq    $8, %rdx
    jae     .loop_copy_tail_8x_u8s_tail
    jmp     .done
.check_if_head_u8s_to_copy_until_aligned_or_done:
    movq    %rsi, %r8
    cmpq    $8, %rsi
    jae     .loop_copy_head_8x_u8s_until_u64_aligned_or_done
    jmp     .check_if_aligned_u64s_to_copy
hermetic_copy_end:
    int3
