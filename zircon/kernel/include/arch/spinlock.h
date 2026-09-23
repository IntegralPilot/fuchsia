// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2014 Travis Geiselbrecht
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_ARCH_INCLUDE_ARCH_SPINLOCK_H_
#define ZIRCON_KERNEL_ARCH_INCLUDE_ARCH_SPINLOCK_H_

#include <lib/fxt/interned_string.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdbool.h>

#include <arch/ops.h>
#include <kernel/cpu.h>
#include <kernel/spin_tracing_config.h>
#include <ktl/atomic.h>

struct TA_CAP("mutex") arch_spin_lock_t {
  ktl::atomic<cpu_num_t> value;
};

void arch_spin_lock_non_instrumented(arch_spin_lock_t* lock) TA_ACQ(lock);
void arch_spin_lock_trace_instrumented(arch_spin_lock_t* lock,
                                       spin_tracing::EncodedLockId encoded_lock_id) TA_ACQ(lock);

inline void arch_spin_lock(arch_spin_lock_t* lock) TA_ACQ(lock) {
  if constexpr (kSchedulerLockSpinTracingEnabled) {
    // If someone is invoking this method directly (instead of using the
    // SpinLockBase wrapper), we have no access to a unique pre-encoded lock ID.
    // We just make one up instead, using our pointer as the unique lock ID, and
    // omitting a lock class ID.  This is not great, but it is the best we can
    // do here (and people _should_ be using the wrappers anyway).
    spin_tracing::EncodedLockId elid{spin_tracing::LockType::kSpinlock,
                                     reinterpret_cast<uint64_t>(lock),
                                     fxt::InternedString::kInvalidId};
    arch_spin_lock_trace_instrumented(lock, elid);
  } else {
    arch_spin_lock_non_instrumented(lock);
  }
}

// Note: trylock operations are not permitted to fail spuriously, even on
// architectures with weak memory ordering.  If a trylock operation fails, it
// must be because the lock was actually observed to be held by another thread
// during the attempt.
bool arch_spin_trylock(arch_spin_lock_t* lock) TA_TRY_ACQ(false, lock);
void arch_spin_unlock(arch_spin_lock_t* lock) TA_REL(lock);

inline cpu_num_t arch_spin_lock_holder_cpu(const arch_spin_lock_t* lock) {
  return lock->value.load(ktl::memory_order_relaxed) - 1;
}

// Returns true if the current CPU holds the lock. In practice, this value is
// only useful if migration to another CPU is prevented, by disabling preemption
// or interrupts, while the value is being used.
//
// Note that this predicate does not synchronize other memory operations (i.e.
// it has relaxed atomic semantics).
inline bool arch_spin_lock_held(const arch_spin_lock_t* lock) {
  return arch_spin_lock_holder_cpu(lock) == arch_curr_cpu_num();
}

// Returns true if unlocked (i.e. not held by any CPU). Primarity useful for
// asserting that locks are not being accessed incorrectly in destructors and
// cleanup paths.
//
// Note that this predicate does not synchronize other memory operations (i.e.
// it has relaxed atomic semantics).
inline bool arch_spin_lock_unlocked(const arch_spin_lock_t* lock) {
  return lock->value.load(ktl::memory_order_relaxed) == 0;
}

#endif  // ZIRCON_KERNEL_ARCH_INCLUDE_ARCH_SPINLOCK_H_
