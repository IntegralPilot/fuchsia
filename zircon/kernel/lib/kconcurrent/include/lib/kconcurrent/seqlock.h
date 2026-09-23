// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_KCONCURRENT_INCLUDE_LIB_KCONCURRENT_SEQLOCK_H_
#define ZIRCON_KERNEL_LIB_KCONCURRENT_INCLUDE_LIB_KCONCURRENT_SEQLOCK_H_

#include <lib/concurrent/seqlock.h>

#include <kernel/lock_validation_guard.h>
#include <kernel/spinlock.h>
#include <lockdep/lockdep.h>

namespace internal {

struct FuchsiaKernelOsal {
  static zx_instant_mono_t GetClockMonotonic() { return current_mono_time(); }
  static void ArchYield() { arch::Yield(); }
};

}  // namespace internal

template <::concurrent::SyncOpt kSyncOpt = ::concurrent::SyncOpt::AcqRelOps>
using SeqLock = ::concurrent::internal::SeqLock<::internal::FuchsiaKernelOsal, kSyncOpt>;
using SeqLockFenceSync = SeqLock<::concurrent::SyncOpt::Fence>;

// A small wrapper for read transaction tokens which make them a bit easier to
// declare, especially when using SeqLocks which have been instrumented with
// lockdep.
//
// Given two SeqLock instances, one wrapped in lockdep wrappers, and one not:
// ```
// SeqLock<> lock;
// DECLARE_SINGLETON_SEQLOCK(wrapped_lock);
// ```
//
// Instead of saying:
// ```
// typename decltype(lock)::ReadTransactionToken token;
// typename decltype(lock)::LockType::ReadTransactionToken wrapped_token;
// ```
//
// Users can also simply say:
// ```
// SeqLockReadTransactionToken token{lock};
// SeqLockReadTransactionToken wrapped_token{wrapped_lock};
// ```
//
// `token` and `wrapped_token` will deduce the proper type for the token based
// on the lock instance, and generate a class which publicly inherits from that
// deduced type, allowing it to be used with the lock's routines).
//
template <typename SeqLockType>
class SeqLockReadTransactionToken : public SeqLockType::ReadTransactionToken {
 public:
  SeqLockReadTransactionToken(const SeqLockType&) {}

  template <typename Class, typename LockType, size_t Index, ::lockdep::LockFlags Flags>
  SeqLockReadTransactionToken(const ::lockdep::LockDep<Class, LockType, Index, Flags>&);
};

template <typename Class, typename LockType, size_t Index, ::lockdep::LockFlags Flags>
SeqLockReadTransactionToken(const ::lockdep::LockDep<Class, LockType, Index, Flags>&)
    -> SeqLockReadTransactionToken<LockType>;

namespace SeqLockPolicy {

template <::concurrent::SyncOpt kSyncOpt>
struct ExclusiveIrqSave {
  struct State {
    interrupt_saved_state_t interrupt_state;
    bool blocking_disallow_state;
  };

  // The lock list and validation is made atomic by interrupt disable in pre-validation.
  using ValidationGuard = lockdep::NullValidationGuard;

  static void PreValidate(SeqLock<kSyncOpt>* lock, State* state) {
    state->interrupt_state = arch_interrupt_save();
    state->blocking_disallow_state = arch_blocking_disallowed();
    arch_set_blocking_disallowed(true);
  }

  static bool Acquire(SeqLock<kSyncOpt>* lock, State*) TA_ACQ(lock) {
    lock->Acquire();
    return true;
  }

  static void Release(SeqLock<kSyncOpt>* lock, State* state) TA_REL(lock) {
    lock->Release();
    arch_set_blocking_disallowed(state->blocking_disallow_state);
    arch_interrupt_restore(state->interrupt_state);
  }
};

template <::concurrent::SyncOpt kSyncOpt>
struct ExclusiveNoIrqSave {
  struct State {
    bool blocking_disallow_state;
  };

  // The lock list and validation is made atomic by interrupt disable before locking.
  using ValidationGuard = lockdep::NullValidationGuard;

  static void PreValidate(SeqLock<kSyncOpt>* lock, State* state) {
    DEBUG_ASSERT(arch_ints_disabled());
    state->blocking_disallow_state = arch_blocking_disallowed();
    arch_set_blocking_disallowed(true);
  }

  static bool Acquire(SeqLock<kSyncOpt>* lock, State*) TA_ACQ(lock) {
    lock->Acquire();
    return true;
  }

  static void Release(SeqLock<kSyncOpt>* lock, State* state) TA_REL(lock) {
    lock->Release();
    arch_set_blocking_disallowed(state->blocking_disallow_state);
  }
};

template <::concurrent::SyncOpt kSyncOpt>
struct SharedIrqSave {
  struct Shared {};  // Type tag indicating that this policy gives shared (not exclusive) access.
  struct State {
    State(bool& tgt) : result_target(tgt) { result_target = false; }
    bool& result_target;
    typename SeqLock<kSyncOpt>::ReadTransactionToken token;
    interrupt_saved_state_t interrupt_state;
  };

  using ValidationGuard = lockdep::NullValidationGuard;

  // The lock list and validation is made atomic by interrupt disable in pre-validation.
  static void PreValidate(SeqLock<kSyncOpt>* lock, State* state) {
    state->interrupt_state = arch_interrupt_save();
  }

  static bool Acquire(SeqLock<kSyncOpt>* lock, State* state) TA_ACQ_SHARED(lock) {
    state->token = lock->BeginReadTransaction();
    return true;
  }

