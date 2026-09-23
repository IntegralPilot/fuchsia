// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Reference: https://godbolt.org/z/1qc317rf8

.globl hermetic_copy
.globl hermetic_copy_end

hermetic_copy:
    // Save x29 (frame pointer) and x30 (link register) on the stack
    // immediately.
    //
    // If a fault exception is encountered during this routine, control will
    // be passed to `hermetic_copy_error` which expects to unwind the stack
    // fully by popping a single frame.
    stp     x29, x30, [sp, #-16]!
    mov     x29, sp
    orr     w8, w1, w0
    tst     x8, #0x7
    b.eq    .check_if_atleast_u64_left_to_copy_after_alignment
    eor     w8, w1, w0
    and     x9, x0, #0x7
    mov     w10, #8
    tst     x8, #0x7
    sub     x8, x10, x9
    ccmp    x2, #8, #0, eq
    csel    x8, x2, x8, lo
    sub     x2, x2, x8
    cbz     x8, .check_if_atleast_u64_left_to_copy_after_alignment
.loop_copy_head_u8s_until_u64_aligned_or_done:
    ldrb    w9, [x1], #1
    subs    x8, x8, #1
    strb    w9, [x0], #1
    b.ne    .loop_copy_head_u8s_until_u64_aligned_or_done
.check_if_atleast_u64_left_to_copy_after_alignment:
    cmp     x2, #8
    b.lo    .check_if_any_tail_u8s_left_to_copy
.loop_copy_aligned_u64s:
    ldr     x8, [x1], #8
    sub     x2, x2, #8
    cmp     x2, #7
    str     x8, [x0], #8
    b.hi    .loop_copy_aligned_u64s
.check_if_any_tail_u8s_left_to_copy:
    cbz     x2, .done
.loop_copy_tail_u8s:
    ldrb    w8, [x1], #1
    subs    x2, x2, #1
    strb    w8, [x0], #1
    b.ne    .loop_copy_tail_u8s
.done:
    tst     w3, #0x1
    csel    x0, x0, x1, ne
    ldp     x29, x30, [sp], #16
    ret
hermetic_copy_end:
    brk  #0x1
