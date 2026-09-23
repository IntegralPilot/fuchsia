// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.text

// int SpinUntilMigratedOrAborted(zx_rseq_t* rseq);
//
// a0 = rseq

.function SpinUntilMigratedOrAborted, global
    // Set up the sequence.
    //
    // Store the start_ip.
    la t0, .Lstart
    sd t0, 8(a0)
    // Compute and store the post_commit_offset.
    lla t0, .Lend
    lla t1, .Lstart
    sub t0, t0, t1
    sd t0, 16(a0)
    // Store the abort_ip.
    la t0, .Labort
    sd t0, 24(a0)

.Lstart:
    // Load the cpu_id from within the sequence.
    lw t1, 0(a0)

.Lloop:
    // Check if we've been migrated without being aborted.
    lw t2, 0(a0)
    bne t1, t2, .Lmigration
    pause
    j .Lloop
.Lend:
    ebreak

.Lmigration:
    // We observed a change in the cpu_id.  Return 0.
    sd zero, 8(a0)
    li a0, 0
    ret

.Labort:
    // We were aborted.  Return 1.
    sd zero, 8(a0)
    li a0, 1
    ret
.end_function

// int RseqIncrement(uintptr_t* counter, uintptr_t val, zx_rseq_t* rseq, uint32_t cpu);
//
// a0=counter, a1=val, a2=rseq, a3=cpu

.function RseqIncrement, global
    // Set up the sequence.
    la t0, .Lincr_start
    // start_ip
    sd t0, 8(a2)
    lla t0, .Lincr_post
    lla t1, .Lincr_start
    sub t0, t0, t1
    // post_commit_offset
    sd t0, 16(a2)
    la t0, .Lincr_abort
    // abort_ip
    sd t0, 24(a2)

.Lincr_start:
    // Load cpu_id
    lw t0, 0(a2)
    // Abort if not equal
    bne t0, a3, .Lincr_abort

    // Load *counter
    ld t1, 0(a0)
    // Add val
    add t1, t1, a1

    // Widen the window for interrupts
    .rept 1000
    nop
    .endr

    // Store *counter
    sd t1, 0(a0)
.Lincr_post:
    // Success. Clear start_ip to deactivate.
    sd zero, 8(a2)
    // Return 1 (success)
    li a0, 1
    ret

.Lincr_abort:
    // Abort. Clear start_ip to deactivate.
    sd zero, 8(a2)
    // Return 0 (failure)
    li a0, 0
    ret
.end_function
