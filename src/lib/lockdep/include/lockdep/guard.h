// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_GUARD_H_
#define SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_GUARD_H_

#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <type_traits>
#include <utility>

#include <lockdep/common.h>
#include <lockdep/lock_class.h>
#include <lockdep/lock_policy.h>
#include <lockdep/lock_traits.h>

namespace lockdep {

namespace internal {

// Utility to deduce the LockType given any subclass of Lock<LockType>.
template <typename LockType>
LockType DeduceLockType(Lock<LockType>*);

// Determines the LockType of any subclass of Lock<LockType> or
// Lock<GlobalReference<LockType, Reference>>.
template <typename T>
using GetLockType = RemoveGlobalReference<decltype(DeduceLockType(static_cast<T*>(nullptr)))>;

// Trait type that determines whether the given LockType is nestable.
template <typename LockType>
struct IsLockTypeNestable : std::bool_constant<(LockTraits<RemoveGlobalReference<LockType>>::Flags &
                                                LockFlagsNestable) != 0> {};

// Trait type that determines whether the given instrumented lock is nestable.
template <typename T>
struct IsLockNestable : std::false_type {};
template <typename Class, typename LockType, size_t Index, LockFlags Flags>
struct IsLockNestable<LockDep<Class, LockType, Index, Flags>>
    : std::bool_constant<(Flags & LockFlagsNestable) != 0> {};

// Trait type that determines whether the given policy type has a nested type
// tag named Shared.
template <typename LockPolicy, typename Enabled = void>
struct IsSharedLockPolicy : std::false_type {};
template <typename LockPolicy>
struct IsSharedLockPolicy<LockPolicy, std::void_t<typename LockPolicy::Shared>> : std::true_type {};

// Detect whether `LockPolicy<LockType>::AssertHeld(const LockType&)` is a valid expression.
template <typename LockType, typename = void>
struct PolicyHasAssertHeld : std::false_type {};
template <typename LockType>
struct PolicyHasAssertHeld<LockType, std::void_t<decltype(LockPolicy<LockType>::AssertHeld(
                                         std::declval<const LockType>()))>> : std::true_type {};

// Enable if the given LockType has a policy supporting `AssertHeld`.
template <typename LockType>
using EnableIfPolicyHasAssertHeld = std::enable_if_t<PolicyHasAssertHeld<LockType>::value>;

// Enable if the given T is nestable and uses same type as LockType.
template <typename T, typename LockType>
using EnableIfNestable =
    std::enable_if_t<std::is_same_v<GetLockType<T>, LockType> &&
                     std::disjunction_v<IsLockTypeNestable<LockType>, IsLockNestable<T>>>;

// Enable if the given T is not nestable and uses same type as LockType.
template <typename T, typename LockType>
using EnableIfNotNestable =
    std::enable_if_t<std::is_same_v<GetLockType<T>, LockType> &&
                     !std::disjunction_v<IsLockTypeNestable<LockType>, IsLockNestable<T>>>;

template <typename LockType, typename Option>
using EnableIfShared = std::enable_if_t<IsSharedLockPolicy<LockPolicy<LockType, Option>>::value>;

template <typename LockType, typename Option>
using EnableIfNotShared =
    std::enable_if_t<!IsSharedLockPolicy<LockPolicy<LockType, Option>>::value>;

}  // namespace internal

// Assert that the given lock is exclusively held by the current thread.
//
// Can be used both for runtime debugging checks, and also to help when
// thread safety analysis can't prove you are holding a lock. The underlying
// lock implementation may optimize away asserts in release builds.
//
// Calling this function requires that LockType has a policy implementing
// `AssertHeld`. The default policy automatically implements AssertHeld
// if the underlying lock object has an `AssertHeld` method.
template <typename Lockable, typename Option = void>
void AssertHeld(const Lockable& lock) __TA_ASSERT(lock) __TA_ASSERT(lock.lock())
    __TA_ASSERT(lock.capability()) {
  LockPolicy<internal::GetLockType<Lockable>, Option>::AssertHeld(lock.lock());
}

// Type tag to select the (private) ordered Guard constructor.
enum OrderedLockTag { OrderedLock };

// Type tag to select the ordered Guard constructor for type erased locks.
enum AssertOrderedLockTag { AssertOrderedLock };

// Type tag to select the adopting Guard constructor.
enum AdoptLockTag { AdoptLock };

// Type tag to select the aliased lock Guard constructor.
enum AliasedLockTag { AliasedLock };

// Type tag to select the aliased lock and ordered Guard constructor for type
// erased locks.
enum AssertOrderedAliasedLockTag { AssertOrderedAliasedLock };

// Base RAII type that automatically manages the duration of a lock acquisition.
// TODO(eieio): Specializations handle exclusive and shared lock acquisitions.
// These are largely identical except for the static lock analysis annotations.
// See if there is away to factor out the common logic for better
// maintainability.
template <typename LockType, typename Option = void, typename Enable = void>
class Guard;

// Specialization of Guard that acquires the given lock in exclusive mode.
template <typename LockType, typename Option>
class __TA_SCOPED_CAPABILITY
    Guard<LockType, Option, internal::EnableIfNotShared<LockType, Option>> {
  static_assert(!std::is_same<LockPolicy<LockType, Option>, AmbiguousOption>::value,
                "The Option argument of Guard<LockType, Option> must always "
                "be specified when the policy for LockType is defined using "
                "the macro LOCK_DEP_POLICY_OPTION(). See the macro docs for "
                "details.");

 public:
  static constexpr LockFlags kLockFlags = LockTraits<RemoveGlobalReference<LockType>>::Flags;

  Guard(Guard&&) = delete;
  Guard(const Guard&) = delete;
  Guard& operator=(Guard&&) = delete;
  Guard& operator=(const Guard&) = delete;

  // Acquires the given lock. This constructor participates in overload
  // resolution when the underlying lock type is not nestable.
  template <typename Lockable, typename... Args,
            typename = internal::EnableIfNotNestable<Lockable, LockType>>
  __WARN_UNUSED_CONSTRUCTOR Guard(Lockable* lock, Args&&... state_args) __TA_ACQUIRE(lock)
      __TA_ACQUIRE(lock->capability())
      : validator_{&lock->lock(), lock->id()}, state_{std::forward<Args>(state_args)...} {
    ValidateAndAcquire();
  }

  // Acquires the given lock. This constructor participates in overload
  // resolution when the underlying lock type is nestable.
  template <typename Lockable, typename... Args,
            typename = internal::EnableIfNestable<Lockable, LockType>>
  __WARN_UNUSED_CONSTRUCTOR Guard(Lockable* lock, uintptr_t order, Args&&... state_args)
      __TA_ACQUIRE(lock) __TA_ACQUIRE(lock->capability())
      : Guard{OrderedLock, lock, order, std::forward<Args>(state_args)...} {}

  // Acquires the given type erased lock. The caller asserts the underlying lock
  // type is nestable, which is verified by a runtime check in debug builds.
  template <typename... Args>
  __WARN_UNUSED_CONSTRUCTOR Guard(AssertOrderedLockTag, Lock<LockType>* lock, uintptr_t order,
                                  Args&&... state_args) __TA_ACQUIRE(lock)
      __TA_ACQUIRE(lock->capability())
      : Guard{OrderedLock, lock, order, std::forward<Args>(state_args)...} {
    ZX_DEBUG_ASSERT(!kLockValidationEnabled || ValidatorLockClassState::IsNestable(lock->id()));
  }

  // Acquires the given type erased lock and its alias. Only the first lock is
  // actually acquired, the alias is verified to be the same lock by a runtime
  // check and its capability is acquired to satisfy static analysis.
  //
  // This constructor participates in overload resolution when the underlying
  // lock type is not nestable.
  template <typename Lockable, typename... Args,
            typename = internal::EnableIfNotNestable<Lockable, LockType>>
  __WARN_UNUSED_CONSTRUCTOR Guard(AliasedLockTag, Lockable* lock, Lockable* alias,
                                  Args&&... state_args) __TA_ACQUIRE(lock)
      __TA_ACQUIRE(lock->capability()) __TA_ACQUIRE(alias) __TA_ACQUIRE(alias->capability())
      : validator_{&lock->lock(), lock->id()}, state_{std::forward<Args>(state_args)...} {
    ZX_DEBUG_ASSERT(lock == alias);
    ValidateAndAcquire();
  }

  // Acquires the given type erased lock and its alias. Only the first lock is
  // actually acquired, the alias is verified to be the same lock by a runtime
  // check and its capability is acquired to satisfy static analysis.
  //
  // This constructor participates in overload resolution when the underlying
  // lock type is nestable.
  template <typename Lockable, typename... Args,
            typename = internal::EnableIfNestable<Lockable, LockType>>
  __WARN_UNUSED_CONSTRUCTOR Guard(AliasedLockTag, Lockable* lock, Lockable* alias, uintptr_t order,
                                  Args&&... state_args) __TA_ACQUIRE(lock)
      __TA_ACQUIRE(lock->capability()) __TA_ACQUIRE(alias) __TA_ACQUIRE(alias->capability())
      : Guard{OrderedLock, lock, order, std::forward<Args>(state_args)...} {
    ZX_DEBUG_ASSERT(lock == alias);
  }

  // Acquires the given type erased lock and its alias. Only the first lock is
  // actually acquired, the alias is verified to be the same lock by a runtime
  // check and its capability is acquired to satisfy static analysis.
  //
  // The caller asserts the underlying lock type is nestable, which is verified
  // by a runtime check in debug builds.
  template <typename Lockable, typename... Args>
  __WARN_UNUSED_CONSTRUCTOR Guard(AssertOrderedAliasedLockTag, Lockable* lock, Lockable* alias,
                                  uintptr_t order, Args&&... state_args) __TA_ACQUIRE(lock)
      __TA_ACQUIRE(lock->capability()) __TA_ACQUIRE(alias) __TA_ACQUIRE(alias->capability())
      : Guard{OrderedLock, lock, order, std::forward<Args>(state_args)...} {
    ZX_DEBUG_ASSERT(lock == alias);
    ZX_DEBUG_ASSERT(!kLockValidationEnabled || ValidatorLockClassState::IsNestable(lock->id()));
  }

  // Destructor that automatically releases the lock if not already released.
  ~Guard() __TA_RELEASE() { Release(); }

  // Releases the lock early before this guard instance goes out of scope.
  //
  // Note: It is important to validate the release operation and clear the
  // validator state _before_ we actually release the lock.  Failure to observe
  // this ordering requirement can enable the following sequence.
  //
  // 1) A kernel spinlock is held with interrupts disabled.
  // 2) During the lock release operation, the lock is released and interrupts
  //    are re-enabled.
  // 3) Before the validation of the release operation has taken place and the
  //    state is cleared, and interrupt it taken.
  // 4) During the interrupt handler, the same lock is acquired.
  // 5) To lockdep, it looks like the lock is already currently held which
  //    results in a reentrancy violation, even though the lock has already been
  //    dropped.
  template <typename... Args>
  void Release(Args&&... args) __TA_RELEASE() {
    LockType* lock_ptr = validator_.lock();

    if (lock_ptr != nullptr) {
      validator_.ValidateRelease(/* clear */ true);
      LockPolicy<LockType, Option>::Release(lock_ptr, &state_, std::forward<Args>(args)...);
    }
  }

  // Returns true if the guard has an actively acquired lock.
  explicit operator bool() const { return validator_.lock() != nullptr; }

  // Returns true if this guard wraps |lock|.
  bool wraps_lock(const LockType& lock) const { return &lock == validator_.lock(); }

  // Read-only access to the underlying |lock|.
  const LockType* lock() const { return validator_.lock(); }

  class Adoptable;
  // Releases this scoped capability without releasing the underlying lock or
  // un-tracking the lock in the validator. Returns an object holding the
  // lock state and validator state which may be adopted by another Guard.
  // This is useful in the rare situation where a lock wants to be used and
  // released in a different scope, or if iterating over a data structure that
  // uses fine-grained locking.
  //
  // Example:
  //  Guard<fbl::Mutex> guard{&lock};
  //
  //  // Setup actions...
  //
  //  DoTaskAndReleaseLock(guard.take());
  //  or
  //  return guard.take();
  //
  Adoptable take() __TA_RELEASE() { return Adoptable(std::move(validator_), std::move(state_)); }

  // Adopts the lock state and validate state from a previous take() action.
  // This requires providing a reference to the actual lock whose guard is being
  // adopted.
  //
  // Example:
  //  Guard<fbl::Mutex> guard{AdoptLock, &lock, std::move(adopt)};
  template <typename Lockable>
  __WARN_UNUSED_CONSTRUCTOR Guard(AdoptLockTag tag, Lockable* lock, Adoptable&& state)
      __TA_ACQUIRE(lock) __TA_ACQUIRE(lock->capability())
      : validator_(std::move(state.state_.value().first)),
        state_(std::move(state.state_.value().second)) {
    ZX_DEBUG_ASSERT(&lock->lock() == validator_.lock());
    state.state_.reset();
  }

  // Temporarily releases and un-tracks the guarded lock before executing the
  // given callable Op and then re-acquires and tracks the lock. This permits
  // the same Guard instance to protect a larger scope while performing an
  // operation unlocked. This is especially useful in guarded loops:
  //
  //  Guard<fbl::Mutex> guard{&lock_};
  //  for (auto* entry : objects_.next()) {
  //      if (Pred(entry)) {
  //          objects_.erase(entry);
  //          guard.CallUnlocked([entry]() {
  //              // Unlocked operation on entry ...
  //          });
  //      }
  //  }
  //
  template <typename Op, typename... ReleaseArgs>
  void CallUnlocked(Op&& op, ReleaseArgs&&... release_args) __TA_NO_THREAD_SAFETY_ANALYSIS {
    ZX_DEBUG_ASSERT(validator_.lock() != nullptr);

    validator_.ValidateRelease(/* clear */ false);
    LockPolicy<LockType, Option>::Release(validator_.lock(), &state_,
                                          std::forward<ReleaseArgs>(release_args)...);

    std::forward<Op>(op)();

    ValidateAndAcquire();
  }

  // Temporarily un-tracks the lock before executing the given callable Op and
  // then tracks the lock again. This is similar to LockFlagsTrackingDisabled
  // flag except that it can scoped to a portion of a critical section, instead
  // of all usages of a lock. Valid usages of this are where the lockdep
  // validation may be too pessimistic and is unable to see that an operation is
  // safe. This should generally only be used in temporary scenarios until
  // either the underlying code can be restructured or lockdep expanded to
  // understand the scenario.
  //
  // An example of a scenario would be:
  //
  // void foo() {
  //   lockdep::AssertNoLocksHeld();
  //   // [Do things that require no locks be held]
  // }
  //
  // Guard<Mutex> guard{&lock_};
  // // lock_ is actually be safe to held over foo, but foo doesn't know that.
  // guard.CallUntracked([]{foo();});
  template <typename Op>
  void CallUntracked(Op&& op) {
    ZX_DEBUG_ASSERT(validator_.lock() != nullptr);

    validator_.ValidateRelease(/* clear */ false);

    std::forward<Op>(op)();

    validator_.ValidateAcquire();
  }

 private:
  template <size_t, typename, typename>
  friend class GuardMultiple;

  // Validates and acquires the lock. If the lock is a try-lock that failed
  // the release bookkeeping is performed and the guard is left in the empty
  // state. This method factors out the common body of the main constructors;
  // thread safety analysis is disabled to silence the unnecessary warning
  // about the conditional path that would not be raised in the constructor
  // body.
  void ValidateAndAcquire() __TA_NO_THREAD_SAFETY_ANALYSIS {
    // TODO(https://fxbug.dev/42165728): break this acquire up into two steps.  One where we
    // validate the operation before attempting to obtain the lock, and the
    // other where we update our bookkeeping to indicate that the lock is owned
    // _after_ successfully obtaining the lock.
    LockPolicy<LockType, Option>::PreValidate(validator_.lock(), &state_);
    validator_.ValidateAcquire();
    if (!LockPolicy<LockType, Option>::Acquire(validator_.lock(), &state_)) {
      validator_.ValidateRelease(/* clear */ true);
    }
  }

  // Ordered lock constructor used by the nestable lock constructor above and
  // by GuardMultiple.
  template <typename Lockable, typename... Args>
  __WARN_UNUSED_CONSTRUCTOR Guard(OrderedLockTag, Lockable* lock, uintptr_t order,
                                  Args&&... state_args) __TA_ACQUIRE(lock)
      __TA_ACQUIRE(lock->capability())
      : validator_{&lock->lock(), lock->id(), order}, state_{std::forward<Args>(state_args)...} {
    ValidateAndAcquire();
  }

  // Validator type used when lock validation is enabled. Provides the
  // AcquiredLockEntry instance and bookkeeping calls required by
  // ThreadLockState.
  struct LockValidator {
    LockValidator(LockType* lock, LockClassId id, uintptr_t order = 0)
        : lock_entry{lock, id, order} {}

    LockValidator(LockValidator&& other) { *this = std::move(other); }
    LockValidator& operator=(LockValidator&& other) {
      if (this != &other) {
        typename LockPolicy<LockType, Option>::ValidationGuard guard{};
        lock_entry = std::move(other.lock_entry);
      }
      return *this;
    }

    void ValidateAcquire() {
      typename LockPolicy<LockType, Option>::ValidationGuard guard{};
      ThreadLockState::Get(kLockFlags)->Acquire(&lock_entry);
    }
    void ValidateRelease(bool clear) {
      typename LockPolicy<LockType, Option>::ValidationGuard guard{};
      ThreadLockState::Get(kLockFlags)->Release(&lock_entry);
      if (clear) {
        lock_entry.Clear();
      }
    }

    LockType* lock() const { return static_cast<LockType*>(lock_entry.address()); }

    AcquiredLockEntry lock_entry;
  };

  // Validator type used when lock validation is disabled.
  struct NoValidator {
    NoValidator(LockType* lock, LockClassId, uintptr_t = 0) : address{lock} {}

    NoValidator(NoValidator&& other) noexcept { *this = std::move(other); }
    NoValidator& operator=(NoValidator&& other) noexcept {
      if (this != &other) {
        address = other.address;
        other.address = nullptr;
      }
      return *this;
    }

    void ValidateAcquire() {}
    void ValidateRelease(bool clear) {
      if (clear) {
        address = nullptr;
      }
    }

    LockType* lock() const { return address; }

    LockType* address;
  };

  // Alias of the configured validator.
  using Validator = IfLockValidationEnabled<LockValidator, NoValidator>;

  // The validator to use when acquiring and releasing the lock.
  Validator validator_;

  // State to store in the guard as specified by the lock policy. For example,
  // this may be used to save IRQ state for spinlocks.
  using State = typename LockPolicy<LockType, Option>::State;
  State state_;
};

template <typename LockType, typename Option>
class Guard<LockType, Option, internal::EnableIfNotShared<LockType, Option>>::Adoptable {
 public:
  Adoptable() = default;
  Adoptable(Adoptable&& other) : state_(std::move(other.state_)) { other.state_.reset(); }
  Adoptable(const Adoptable& other) = delete;
  Adoptable& operator=(const Adoptable&) = delete;
  Adoptable& operator=(Adoptable&& other) {
    ZX_ASSERT(!state_.has_value());
    if (other.state_.has_value()) {
      state_.emplace(std::move(*other.state_));
      other.state_.reset();
    }
    return *this;
  }
  ~Adoptable() { ZX_ASSERT(!state_.has_value()); }

