// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// On x86_64, strong and weak compare_exchange operations with acquire/release semantics have the same implementation.

// Ref: https://godbolt.org/z/WWcreMrd4

.globl atomic_compare_exchange_u32_acq_rel
.globl atomic_compare_exchange_u32_acq_rel_end
.globl atomic_compare_exchange_weak_u32_acq_rel
.globl atomic_compare_exchange_weak_u32_acq_rel_end

atomic_compare_exchange_u32_acq_rel:
atomic_compare_exchange_weak_u32_acq_rel:
  movl (%rsi), %eax
  xorl %ecx, %ecx
  lock cmpxchgl %edx, (%rdi)
  sete %dl
  je   .success
  movl %eax, (%rsi)
.success:
  movb %dl, %cl
  movq %rcx, %rax
  retq
atomic_compare_exchange_u32_acq_rel_end:
atomic_compare_exchange_weak_u32_acq_rel_end:
  int3
