// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_NULL_LOCK_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_NULL_LOCK_H_

#include <lib/zircon-internal/thread_annotations.h>

#include <fbl/null_lock.h>
#include <kernel/lock_validation_guard.h>
#include <kernel/lockdep.h>

using NullLock = fbl::NullLock;

struct NullLockPolicy {
  struct State {};

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  template <typename LockType>
  static void PreValidate(LockType*, State*) {}

  template <typename LockType>
  static bool Acquire(LockType* lock, State* state) TA_ACQ(lock) {
    lock->Acquire();
    return true;
  }

  template <typename LockType>
  static void Release(LockType* lock, State* state) TA_REL(lock) {
    lock->Release();
  }

  template <typename LockType>
  static void AssertHeld(const LockType& lock) TA_ASSERT(lock) {}
};

LOCK_DEP_TRAITS(NullLock, lockdep::LockFlagsNone);
LOCK_DEP_POLICY(NullLock, NullLockPolicy);

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_NULL_LOCK_H_
