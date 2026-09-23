// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Reference: https://godbolt.org/z/9d6qndrvv

.globl hermetic_zero
.globl hermetic_zero_end

hermetic_zero:
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
    andi    a3, a0, 7
    beqz    a3, .check_if_aligned_u64s_to_zero
    li      a4, 8
    mv      a2, a1
    bltu    a1, a4, .check_if_head_u8s_to_zero_until_aligned_or_done
    sub     a2, a4, a3
.check_if_head_u8s_to_zero_until_aligned_or_done:
    sub     a1, a1, a2
    beqz    a2, .check_if_aligned_u64s_to_zero
    add     a2, a2, a0
.loop_zero_head_u8s_until_u64_aligned_or_done:
    mv      a3, a0
    addi    a0, a0, 1
    sb      zero, 0(a3)
    bne     a0, a2, .loop_zero_head_u8s_until_u64_aligned_or_done
.check_if_aligned_u64s_to_zero:
    li      a2, 8
    bltu    a1, a2, .check_if_any_tail_u8s_left_to_zero
    li      a2, 7
.loop_zero_aligned_u64s:
    sd      zero, 0(a0)
    addi    a1, a1, -8
    addi    a0, a0, 8
    bltu    a2, a1, .loop_zero_aligned_u64s
.check_if_any_tail_u8s_left_to_zero:
    beqz    a1, .prepare_to_return_destination_address
    add     a2, a0, a1
.loop_zero_tail_u8s_tail:
    addi    a1, a0, 1
    sb      zero, 0(a0)
    mv      a0, a1
    bne     a1, a2, .loop_zero_tail_u8s_tail
    j       .done
.prepare_to_return_destination_address:
    mv      a1, a0
.done:
    // The address we return should be in a1.
    mv      a0, a1
    ld      ra, 8(sp)
    ld      s0, 0(sp)
    addi    sp, sp, 16
    ret
hermetic_zero_end:
    unimp
