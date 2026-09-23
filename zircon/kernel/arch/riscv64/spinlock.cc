// Copyright 2023 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <arch/ops.h>
#include <arch/spinlock.h>
#include <kernel/ffi.h>
#include <kernel/spin_tracing.h>
#include <ktl/atomic.h>

extern "C" {

void rust_arch_spin_lock_non_instrumented(uint32_t* lock);
bool rust_arch_spin_trylock(uint32_t* lock);
void rust_arch_spin_unlock(uint32_t* lock);

}  // extern "C"

namespace {
void on_lock_acquired(arch_spin_lock_t* lock) TA_ASSERT(lock) {}
}  // namespace

FFI_ALWAYS_INLINE void arch_spin_lock_non_instrumented(arch_spin_lock_t* lock) {
  rust_arch_spin_lock_non_instrumented(reinterpret_cast<uint32_t*>(&lock->value));
  on_lock_acquired(lock);
}

FFI_ALWAYS_INLINE void arch_spin_lock_trace_instrumented(
    arch_spin_lock_t* lock, spin_tracing::EncodedLockId encoded_lock_id) {
  if (rust_arch_spin_trylock(reinterpret_cast<uint32_t*>(&lock->value))) {
    on_lock_acquired(lock);
    return;
  }

  spin_tracing::Tracer<true> spin_tracer;
  rust_arch_spin_lock_non_instrumented(reinterpret_cast<uint32_t*>(&lock->value));
  spin_tracer.Finish(spin_tracing::FinishType::kLockAcquired, encoded_lock_id);
  on_lock_acquired(lock);
}

static_assert(sizeof(arch_spin_lock_t) == 4);
static_assert(alignof(arch_spin_lock_t) == 4);

FFI_ALWAYS_INLINE bool arch_spin_trylock(arch_spin_lock_t* lock) TA_NO_THREAD_SAFETY_ANALYSIS {
  // Returns false if the lock was acquired, true if it was not (matching TA_TRY_ACQ(false)).
  return !rust_arch_spin_trylock(reinterpret_cast<uint32_t*>(&lock->value));
}

FFI_ALWAYS_INLINE void arch_spin_unlock(arch_spin_lock_t* lock) TA_NO_THREAD_SAFETY_ANALYSIS {
  rust_arch_spin_unlock(reinterpret_cast<uint32_t*>(&lock->value));
}