  // Returns true if the adoptable holds an actively acquired lock.
  explicit operator bool() const { return state_.has_value(); }

  // Returns true if this adoptable originates from |lock|.
  bool wraps_lock(const LockType& lock) const {
    return state_.has_value() && &lock == state_->first.lock();
  }

  // Read-only access to the underlying |lock|.
  const LockType* lock() const {
    DEBUG_ASSERT(*this);
    return state_->first.lock();
  }

 private:
  Adoptable(Validator&& v, State&& s) : state_({std::move(v), std::move(s)}) {}
  friend Guard;
  std::optional<std::pair<Validator, State>> state_;
};

// Specialization of Guard that acquires the given lock in shared mode.
template <typename LockType, typename Option>
class __TA_SCOPED_CAPABILITY Guard<LockType, Option, internal::EnableIfShared<LockType, Option>> {
  static_assert(!std::is_same<LockPolicy<LockType, Option>, AmbiguousOption>::value,
                "The Option argument of Guard<LockType, Option> must always "
                "be specified when the policy for LockType is defined using "
                "the macro LOCK_DEP_POLICY_OPTION(). See the macro docs for "
                "details.");

 public:
  static constexpr LockFlags kLockFlags = LockTraits<RemoveGlobalReference<LockType>>::Flags;

