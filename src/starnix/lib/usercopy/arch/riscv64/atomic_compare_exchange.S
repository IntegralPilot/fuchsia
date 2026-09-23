// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Ref: https://godbolt.org/z/WWcreMrd4

.globl atomic_compare_exchange_u32_acq_rel
.globl atomic_compare_exchange_u32_acq_rel_end
.globl atomic_compare_exchange_weak_u32_acq_rel
.globl atomic_compare_exchange_weak_u32_acq_rel_end

atomic_compare_exchange_u32_acq_rel:
atomic_compare_exchange_weak_u32_acq_rel:
    lw      a5, 0(a1)
.cmpxchg_loop:
    lr.w.aq a4, 0(a0)
    bne     a4, a5, .cmpxchg_store_actual
    sc.w.rl a6, a2, 0(a0)
    bnez    a6, .cmpxchg_loop
.cmpxchg_store_actual:
    subw    a5, a4, a5
    seqz    a0, a5
    beq     a5, zero, .cmpxchg_ret
    sw      a4, 0(a1)
.cmpxchg_ret:
    andi    a0, a0, 1
    ret
atomic_compare_exchange_u32_acq_rel_end:
atomic_compare_exchange_weak_u32_acq_rel_end:
    unimp
