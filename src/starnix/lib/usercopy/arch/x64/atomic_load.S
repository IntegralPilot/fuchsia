// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// On x86_64, an atomic load is just a load.

// Ref: https://godbolt.org/z/WWcreMrd4

.globl atomic_load_u32_relaxed
.globl atomic_load_u32_relaxed_end
.globl atomic_load_u32_acquire
.globl atomic_load_u32_acquire_end

atomic_load_u32_relaxed:
atomic_load_u32_acquire:
    movl (%rdi), %eax
    retq
atomic_load_u32_relaxed_end:
atomic_load_u32_acquire_end:
    int3