  Guard(Guard&&) = delete;
  Guard(const Guard&) = delete;
  Guard& operator=(Guard&&) = delete;
  Guard& operator=(const Guard&) = delete;

  // Acquires the given lock. This constructor participates in overload
  // resolution when the underlying lock type is not nestable.
  template <typename Lockable, typename... Args,
            typename = internal::EnableIfNotNestable<Lockable, LockType>>
  __WARN_UNUSED_CONSTRUCTOR Guard(Lockable* lock, Args&&... state_args) __TA_ACQUIRE_SHARED(lock)
      __TA_ACQUIRE_SHARED(lock->capability())
      : validator_{&lock->lock(), lock->id()}, state_{std::forward<Args>(state_args)...} {
    ValidateAndAcquire();
  }

  // Acquires the given lock. This constructor participates in overload
  // resolution when the underlying lock type is nestable.
  template <typename Lockable, typename... Args,
            typename = internal::EnableIfNestable<Lockable, LockType>>
  __WARN_UNUSED_CONSTRUCTOR Guard(Lockable* lock, uintptr_t order, Args&&... state_args)
      __TA_ACQUIRE_SHARED(lock) __TA_ACQUIRE_SHARED(lock->capability())
      : Guard{OrderedLock, lock, order, std::forward<Args>(state_args)...} {}

