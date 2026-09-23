// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_KCONCURRENT_INCLUDE_LIB_KCONCURRENT_CHAINLOCK_TRANSACTION_H_
#define ZIRCON_KERNEL_LIB_KCONCURRENT_INCLUDE_LIB_KCONCURRENT_CHAINLOCK_TRANSACTION_H_

#include <assert.h>
#include <lib/arch/intrin.h>
#include <lib/concurrent/chainlock_transaction.h>
#include <lib/kconcurrent/chainlock.h>
#include <lib/ktrace.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <arch/arch_interrupt.h>
#include <arch/mp.h>
#include <arch/ops.h>
#include <kernel/cpu.h>
#include <kernel/percpu.h>
#include <kernel/preempt_disabled_token.h>
#include <kernel/thread.h>
#include <ktl/atomic.h>
#include <ktl/functional.h>
#include <ktl/type_traits.h>
#include <platform/timer.h>

// Implementations of the methods and member types of ChainLockTransactin declared in
// lib/kconcurrent/chainlock.h. This separation avoids circular dependencies with kernel/thread.h,
// which uses these types and also provides some of the facilities the following definitions use.

// While not strictly necessary, the constructors and destructor are defined inline here to make it
// easy to add trace instrumentation as needed.
inline ChainLockTransaction::ChainLockTransaction(CallsiteInfo callsite_info)
    : Base{callsite_info} {}

inline ChainLockTransaction::ChainLockTransaction(CallsiteInfo callsite_info, uint32_t locks_held,
                                                  bool finalized)
    : Base{callsite_info, locks_held, finalized} {}

inline ChainLockTransaction::~ChainLockTransaction() {}

inline ChainLockTransaction* ChainLockTransaction::Active() {
  DEBUG_ASSERT(arch_ints_disabled());
  return arch_get_curr_percpu()->active_cl_transaction;
}

inline void ChainLockTransaction::SetActive(ChainLockTransaction* transaction) {
  DEBUG_ASSERT(arch_ints_disabled());
  arch_get_curr_percpu()->active_cl_transaction = transaction;
}

inline ChainLockTransaction::Token ChainLockTransaction::AllocateReservedToken() {
  DEBUG_ASSERT(arch_ints_disabled());
  return ReservedToken(arch_curr_cpu_num());
}

inline zx_instant_mono_ticks_t ChainLockTransaction::GetCurrentTicks() {
  return current_mono_ticks();
}

inline bool ChainLockTransaction::RecordBackoffOrRetry(Token observed_token,
                                                       ktl::atomic<Token>& lock_state,
                                                       ContentionState& contention_state) {
  percpu* percpu = arch_get_curr_percpu();
  const cpu_num_t current_cpu = arch_curr_cpu_num();
  const cpu_mask_t current_cpu_mask = cpu_num_to_mask(current_cpu);

  // Observe the conflict id for the current cpu and add this cpu to the contention state of the
  // chain lock instigating the backoff operation.
  const uint64_t conflict_id = percpu->chain_lock_conflict_id.load(ktl::memory_order_acquire);
  contention_state.fetch_or(current_cpu_mask, ktl::memory_order_acq_rel);

  // If the lock state has the same token after observing the conflict id, this transaction needs to
  // wait until the conflict is (potentially) resolved by the owner incrementing the conflict id
  // when releasing the lock.
  const Token current_token = lock_state.load(ktl::memory_order_acquire);
  if (current_token == observed_token) {
    conflict_id_ = conflict_id;
    return true;
  }

  return false;
}

template <typename Callable>
inline void ChainLockTransaction::ReleaseAndNotify(ContentionState& contention_state,
                                                   Callable&& do_release) {
  // Sample the contention state before releasing the chainlock. The caller will ensure that
  // contention state will not be modified until after do_release is called.
  cpu_mask_t wake_mask = contention_state.exchange(0, ktl::memory_order_acq_rel);

  // Perform the actual chainlock release operation, allowing contenders to attempt to acquire the
  // lock. The contention state MUST NOT be accessed after the release to avoid use-after-free
  // hazards!!
  ktl::invoke(do_release);

  // Use the sampled contention state to poke any backed-off contenders.
  for (cpu_num_t cpu = 0; wake_mask; ++cpu, wake_mask >>= 1) {
    if (wake_mask & 1) {
      percpu::Get(cpu).chain_lock_conflict_id.fetch_add(1, ktl::memory_order_release);
    }
  }
}

inline void ChainLockTransaction::WaitForConflictResolution(percpu* percpu) {
  if (conflict_id_ != 0) {
    // Wait for the contention to be resolved on the CPU where the contention by this transaction
    // was originally observed. That might be a different CPU than the current thread is on, since
    // the thread might be able to migrate during the relax phase of transaction backoff.
    do {
      arch::Yield();
    } while (percpu->chain_lock_conflict_id.load(ktl::memory_order_acquire) == conflict_id_);
    conflict_id_ = 0;
  } else {
    arch::Yield();
  }
}

