// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Note: These routines use ldxr/stxr. If we support processors with LSE support we should use those
// instructions instead when available.

// Ref: https://godbolt.org/z/WWcreMrd4

.globl atomic_compare_exchange_u32_acq_rel
.globl atomic_compare_exchange_u32_acq_rel_end
atomic_compare_exchange_u32_acq_rel:
  ldr     w9, [x1]
.compare_loop:
  ldaxr   w8, [x0]
  cmp     w8, w9
  b.ne    .compare_failed
  stlxr   w10, w2, [x0]
  cbnz    w10, .compare_loop
  mov     w9, #1
  tbz     w9, #0, .compare_store_actual
  b       .compare_ret
.compare_failed:
  mov     w9, wzr
  clrex
  tbnz    w9, #0, .compare_ret
.compare_store_actual:
  str     w8, [x1]
.compare_ret:
  mov     w0, w9
  ret
atomic_compare_exchange_u32_acq_rel_end:
  brk     #0x1

.globl atomic_compare_exchange_weak_u32_acq_rel
.globl atomic_compare_exchange_weak_u32_acq_rel_end
atomic_compare_exchange_weak_u32_acq_rel:
  ldr     w9, [x1]
  ldaxr   w8, [x0]
  cmp     w8, w9
  b.ne    .compare_weak_failed
  stlxr   w9, w2, [x0]
  cmp     w9, #0
  csetm   w9, eq
  tbz     w9, #0, .compare_weak_store_actual
  b       .compare_weak_ret
.compare_weak_failed:
  mov     w9, wzr
  clrex
  tbnz    w9, #0, .compare_weak_ret
.compare_weak_store_actual:
  str     w8, [x1]
.compare_weak_ret:
  and     x0, x9, #0x1
  ret
atomic_compare_exchange_weak_u32_acq_rel_end:
  brk     #0x1