  // Acquires the given type erased lock. The caller asserts the underlying lock
  // type is nestable, which is verified by a runtime check in debug builds.
  template <typename... Args>
  __WARN_UNUSED_CONSTRUCTOR Guard(AssertOrderedLockTag, Lock<LockType>* lock, uintptr_t order,
                                  Args&&... state_args) __TA_ACQUIRE_SHARED(lock)
      __TA_ACQUIRE_SHARED(lock->capability())
      : Guard{OrderedLock, lock, order, std::forward<Args>(state_args)...} {
    ZX_DEBUG_ASSERT(!kLockValidationEnabled || ValidatorLockClassState::IsNestable(lock->id()));
  }

  // Acquires the given type erased lock and its alias. Only the first lock is
  // actually acquired, the alias is verified to be the same lock by a runtime
  // check and its capability is acquired to satisfy static analysis.
  //
  // This constructor participates in overload resolution when the underlying
  // lock type is not nestable.
  template <typename Lockable, typename... Args,
            typename = internal::EnableIfNotNestable<Lockable, LockType>>
  __WARN_UNUSED_CONSTRUCTOR Guard(AliasedLockTag, Lockable* lock, Lockable* alias,
                                  Args&&... state_args) __TA_ACQUIRE_SHARED(lock)
      __TA_ACQUIRE_SHARED(lock->capability()) __TA_ACQUIRE_SHARED(alias)
          __TA_ACQUIRE_SHARED(alias->capability())
      : validator_{&lock->lock(), lock->id()}, state_{std::forward<Args>(state_args)...} {
    ZX_DEBUG_ASSERT(lock == alias);
    ValidateAndAcquire();
  }