  static void Release(SeqLock<kSyncOpt>* lock, State* state) TA_REL_SHARED(lock) {
    state->result_target = lock->EndReadTransaction(state->token);
    arch_interrupt_restore(state->interrupt_state);
  }
};

template <::concurrent::SyncOpt kSyncOpt>
struct SharedNoIrqSave {
  struct Shared {};  // Type tag indicating that this policy gives shared (not exclusive) access.
  struct State {
    State(bool& tgt) : result_target(tgt) { result_target = false; }
    bool& result_target;
    typename SeqLock<kSyncOpt>::ReadTransactionToken token;
  };

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  static void PreValidate(SeqLock<kSyncOpt>* lock, State* state) {}

  static bool Acquire(SeqLock<kSyncOpt>* lock, State* state) TA_ACQ_SHARED(lock) {
    state->token = lock->BeginReadTransaction();
    return true;
  }

  static void Release(SeqLock<kSyncOpt>* lock, State* state) TA_REL_SHARED(lock) {
    state->result_target = lock->EndReadTransaction(state->token);
  }
};
}  // namespace SeqLockPolicy

struct ExclusiveIrqSave {};
struct ExclusiveNoIrqSave {};
struct SharedIrqSave {};
struct SharedNoIrqSave {};

LOCK_DEP_TRAITS(SeqLock<::concurrent::SyncOpt::AcqRelOps>, lockdep::LockFlagsIrqSafe);
LOCK_DEP_POLICY_OPTION(SeqLock<::concurrent::SyncOpt::AcqRelOps>, ExclusiveIrqSave,
                       SeqLockPolicy::ExclusiveIrqSave<::concurrent::SyncOpt::AcqRelOps>);
LOCK_DEP_POLICY_OPTION(SeqLock<::concurrent::SyncOpt::AcqRelOps>, ExclusiveNoIrqSave,
                       SeqLockPolicy::ExclusiveNoIrqSave<::concurrent::SyncOpt::AcqRelOps>);
LOCK_DEP_POLICY_OPTION(SeqLock<::concurrent::SyncOpt::AcqRelOps>, SharedIrqSave,
                       SeqLockPolicy::SharedIrqSave<::concurrent::SyncOpt::AcqRelOps>);
LOCK_DEP_POLICY_OPTION(SeqLock<::concurrent::SyncOpt::AcqRelOps>, SharedNoIrqSave,
                       SeqLockPolicy::SharedNoIrqSave<::concurrent::SyncOpt::AcqRelOps>);

LOCK_DEP_TRAITS(SeqLock<::concurrent::SyncOpt::Fence>, lockdep::LockFlagsIrqSafe);
LOCK_DEP_POLICY_OPTION(SeqLock<::concurrent::SyncOpt::Fence>, ExclusiveIrqSave,
                       SeqLockPolicy::ExclusiveIrqSave<::concurrent::SyncOpt::Fence>);
LOCK_DEP_POLICY_OPTION(SeqLock<::concurrent::SyncOpt::Fence>, ExclusiveNoIrqSave,
                       SeqLockPolicy::ExclusiveNoIrqSave<::concurrent::SyncOpt::Fence>);
LOCK_DEP_POLICY_OPTION(SeqLock<::concurrent::SyncOpt::Fence>, SharedIrqSave,
                       SeqLockPolicy::SharedIrqSave<::concurrent::SyncOpt::Fence>);
LOCK_DEP_POLICY_OPTION(SeqLock<::concurrent::SyncOpt::Fence>, SharedNoIrqSave,
                       SeqLockPolicy::SharedNoIrqSave<::concurrent::SyncOpt::Fence>);

#define DECLARE_SEQLOCK_ACQREL_SYNC(containing_type, ...) \
  LOCK_DEP_INSTRUMENT(containing_type, SeqLock<::concurrent::SyncOpt::AcqRelOps>, ##__VA_ARGS__)

#define DECLARE_SINGLETON_SEQLOCK_ACQREL_SYNC(name, ...) \
  LOCK_DEP_SINGLETON_LOCK(name, SeqLock<::concurrent::SyncOpt::AcqRelOps>, ##__VA_ARGS__)

#define DECLARE_SEQLOCK_FENCE_SYNC(containing_type, ...) \
  LOCK_DEP_INSTRUMENT(containing_type, SeqLock<::concurrent::SyncOpt::Fence>, ##__VA_ARGS__)

#define DECLARE_SINGLETON_SEQLOCK_FENCE_SYNC(name, ...) \
  LOCK_DEP_SINGLETON_LOCK(name, SeqLock<::concurrent::SyncOpt::Fence>, ##__VA_ARGS__)

#define DECLARE_SEQLOCK_EXPLICIT_SYNC(containing_type, sync_type, ...) \
  LOCK_DEP_INSTRUMENT(containing_type, SeqLock<sync_type>, ##__VA_ARGS__)

#define DECLARE_SINGLETON_SEQLOCK_EXPLICIT_SYNC(name, sync_type, ...) \
  LOCK_DEP_SINGLETON_LOCK(name, SeqLock<sync_type>, ##__VA_ARGS__)

// Default to Acq/Rel sync
#define DECLARE_SEQLOCK(T, ...) DECLARE_SEQLOCK_ACQREL_SYNC(T, ##__VA_ARGS__)
#define DECLARE_SINGLETON_SEQLOCK(T, ...) DECLARE_SINGLETON_SEQLOCK_ACQREL_SYNC(T, ##__VA_ARGS__)

template <typename T, typename LockDepWrapper>
using SeqLockPayload = ::concurrent::SeqLockPayload<T, typename LockDepWrapper::LockType>;

#endif  // ZIRCON_KERNEL_LIB_KCONCURRENT_INCLUDE_LIB_KCONCURRENT_SEQLOCK_H_
