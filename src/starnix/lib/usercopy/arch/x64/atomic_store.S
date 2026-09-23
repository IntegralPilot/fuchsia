// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// On x86_64, an atomic store (with released or relaxed memory order) is just a store.

// Ref: https://godbolt.org/z/WWcreMrd4

.globl atomic_store_u32_relaxed
.globl atomic_store_u32_relaxed_end
.globl atomic_store_u32_release
.globl atomic_store_u32_release_end

atomic_store_u32_relaxed:
atomic_store_u32_release:
    movl %esi, (%rdi)
    // Success!
    xorl %eax, %eax
    retq
atomic_store_u32_relaxed_end:
atomic_store_u32_release_end:
    int3