  // Acquires the given type erased lock and its alias. Only the first lock is
  // actually acquired, the alias is verified to be the same lock by a runtime
  // check and its capability is acquired to satisfy static analysis.
  //
  // This constructor participates in overload resolution when the underlying
  // lock type is nestable.
  template <typename Lockable, typename... Args,
            typename = internal::EnableIfNestable<Lockable, LockType>>
  __WARN_UNUSED_CONSTRUCTOR Guard(AliasedLockTag, Lockable* lock, Lockable* alias, uintptr_t order,
                                  Args&&... state_args) __TA_ACQUIRE_SHARED(lock)
      __TA_ACQUIRE_SHARED(lock->capability()) __TA_ACQUIRE_SHARED(alias)
          __TA_ACQUIRE_SHARED(alias->capability())
      : Guard{OrderedLock, lock, order, std::forward<Args>(state_args)...} {
    ZX_DEBUG_ASSERT(lock == alias);
  }

  // Destructor that automatically releases the lock if not already released.
  ~Guard() __TA_RELEASE() { Release(); }

  // Releases the lock early before this guard instance goes out of scope.
  template <typename... Args>
  void Release(Args&&... args) __TA_RELEASE() {
    LockType* lock_ptr = validator_.lock();

    if (lock_ptr != nullptr) {
      validator_.ValidateRelease(/* clear */ true);
      LockPolicy<LockType, Option>::Release(lock_ptr, &state_, std::forward<Args>(args)...);
    }
  }