inline void ChainLockTransaction::OnFinalized(zx_instant_mono_ticks_t contention_start_ticks,
                                              CallsiteInfo callsite_info) {
  KTRACE_COMPLETE("kernel:sched", "lock_spin", contention_start_ticks,
                  ("lock_id", callsite_info.line_number), ("lock_class", *callsite_info.label),
                  ("lock_type", "ChainLockTransaction"_intern),
                  ("backoff_count", Active()->backoff_count()));
  UpdateContentionCounters(current_mono_ticks() - contention_start_ticks);
}

template <>
struct ChainLockTransaction::StateSaver<ChainLockTransaction::StateOptions::NoIrqSave> {
  void Relax(ChainLockTransaction& transaction) {
    transaction.WaitForConflictResolution(arch_get_curr_percpu());
  }
};

template <>
struct ChainLockTransaction::StateSaver<ChainLockTransaction::StateOptions::IrqSave> {
  StateSaver() : interrupt_state{arch_interrupt_save()} {}
  ~StateSaver() { arch_interrupt_restore(interrupt_state); }

  void Relax(ChainLockTransaction& transaction) {
    // Must be latched before enabling interrupts so that the correct contention state is monitored,
    // even if the thread migrates during relax.
    percpu* percpu = arch_get_curr_percpu();

    transaction.Unregister();
    arch_interrupt_restore(interrupt_state);

    transaction.WaitForConflictResolution(percpu);

    interrupt_state = arch_interrupt_save();
    transaction.Register();
  }

  interrupt_saved_state_t interrupt_state;
};

template <>
struct ChainLockTransaction::StateSaver<ChainLockTransaction::StateOptions::PreemptDisable> {
  StateSaver() TA_ACQ(preempt_disabled_token) {
    Thread::Current::preemption_state().PreemptDisable();
    interrupt_state = arch_interrupt_save();
  }
  ~StateSaver() TA_REL(preempt_disabled_token) {
    arch_interrupt_restore(interrupt_state);
    Thread::Current::preemption_state().PreemptReenable();
  }

  void Relax(ChainLockTransaction& transaction) {
    // Must be latched before enabling interrupts/preemption so that the correct contention state is
    // monitored, even if the thread migrates during relax.
    percpu* percpu = arch_get_curr_percpu();

    transaction.Unregister();
    arch_interrupt_restore(interrupt_state);
    Thread::Current::preemption_state().PreemptReenable();

    transaction.WaitForConflictResolution(percpu);

    Thread::Current::preemption_state().PreemptDisable();
    interrupt_state = arch_interrupt_save();
    transaction.Register();
  }

  interrupt_saved_state_t interrupt_state;
};

template <>
struct ChainLockTransaction::StateSaver<ChainLockTransaction::StateOptions::EagerReschedDisable> {
  StateSaver() TA_ACQ(preempt_disabled_token) {
    Thread::Current::preemption_state().EagerReschedDisable();
    interrupt_state = arch_interrupt_save();
  }
  ~StateSaver() TA_REL(preempt_disabled_token) {
    arch_interrupt_restore(interrupt_state);
    Thread::Current::preemption_state().EagerReschedReenable();
  }

  void Relax(ChainLockTransaction& transaction) {
    // Must be latched before enabling interrupts/eager resched so that the correct contention state
    // is monitored, even if the thread migrates during relax.
    percpu* percpu = arch_get_curr_percpu();

    transaction.Unregister();
    arch_interrupt_restore(interrupt_state);
    Thread::Current::preemption_state().EagerReschedReenable();

    transaction.WaitForConflictResolution(percpu);

    Thread::Current::preemption_state().EagerReschedDisable();
    interrupt_state = arch_interrupt_save();
    transaction.Register();
  }

  interrupt_saved_state_t interrupt_state;
};

template <ChainLockTransaction::StateOptions option>
class TA_SCOPED_CAP SingleChainLockGuard {
 public:
  SingleChainLockGuard(ChainLockTransaction::Option<option>, ChainLock& lock,
                       ChainLockTransaction::CallsiteInfo callsite_info)
      TA_ACQ(chainlock_transaction_token, lock)
      : transaction_{callsite_info}, guard_(lock) {
    ChainLockTransaction::Finalize();
  }
  ~SingleChainLockGuard() TA_REL() {}

 private:
  // The order of construction/destruction of these members is important. State must be saved before
  // starting the transaction and the transaction must be started before acquiring the lock with the
  // guard, matching the order that constructors are executed in. Likewise, the guard needs to be
  // released before completing the transaction and the transaction needs to be completed before
  // restoring state, matching the order that the destructors are executed in.
  ChainLockTransaction::StateSaver<option> state_saver_;
  ChainLockTransaction transaction_;
  ChainLockGuard guard_;
};

template <ChainLockTransaction::StateOptions option>
SingleChainLockGuard(ChainLockTransaction::Option<option>, ChainLock&,
                     ChainLockTransaction::CallsiteInfo) -> SingleChainLockGuard<option>;

#endif  // ZIRCON_KERNEL_LIB_KCONCURRENT_INCLUDE_LIB_KCONCURRENT_CHAINLOCK_TRANSACTION_H_
