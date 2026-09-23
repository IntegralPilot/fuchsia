// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_CONCURRENT_CHAINLOCK_GUARD_H_
#define LIB_CONCURRENT_CHAINLOCK_GUARD_H_

#include <lib/concurrent/chainlock.h>
#include <zircon/compiler.h>

namespace concurrent {

// ChainLockGuard is a RAII style acquire/release guard for chain locks.
//
// TODO(eieio): Figure out if this type can be merged with lockdep::Guard to provide some level of
// runtime consistency analysis between chain locks and other lock classes. Chain locks are
// essentially externally ordered locks, where the acquisition of any number of chain locks behaves
// as acquiring a single lock class, having its order dependencies with other lock classes tracked.

template <typename Transaction>
class __TA_SCOPED_CAPABILITY ChainLockGuard {
 public:
  // Unconditionally acquires the given chain lock.
  __WARN_UNUSED_CONSTRUCTOR explicit ChainLockGuard(ChainLock<Transaction>& lock) __TA_ACQUIRE(lock)
      : lock_{lock}, held_{true} {
    lock.AcquireFirstInChain();
  }

  // Adopts the given chain lock that is already acquired via some other mechanism.
  enum AdoptTag { Adopt };
  __WARN_UNUSED_CONSTRUCTOR ChainLockGuard(ChainLock<Transaction>& lock, AdoptTag)
      __TA_REQUIRES(lock)
      : lock_{lock}, held_{true} {}

  // Associates the given lock capability with this scoped capability, but does not acquire it. This
  // is useful in conjunction with the conditional acquire methods below.
  //
  // Example:
  //
  // ChainLockGuard guard{lock, ChainLockGuard::Defer};
  // if (guard.AcquireOrBackoff()) {
  //   // Lock is acquired and guarded. It will be released when guard goes out of scope.
  // } else {
  //   // Lock is not acquired or guarded. It will not be released when guard goes out of scope.
  // }
  //
  // NOTE: This method is correctly annotated for static analysis to understand when this scoped
  // capability does and does not hold the lock. The excludes annotation on the constructor tells
  // static analysis which capability to associate with the scoped capability instance in the "not
  // held" state. Later, methods annotated with acquire or release can change the state to "held"
  // and back to "not held", respectively.
  enum DeferTag { Defer };
  __WARN_UNUSED_CONSTRUCTOR ChainLockGuard(ChainLock<Transaction>& lock, DeferTag)
      __TA_EXCLUDES(lock)
      : lock_{lock}, held_{false} {}

  // Takes responsibility for the given lock from the given guard. This is useful for moving the
  // scope of a lock acquisition deeper into the call stack in certain locking patterns.
  //
  // Asserts that the given guard actually holds the given lock.
  enum TakeTag { Take };
  __WARN_UNUSED_CONSTRUCTOR ChainLockGuard(ChainLockGuard&& other, ChainLock<Transaction>& lock,
                                           TakeTag) __TA_ACQUIRE(lock)
      : lock_{lock}, held_{true} {
    ZX_DEBUG_ASSERT(other.held_);
    ZX_DEBUG_ASSERT(&other.lock_ == &lock_);
    other.held_ = false;
  }

  ChainLockGuard(const ChainLockGuard&) = delete;
  ChainLockGuard& operator=(const ChainLockGuard&) = delete;
  ChainLockGuard(ChainLockGuard&&) = delete;
  ChainLockGuard& operator=(ChainLockGuard&&) = delete;

  // Releases the capability guarded by this instance without actually releasing the lock. This is
  // intended to be used with the Take constructor, which acquires the capability, to move
  // responsibility for the lock to another guard.
  [[nodiscard]] ChainLockGuard&& take() __TA_RELEASE() {
    ZX_DEBUG_ASSERT(held_);
    return std::move(*this);
  }

  // Conditionally acquires the lock or returns false because of backoff.
  bool AcquireOrBackoff() __TA_TRY_ACQUIRE(true) {
    ZX_DEBUG_ASSERT(!held_);
    held_ = lock_.AcquireOrBackoff();
    return held_;
  }