  // Returns true if the guard has an actively acquired lock.
  explicit operator bool() const { return validator_.lock() != nullptr; }

  // Returns true if this guard wraps |lock|.
  bool wraps_lock(const LockType& lock) const { return &lock == validator_.lock(); }

  // Read-only access to the underlying |lock|.
  const LockType* lock() const { return validator_.lock(); }

  class Adoptable;
  // Releases this scoped capability without releasing the underlying lock or
  // un-tracking the lock in the validator. Returns an object holding the
  // lock state and validator state which may be adopted by another Guard.
  // This is useful in the rare situation where a lock wants to be used and
  // released in a different scope, or if iterating over a data structure that
  // uses fine-grained locking.
  //
  // Example:
  //  Guard<fbl::Mutex> guard{&lock};
  //
  //  // Setup actions...
  //
  //  DoTaskAndReleaseLock(guard.take());
  //  or
  //  return guard.take();
  //
  Adoptable take() __TA_RELEASE() { return Adoptable(std::move(validator_), std::move(state_)); }

  // Adopts the lock state and validate state from a previous take() action.
  // This requires providing a reference to the actual lock whose guard is being
  // adopted.
  //
  // Example:
  //  Guard<fbl::Mutex> guard{AdoptLock, &lock, std::move(adopt)};
  template <typename Lockable>
  __WARN_UNUSED_CONSTRUCTOR Guard(AdoptLockTag tag, Lockable* lock, Adoptable&& state)
      __TA_ACQUIRE_SHARED(lock) __TA_ACQUIRE_SHARED(lock->capability())
      : validator_(std::move(state.state_.value().first)),
        state_(std::move(state.state_.value().second)) {
    ZX_DEBUG_ASSERT(&lock->lock() == validator_.lock());
    state.state_.reset();
  }

