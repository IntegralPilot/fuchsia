// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Reference: https://godbolt.org/z/vW3nrv7Wr

.globl hermetic_copy_until_null_byte
.globl hermetic_copy_until_null_byte_end

.check_for_null_byte_mask:
     .quad   -9187201950435737472 // 0x8080808080808080
hermetic_copy_until_null_byte:
    // Save x1 (ra/return address) and x8 (s0/fp/frame pointer) on the stack
    // immediately.
    //
    // If a fault exception is encountered during this routine, control will
    // be passed to `hermetic_copy_error` which expects the stack to _only_
    // hold these register values.
    addi    sp, sp, -16
    sd      ra, 8(sp)
    sd      s0, 0(sp)
    addi    s0, sp, 16
    or      a4, a1, a0
    andi    a4, a4, 7
    beqz    a4, .check_if_aligned_u64s_to_copy
    xor     a4, a1, a0
    andi    a4, a4, 7
    snez    a4, a4
    sltiu   a5, a2, 8
    or      a5, a5, a4
    mv      a4, a2
    bnez    a5, .check_if_head_u8s_to_copy_until_aligned_or_done
    andi    a4, a0, 7
    li      a5, 8
    sub     a4, a5, a4
.check_if_head_u8s_to_copy_until_aligned_or_done:
    sub     a2, a2, a4
    beqz    a4, .check_if_aligned_u64s_to_copy
.loop_copy_head_u8s_until_u64_aligned_or_done:
    lbu     a5, 0(a1)
    addi    a1, a1, 1
    sb      a5, 0(a0)
    addi    a0, a0, 1
    beqz    a5, .prepare_to_return
    addi    a4, a4, -1
    bnez    a4, .loop_copy_head_u8s_until_u64_aligned_or_done
.check_if_aligned_u64s_to_copy:
    li      a4, 8
    bltu    a2, a4, .check_if_any_tail_u8s_left_to_copy
.prepare_constants_to_check_for_null_byte:
    // See https://jameshfisher.com/2017/01/24/bitwise-check-for-zero-byte/ for
    // details on the algorithm used to check for a null byte in a word.
    //
    // a7 will hold the null byte mask.
    auipc   a4, %pcrel_hi(.check_for_null_byte_mask)
    ld      a7, %pcrel_lo(.prepare_constants_to_check_for_null_byte)(a4)
    // t0 will hold the constant we use to subtract 0x01 from each byte.
    lui     a4, 1044464
    addiw   a5, a4, -257
    slli    a4, a5, 32
    add     t0, a5, a4
    li      a6, 7
.loop_copy_aligned_u64s:
    ld      a4, 0(a1)
    // Check if null byte exists in the u64.
    not     a5, a4
    add     t1, a4, t0
    and     a5, a5, a7
    and     a5, a5, t1
    bnez    a5, .loop_find_and_return_null_byte_in_aligned_u64
    // null byte not found, continue to copy the u64 to the destination.
    sd      a4, 0(a0)
    addi    a0, a0, 8
    addi    a2, a2, -8
    addi    a1, a1, 8
    bltu    a6, a2, .loop_copy_aligned_u64s
.check_if_any_tail_u8s_left_to_copy:
    beqz    a2, .prepare_to_return
    addi    a2, a2, -1
.loop_copy_tail_u8s:
    lbu     a5, 0(a1)
    addi    a1, a1, 1
    sb      a5, 0(a0)
    addi    a0, a0, 1
    beqz    a5, .prepare_to_return
    mv      a4, a2
    addi    a2, a2, -1
    bnez    a4, .loop_copy_tail_u8s
    j       .prepare_to_return
.loop_find_and_return_null_byte_in_aligned_u64:
    lbu     a2, 0(a1)
    addi    a1, a1, 1
    sb      a2, 0(a0)
    addi    a0, a0, 1
    bnez    a2, .loop_find_and_return_null_byte_in_aligned_u64
.prepare_to_return:
    bnez    a3, .done
    // Move the source address to the return register.
    mv      a0, a1
.done:
    ld      ra, 8(sp)
    ld      s0, 0(sp)
    addi    sp, sp, 16
    ret
hermetic_copy_until_null_byte_end:
    unimp
