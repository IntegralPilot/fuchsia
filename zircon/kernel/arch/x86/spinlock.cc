// Copyright 2019 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/intrin.h>

#include <arch/arch_ops.h>
#include <arch/spinlock.h>
#include <kernel/spin_tracing.h>

namespace {

void assert_lock_held(arch_spin_lock_t *lock) TA_ASSERT(lock) {}

inline void arch_spin_lock_core(arch_spin_lock_t *lock, cpu_num_t val) TA_ACQ(lock) {
  cpu_num_t expected = 0;
  while (unlikely(!lock->value.compare_exchange_weak(expected, val, ktl::memory_order_acquire,
                                                     ktl::memory_order_relaxed))) {
    expected = 0;
    do {
      arch::Yield();
    } while (unlikely(lock->value.load(ktl::memory_order_relaxed) != 0));
  }

  assert_lock_held(lock);
}

}  // namespace

void arch_spin_lock_non_instrumented(arch_spin_lock_t *lock) TA_ACQ(lock) {
  struct x86_percpu *percpu = x86_get_percpu();
  cpu_num_t val = percpu->cpu_num + 1;
  arch_spin_lock_core(lock, val);
  percpu->num_spinlocks++;
}

void arch_spin_lock_trace_instrumented(arch_spin_lock_t *lock,
                                       spin_tracing::EncodedLockId encoded_lock_id) TA_ACQ(lock) {
  struct x86_percpu *percpu = x86_get_percpu();
  cpu_num_t val = percpu->cpu_num + 1;

  // If this lock acquisition is trace instrumented, try to obtain the lock once
  // before we decide that we need to spin and produce spin trace events.
  cpu_num_t expected = 0;
  lock->value.compare_exchange_weak(expected, val, ktl::memory_order_acquire,
                                    ktl::memory_order_relaxed);
  if (expected == 0) {
    percpu->num_spinlocks++;
    assert_lock_held(lock);
    return;
  }

  spin_tracing::Tracer<true> spin_tracer;
  arch_spin_lock_core(lock, val);
  spin_tracer.Finish(spin_tracing::FinishType::kLockAcquired, encoded_lock_id);

  percpu->num_spinlocks++;
}

bool arch_spin_trylock(arch_spin_lock_t *lock) TA_NO_THREAD_SAFETY_ANALYSIS {
  struct x86_percpu *percpu = x86_get_percpu();
  cpu_num_t val = percpu->cpu_num + 1;

  cpu_num_t expected = 0;

  lock->value.compare_exchange_strong(expected, val, ktl::memory_order_acquire,
                                      ktl::memory_order_relaxed);
  if (expected == 0) {
    percpu->num_spinlocks++;
  }

  return expected != 0;
}

void arch_spin_unlock(arch_spin_lock_t *lock) TA_NO_THREAD_SAFETY_ANALYSIS {
  x86_get_percpu()->num_spinlocks--;
  lock->value.store(0, ktl::memory_order_release);
}