  // Temporarily releases and un-tracks the guarded lock before executing the
  // given callable Op and then re-acquires and tracks the lock. This permits
  // the same Guard instance to protect a larger scope while performing an
  // operation unlocked. This is especially useful in guarded loops:
  //
  //  Guard<fbl::Mutex> guard{&lock_};
  //  for (auto* entry : objects_.next()) {
  //      if (Pred(entry)) {
  //          objects_.erase(entry);
  //          guard.CallUnlocked([entry]() {
  //              // Unlocked operation on entry ...
  //          });
  //      }
  //  }
  //
  template <typename Op, typename... ReleaseArgs>
  void CallUnlocked(Op&& op, ReleaseArgs&&... release_args) __TA_NO_THREAD_SAFETY_ANALYSIS {
    ZX_DEBUG_ASSERT(validator_.lock() != nullptr);

    validator_.ValidateRelease(/* clear */ false);
    LockPolicy<LockType, Option>::Release(validator_.lock(), &state_,
                                          std::forward<ReleaseArgs>(release_args)...);

    std::forward<Op>(op)();

    ValidateAndAcquire();
  }

  // Temporarily un-tracks the lock before executing the given callable Op and
  // then tracks the lock again. This is similar to LockFlagsTrackingDisabled
  // flag except that it can scoped to a portion of a critical section, instead
  // of all usages of a lock. Valid usages of this are where the lockdep
  // validation may be too pessimistic and is unable to see that an operation is
  // safe. This should generally only be used in temporary scenarios until
  // either the underlying code can be restructured or lockdep expanded to
  // understand the scenario.
  //
  // An example of a scenario would be:
  //
  // void foo() {
  //   lockdep::AssertNoLocksHeld();
  //   // [Do things that require no locks be held]
  // }
  //
  // Guard<Mutex> guard{&lock_};
  // // lock_ is actually be safe to held over foo, but foo doesn't know that.
  // guard.CallUntracked([]{foo();});
  template <typename Op>
  void CallUntracked(Op&& op) {
    ZX_DEBUG_ASSERT(validator_.lock() != nullptr);

    validator_.ValidateRelease(/* clear */ false);

    std::forward<Op>(op)();

    validator_.ValidateAcquire();
  }

 private:
  template <size_t, typename, typename>
  friend class GuardMultiple;

