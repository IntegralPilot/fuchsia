// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// On riscv, lw provides an atomic read (in the sense that torn reads will not
// occur). To provide synchronization, the fence instruction is used.

// Ref: https://godbolt.org/z/WWcreMrd4

.globl atomic_load_u32_relaxed
.globl atomic_load_u32_relaxed_end
.globl atomic_load_u32_acquire
.globl atomic_load_u32_acquire_end

atomic_load_u32_relaxed:
    lw    a0, 0(a0)
    slli  a0, a0, 32
    srli  a0, a0, 32
    ret
atomic_load_u32_relaxed_end:
    unimp

atomic_load_u32_acquire:
    lw    a0, 0(a0)
    // No reads/writes performed after the fence may be reordered before reads
    // performed the before fence.
    fence r, rw
    slli  a0, a0, 32
    srli  a0, a0, 32
    ret
atomic_load_u32_acquire_end:
    unimp
