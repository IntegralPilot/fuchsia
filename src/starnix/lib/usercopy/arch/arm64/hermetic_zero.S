// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Reference: https://godbolt.org/z/9d6qndrvv

.globl hermetic_zero
.globl hermetic_zero_end

hermetic_zero:
    // Save x29 (frame pointer) and x30 (link register) on the stack
    // immediately.
    //
    // If a fault exception is encountered during this routine, control will
    // be passed to `hermetic_copy_error` which expects to unwind the stack
    // fully by popping a single frame.
    stp     x29, x30, [sp, #-16]!
    mov     x29, sp
    ands    x8, x0, #0x7
    b.eq    .check_if_atleast_u64_left_to_zero_after_alignment
    mov     w9, #8
    cmp     x1, #8
    sub     x8, x9, x8
    csel    x8, x1, x8, lo
    sub     x1, x1, x8
    cbz     x8, .check_if_atleast_u64_left_to_zero_after_alignment
.loop_zero_head_u8s_until_u64_aligned_or_done:
    subs    x8, x8, #1
    strb    wzr, [x0], #1
    b.ne    .loop_zero_head_u8s_until_u64_aligned_or_done
.check_if_atleast_u64_left_to_zero_after_alignment:
    cmp     x1, #8
    b.lo    .check_if_any_tail_u8s_left_to_zero
.loop_zero_aligned_u64s:
    sub     x1, x1, #8
    str     xzr, [x0], #8
    cmp     x1, #7
    b.hi    .loop_zero_aligned_u64s
.check_if_any_tail_u8s_left_to_zero:
    cbz     x1, .done
.loop_zero_tail_u8s:
    subs    x1, x1, #1
    strb    wzr, [x0], #1
    b.ne    .loop_zero_tail_u8s
.done:
    ldp     x29, x30, [sp], #16
    ret
hermetic_zero_end:
    brk    #0x01