  // Validates and acquires the lock. If the lock is a try-lock that failed
  // the release bookkeeping is performed and the guard is left in the empty
  // state. This method factors out the common body of the main constructors;
  // thread safety analysis is disabled to silence the unnecessary warning
  // about the conditional path that would not be raised in the constructor
  // body.
  void ValidateAndAcquire() __TA_NO_THREAD_SAFETY_ANALYSIS {
    // TODO(https://fxbug.dev/42165728): break this acquire up into two steps.  One where we
    // validate the operation before attempting to obtain the lock, and the
    // other where we update our bookkeeping to indicate that the lock is owned
    // _after_ successfully obtaining the lock.
    LockPolicy<LockType, Option>::PreValidate(validator_.lock(), &state_);
    validator_.ValidateAcquire();
    if (!LockPolicy<LockType, Option>::Acquire(validator_.lock(), &state_)) {
      validator_.ValidateRelease(/* clear */ true);
    }
  }

  // Ordered lock constructor used by the nestable lock constructor above and
  // by GuardMultiple.
  template <typename Lockable, typename... Args>
  __WARN_UNUSED_CONSTRUCTOR Guard(OrderedLockTag, Lockable* lock, uintptr_t order,
                                  Args&&... state_args) __TA_ACQUIRE_SHARED(lock)
      __TA_ACQUIRE_SHARED(lock->capability())
      : validator_{&lock->lock(), lock->id(), order}, state_{std::forward<Args>(state_args)...} {
    ValidateAndAcquire();
  }

  // Validator type used when lock validation is enabled. Provides the
  // AcquiredLockEntry instance and bookkeeping calls required by
  // ThreadLockState.
  struct LockValidator {
    LockValidator(LockType* lock, LockClassId id, uintptr_t order = 0)
        : lock_entry{lock, id, order} {}

    LockValidator(LockValidator&& other) { *this = std::move(other); }
    LockValidator& operator=(LockValidator&& other) {
      if (this != &other) {
        typename LockPolicy<LockType, Option>::ValidationGuard guard{};
        lock_entry = std::move(other.lock_entry);
      }
      return *this;
    }

    void ValidateAcquire() {
      typename LockPolicy<LockType, Option>::ValidationGuard guard{};
      ThreadLockState::Get(kLockFlags)->Acquire(&lock_entry);
    }
    void ValidateRelease(bool clear) {
      typename LockPolicy<LockType, Option>::ValidationGuard guard{};
      ThreadLockState::Get(kLockFlags)->Release(&lock_entry);
      if (clear) {
        lock_entry.Clear();
      }
    }

    LockType* lock() const { return static_cast<LockType*>(lock_entry.address()); }

    AcquiredLockEntry lock_entry;
  };

  // Validator type used when lock validation is disabled.
  struct NoValidator {
    NoValidator(LockType* lock, LockClassId, uintptr_t = 0) : address{lock} {}

    NoValidator(NoValidator&& other) noexcept { *this = std::move(other); }
    NoValidator& operator=(NoValidator&& other) noexcept {
      if (this != &other) {
        address = other.address;
        other.address = nullptr;
      }
      return *this;
    }

    void ValidateAcquire() {}
    void ValidateRelease(bool clear) {
      if (clear) {
        address = nullptr;
      }
    }

    LockType* lock() const { return address; }

    LockType* address;
  };

  // Alias of the configured validator.
  using Validator = IfLockValidationEnabled<LockValidator, NoValidator>;

  // The validator to use when acquiring and releasing the lock.
  Validator validator_;

  // State to store in the guard as specified by the lock policy. For example,
  // this may be used to save IRQ state for spinlocks.
  typename LockPolicy<LockType, Option>::State state_;
};

// NullGuard is a stub class that has the same API as Guard but does nothing.
class __TA_SCOPED_CAPABILITY NullGuard {
 public:
  NullGuard(NullGuard&&) = delete;
  NullGuard(const NullGuard&) = delete;
  NullGuard& operator=(NullGuard&&) = delete;
  NullGuard& operator=(const NullGuard&) = delete;

  template <typename Lockable, typename... Args>
  explicit NullGuard(Lockable* lock, Args&&... state_args) __TA_ACQUIRE(lock) {}
  ~NullGuard() __TA_RELEASE() { Release(); }
  template <typename... Args>
  void Release(Args&&... args) __TA_RELEASE() {}

  template <typename Op, typename... ReleaseArgs>
  void CallUnlocked(Op&& op, ReleaseArgs&&... release_args) {
    std::forward<Op>(op)();
  }
  template <typename Op>
  void CallUntracked(Op&& op) {
    std::forward<Op>(op)();
  }
};

}  // namespace lockdep

#endif  // SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_GUARD_H_