  using AllowFinalized = typename ChainLock<Transaction>::AllowFinalized;
  using RecordBackoff = typename ChainLock<Transaction>::RecordBackoff;

  // Conditionally acquires the lock or returns false if failed.
  bool TryAcquire(AllowFinalized allow_finalized = AllowFinalized::No,
                  RecordBackoff record_backoff = RecordBackoff::Yes) __TA_TRY_ACQUIRE(true) {
    ZX_DEBUG_ASSERT(!held_);
    held_ = lock_.TryAcquire(allow_finalized, record_backoff);
    return held_;
  }

  // Conditionally acquires the lock, using the given callable, or returns false if failed. This is
  // useful in certain locking patterns where an additional predicate needs to be injected into the
  // lock condition.
  template <typename Callable>
  bool TryAcquireWith(Callable&& callable) __TA_TRY_ACQUIRE(true) {
    ZX_DEBUG_ASSERT(!held_);
    held_ = std::forward<Callable>(callable)(lock_);
    return held_;
  }

  // Releases the guarded lock and its capability.
  void Release() __TA_RELEASE() {
    ZX_DEBUG_ASSERT(held_);
    lock_.Release();
    held_ = false;
  }

  // Prevents this guard from releasing the lock when it goes out of scope, but does not release the
  // lock or its capability. This is a temporary workaround to allow guards to be used in scopes
  // that will release the guarded lock in a function or method called in the guarded scope.
  //
  // Example:
  //
  // {
  //   ChainLockGuard guard{object->get_lock()};
  //
  //   // Validation with lock held ...
  //
  //   guard.Unguard(); // Avoid double release when guard destructs.
  //   DoOperationAndReleaseLock(object);
  // }
  //
  // Possible alternatives to explore that could replace this workaround:
  //
  // 1. Move the guard into the callee that will release the lock:
  //
  //   DoOperationAndReleaseLock(object, guard.take());
  //
  // 2. Make the guard destructor check that that lock is actually held to avoid the double release:
  //
  //   ~ChainLockGuard() __TA_RELEASE() {
  //     if (held_ && lock_.is_held()) {
  //       lock_.Release();
  //     }
  //   }
  //
  // All of these approaches have potential advantages and disadvantages that need to be considered.
  //
  void Unguard() {
    ZX_DEBUG_ASSERT(held_);
    held_ = false;
  }

  // Releases the lock if currently held.
  ~ChainLockGuard() __TA_RELEASE() {
    if (held_) {
      lock_.AssertHeld();
      lock_.Release();
    }
  }

 private:
  ChainLock<Transaction>& lock_;
  bool held_;
};

// Deduction guides that allow the Transaction type to be deduced from the ChainLock instantiation
// passed to the constructor.
//
// Example:
//
// using MyChainLock = concurrent::ChainLock<MyTransaction>;
// using concurrent::ChainLockGuard;
//
// MyChainLock lock;
//
// ChainLockGuard guard{lock}; // Deduces ChainLockGuard<MyTransaction> from the lock argument.
//
template <typename Transaction>
ChainLockGuard(ChainLock<Transaction>&) -> ChainLockGuard<Transaction>;

template <typename Transaction>
ChainLockGuard(ChainLock<Transaction>& lock,
               typename ChainLockGuard<Transaction>::AdoptTag) -> ChainLockGuard<Transaction>;

template <typename Transaction>
ChainLockGuard(ChainLock<Transaction>& lock,
               typename ChainLockGuard<Transaction>::DeferTag) -> ChainLockGuard<Transaction>;

template <typename Transaction>
ChainLockGuard(ChainLock<Transaction>& lock,
               typename ChainLockGuard<Transaction>::TakeTag) -> ChainLockGuard<Transaction>;

}  // namespace concurrent

#endif  // LIB_CONCURRENT_CHAINLOCK_GUARD_H_
