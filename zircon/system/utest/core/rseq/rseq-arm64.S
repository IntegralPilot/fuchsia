// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.text

// int SpinUntilMigratedOrAborted(zx_rseq_t* rseq);
//
// x0 = rseq

.function SpinUntilMigratedOrAborted, global
    // Set up the sequence.
    //
    // Store the start_ip.
    adr x2, .Lstart
    str x2, [x0, #8]
    // Compute and store the post_commit_offset.
    mov x2, #(.Lend - .Lstart)
    str x2, [x0, #16]
    // Store the abort_ip.
    adr x2, .Labort
    str x2, [x0, #24]

.Lstart:
    // Load the cpu_id from within the sequence.
    ldr w1, [x0]

.Lloop:
    // Check if we've been migrated without being aborted.
    ldr w2, [x0]
    cmp w1, w2
    b.ne .Lmigration
    yield
    b .Lloop
.Lend:
    brk #0

.Lmigration:
    // We observed a change in the cpu_id.  Return 0.
    mov x2, #0
    str x2, [x0, #8]
    mov x0, #0
    ret

.Labort:
    // We were aborted.  Return 1.
    mov x2, #0
    str x2, [x0, #8]
    mov x0, #1
    ret
.end_function

// int RseqIncrement(uintptr_t* counter, uintptr_t val, zx_rseq_t* rseq, uint32_t cpu);
//
// x0=counter, x1=val, x2=rseq, x3=cpu

.function RseqIncrement, global
    // Set up the sequence.
    adr x4, .Lincr_start
    // start_ip
    str x4, [x2, #8]
    mov x4, #(.Lincr_post - .Lincr_start)
    // post_commit_offset
    str x4, [x2, #16]
    adr x4, .Lincr_abort
    // abort_ip
    str x4, [x2, #24]

.Lincr_start:
    // Load cpu_id
    ldr w4, [x2]
    // Compare with expected cpu
    cmp w4, w3
    // Abort if not equal
    b.ne .Lincr_abort

    // Load *counter
    ldr x5, [x0]
    // Add val
    add x5, x5, x1

    // Widen the window for interrupts
    .rept 1000
    nop
    .endr

    // Store *counter
    str x5, [x0]
.Lincr_post:
    // Success. Clear start_ip to deactivate.
    mov x4, #0
    str x4, [x2, #8]
    // Return 1 (success)
    mov w0, #1
    ret

.Lincr_abort:
    // Abort. Clear start_ip to deactivate.
    mov x4, #0
    str x4, [x2, #8]
    // Return 0 (failure)
    mov w0, #0
    ret
.end_function
