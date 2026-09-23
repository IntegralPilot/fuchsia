// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Reference: https://godbolt.org/z/1qc317rf8

.globl hermetic_copy
.globl hermetic_copy_end

hermetic_copy:
    // Save x1 (ra/return address) and x8 (s0/fp/frame pointer) on the stack
    // immediately.
    //
    // If a fault exception is encountered during this routine, control will
    // be passed to `hermetic_copy_error` which expects to unwind the stack
    // fully by popping a single frame.
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
    add     a4, a4, a1
.loop_copy_head_u8s_until_u64_aligned_or_done:
    lbu     a5, 0(a1)
    addi    a1, a1, 1
    sb      a5, 0(a0)
    addi    a0, a0, 1
    bne     a1, a4, .loop_copy_head_u8s_until_u64_aligned_or_done
.check_if_aligned_u64s_to_copy:
    li      a4, 8
    bltu    a2, a4, .check_if_any_tail_u8s_left_to_copy
    li      a4, 7
.loop_copy_aligned_u64s:
    ld      a5, 0(a1)
    sd      a5, 0(a0)
    addi    a0, a0, 8
    addi    a2, a2, -8
    addi    a1, a1, 8
    bltu    a4, a2, .loop_copy_aligned_u64s
.check_if_any_tail_u8s_left_to_copy:
    beqz    a2, .no_tail_u8s_left_to_copy
    add     a6, a1, a2
.loop_copy_tail_u8s_tail:
    lbu     a4, 0(a1)
    addi    a5, a1, 1
    addi    a2, a0, 1
    sb      a4, 0(a0)
    mv      a0, a2
    mv      a1, a5
    bne     a5, a6, .loop_copy_tail_u8s_tail
    // Skip the next instruction if we should return the source
    // address since the last accessed source address sits in a2.
    bnez    a3, .done
.prepare_to_return_destination_address:
    mv      a2, a5
.done:
    // The address we return should be in a2.
    mv      a0, a2
    ld      ra, 8(sp)
    ld      s0, 0(sp)
    addi    sp, sp, 16
    ret
.no_tail_u8s_left_to_copy:
    mv      a5, a1
    mv      a2, a0
    beqz    a3, .prepare_to_return_destination_address
    j       .done
hermetic_copy_end:
    unimp
