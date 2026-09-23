// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// On riscv, sw provides an atomic write (in the sense that torn writes will not
// occur). To provide synchronization, the fence instruction is used.

// Ref: https://godbolt.org/z/WWcreMrd4

.globl atomic_store_u32_relaxed
.globl atomic_store_u32_relaxed_end
.globl atomic_store_u32_release
.globl atomic_store_u32_release_end

atomic_store_u32_relaxed:
    sw    a1, 0(a0)
    li    a0, 0
    ret
atomic_store_u32_relaxed_end:
    unimp

atomic_store_u32_release:
    // No writes performed after the fence may be reordered before reads/writes
    // performed before the fence.
    fence rw, w
    sw    a1, 0(a0)
    li    a0, 0
    ret
atomic_store_u32_release_end:
    unimp
