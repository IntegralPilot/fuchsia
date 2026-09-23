// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Reference: https://godbolt.org/z/vW3nrv7Wr

.globl hermetic_copy_until_null_byte
.globl hermetic_copy_until_null_byte_end

hermetic_copy_until_null_byte:
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
.loop_copy_head_u8s_until_u64_aligned_or_done:
    movzbl  (%rax), %r8d
    incq    %rax
    movb    %r8b, (%rdi)
    incq    %rdi
    testb   %r8b, %r8b
    je      .done
    decq    %rsi
    jne     .loop_copy_head_u8s_until_u64_aligned_or_done
.check_if_aligned_u64s_to_copy:
    cmpq    $8, %rdx
    jb      .check_if_any_tail_u8s_left_to_copy
    movabsq $-9187201950435737472, %rsi
    movabsq $-72340172838076673, %r8
.loop_copy_aligned_u64s:
    movq    (%rax), %r9
    movq    %r9, %r10
    notq    %r10
    andq    %rsi, %r10
    leaq    (%r9,%r8), %r11
    testq   %r11, %r10
    jne     .loop_find_null_byte_in_aligned_u64
    movq    %r9, (%rdi)
    addq    $8, %rdi
    addq    $8, %rax
    addq    $-8, %rdx
    cmpq    $7, %rdx
    ja      .loop_copy_aligned_u64s
.check_if_any_tail_u8s_left_to_copy:
    testq   %rdx, %rdx
    je      .done
    decq    %rdx
.loop_copy_tail_u8s_tail:
    movzbl  (%rax), %r8d
    incq    %rax
    movb    %r8b, (%rdi)
    incq    %rdi
    subq    $1, %rdx
    setb    %sil
    testb   %r8b, %r8b
    je      .done
    testb   %sil, %sil
    je      .loop_copy_tail_u8s_tail
    jmp     .done
.loop_find_null_byte_in_aligned_u64:
    movzbl  (%rax), %edx
    incq    %rax
    movb    %dl, (%rdi)
    incq    %rdi
    testb   %dl, %dl
    jne     .loop_find_null_byte_in_aligned_u64
.done:
    testb   %cl, %cl
    cmovneq %rdi, %rax
    popq    %rbp
    retq
hermetic_copy_until_null_byte_end:
    int3
