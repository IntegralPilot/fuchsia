// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/arch/intrin.h>
#include <lib/fit/defer.h>
#include <lib/unittest/unittest.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>
#include <zircon/time.h>

#include <kernel/lock_validation_guard.h>
#include <kernel/mp.h>
#include <kernel/mutex.h>
#include <kernel/spinlock.h>
#include <ktl/atomic.h>
#include <ktl/bit.h>
#include <lockdep/guard.h>
#include <lockdep/guard_multiple.h>
#include <lockdep/lockdep.h>

#include "tests.h"

#include <ktl/enforce.h>

#if WITH_LOCK_DEP_TESTS

// Defined in kernel/lib/lockdep.
zx_status_t TriggerAndWaitForLoopDetection(zx_instant_mono_t deadline);

namespace test {

// Global flag that determines whether try lock operations succeed.
bool g_try_lock_succeeds = true;

// Define some proxy types to simulate different kinds of locks.
struct Spinlock : Mutex {
  using Mutex::Mutex;

  bool TryAcquire() TA_TRY_ACQ(true) {
    if (g_try_lock_succeeds)
      Acquire();
    return g_try_lock_succeeds;
  }
  bool TryAcquireIrqSave(uint64_t* flags) TA_TRY_ACQ(true) {
    (void)flags;
    if (g_try_lock_succeeds)
      Acquire();
    return g_try_lock_succeeds;
  }
};
LOCK_DEP_TRAITS(Spinlock, lockdep::LockFlagsIrqSafe);

// Fake C-style locking primitive.
struct TA_CAP("mutex") spinlock_t {
  void Acquire() TA_ACQ() {}
  void Release() TA_REL() {}
  bool TryAcquire() TA_TRY_ACQ(true) { return g_try_lock_succeeds; }
};
LOCK_DEP_TRAITS(spinlock_t, lockdep::LockFlagsIrqSafe);

static void spinlock_lock(spinlock_t* lock) TA_ACQ(lock) { lock->Acquire(); }
static void spinlock_unlock(spinlock_t* lock) TA_REL(lock) { lock->Release(); }
static bool spinlock_try_lock(spinlock_t* lock) TA_TRY_ACQ(true, lock) {
  return lock->TryAcquire();
}

// Type tags to select Guard<> lock policies for Spinlock and spinlock_t.
struct IrqSave {};
struct NoIrqSave {};
struct TryIrqSave {};
struct TryNoIrqSave {};

struct SpinlockNoIrqSave {
  struct State {};

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  static void PreValidate(Spinlock* lock, State*) {}
  static bool Acquire(Spinlock* lock, State*) TA_ACQ(lock) {
    lock->Acquire();
    return true;
  }

  static void Release(Spinlock* lock, State*) TA_REL(lock) { lock->Release(); }
};
LOCK_DEP_POLICY_OPTION(Spinlock, NoIrqSave, SpinlockNoIrqSave);

struct SpinlockIrqSave {
  struct State {};

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  static void PreValidate(Spinlock* lock, State*) {}
  static bool Acquire(Spinlock* lock, State*) TA_ACQ(lock) {
    lock->Acquire();
    return true;
  }

  static void Release(Spinlock* lock, State*) TA_REL(lock) { lock->Release(); }
};
LOCK_DEP_POLICY_OPTION(Spinlock, IrqSave, SpinlockIrqSave);

struct SpinlockTryNoIrqSave {
  struct State {};

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  static void PreValidate(Spinlock* lock, State*) {}
  static bool Acquire(Spinlock* lock, State*) TA_TRY_ACQ(true, lock) { return lock->TryAcquire(); }
  static void Release(Spinlock* lock, State*) TA_REL(lock) { lock->Release(); }
};
LOCK_DEP_POLICY_OPTION(Spinlock, TryNoIrqSave, SpinlockTryNoIrqSave);

struct SpinlockTryIrqSave {
  struct State {};

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  static void PreValidate(Spinlock* lock, State*) {}
  static bool Acquire(Spinlock* lock, State*) TA_TRY_ACQ(true, lock) { return lock->TryAcquire(); }
  static void Release(Spinlock* lock, State*) TA_REL(lock) { lock->Release(); }
};
LOCK_DEP_POLICY_OPTION(Spinlock, TryIrqSave, SpinlockTryIrqSave);

struct spinlock_t_NoIrqSave {
  struct State {};

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  static void PreValidate(spinlock_t* lock, State*) {}
  static bool Acquire(spinlock_t* lock, State*) TA_ACQ(lock) {
    spinlock_lock(lock);
    return true;
  }
  static void Release(spinlock_t* lock, State*) TA_REL(lock) { spinlock_unlock(lock); }
};
LOCK_DEP_POLICY_OPTION(spinlock_t, NoIrqSave, spinlock_t_NoIrqSave);

struct spinlock_t_IrqSave {
  struct State {};

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  static void PreValidate(spinlock_t* lock, State*) {}
  static bool Acquire(spinlock_t* lock, State*) TA_ACQ(lock) {
    spinlock_lock(lock);
    return true;
  }

  static void Release(spinlock_t* lock, State*) TA_REL(lock) { spinlock_unlock(lock); }
};
LOCK_DEP_POLICY_OPTION(spinlock_t, IrqSave, spinlock_t_IrqSave);

struct spinlock_t_TryNoIrqSave {
  struct State {};

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  static void PreValidate(spinlock_t* lock, State*) {}
  static bool Acquire(spinlock_t* lock, State*) TA_TRY_ACQ(true, lock) {
    return spinlock_try_lock(lock);
  }
  static void Release(spinlock_t* lock, State*) TA_REL(lock) { spinlock_unlock(lock); }
};
LOCK_DEP_POLICY_OPTION(spinlock_t, TryNoIrqSave, spinlock_t_TryNoIrqSave);

struct spinlock_t_TryIrqSave {
  struct State {};

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  static void PreValidate(spinlock_t* lock, State*) {}
  static bool Acquire(spinlock_t* lock, State*) TA_TRY_ACQ(true, lock) {
    return spinlock_try_lock(lock);
  }
  static void Release(spinlock_t* lock, State*) TA_REL(lock) { spinlock_unlock(lock); }
};
LOCK_DEP_POLICY_OPTION(spinlock_t, TryIrqSave, spinlock_t_TryIrqSave);

struct Mutex : ::Mutex {
  using ::Mutex::Mutex;
};
// Uses the default traits: fbl::LockClassState::None.

struct Nestable : ::Mutex {
  using ::Mutex::Mutex;
};
LOCK_DEP_TRAITS(Nestable, lockdep::LockFlagsNestable);

struct TA_CAP("mutex") ReadWriteLock {
  bool AcquireWrite() TA_ACQ() { return true; }
  bool AcquireRead() TA_ACQ_SHARED() { return true; }
  void Release() TA_REL() {}

  struct Read {
    struct State {};
    struct Shared {};
    // Protects the thread local lock list and validation.
    using ValidationGuard = LockValidationGuard;
    static void PreValidate(ReadWriteLock* lock, State*) {}
    static bool Acquire(ReadWriteLock* lock, State*) TA_ACQ_SHARED(lock) {
      return lock->AcquireRead();
    }
    static void Release(ReadWriteLock* lock, State*) TA_REL(lock) { lock->Release(); }
  };

  struct Write {
    struct State {};
    // Protects the thread local lock list and validation.
    using ValidationGuard = LockValidationGuard;
    static void PreValidate(ReadWriteLock* lock, State*) {}
    static bool Acquire(ReadWriteLock* lock, State*) TA_ACQ(lock) { return lock->AcquireWrite(); }
    static void Release(ReadWriteLock* lock, State*) TA_REL(lock) { lock->Release(); }
  };
};
LOCK_DEP_POLICY_OPTION(ReadWriteLock, ReadWriteLock::Read, ReadWriteLock::Read);
LOCK_DEP_POLICY_OPTION(ReadWriteLock, ReadWriteLock::Write, ReadWriteLock::Write);

struct Foo {
  LOCK_DEP_INSTRUMENT(Foo, Mutex) lock;

  void TestRequire() TA_REQ(lock) {}
  void TestExclude() TA_EXCL(lock) {}
};

struct Bar {
  LOCK_DEP_INSTRUMENT(Bar, Mutex) lock;

  void TestRequire() TA_REQ(lock) {}
  void TestExclude() TA_EXCL(lock) {}
};

template <typename LockType, lockdep::LockFlags Flags = lockdep::LockFlagsNone>
struct Baz {
  LOCK_DEP_INSTRUMENT(Baz, LockType, Flags) lock;

  lockdep::Lock<LockType>* get_lock() TA_RET_CAP(lock) { return &lock; }

  void TestRequire() TA_REQ(lock) {}
  void TestExclude() TA_EXCL(lock) {}
  void TestShared() TA_REQ_SHARED(lock) {}
};

struct MultipleLocks {
  LOCK_DEP_INSTRUMENT(MultipleLocks, Mutex) lock_a;
  LOCK_DEP_INSTRUMENT(MultipleLocks, Mutex) lock_b;

  void TestRequireLockA() TA_REQ(lock_a) {}
  void TestExcludeLockA() TA_EXCL(lock_a) {}
  void TestRequireLockB() TA_REQ(lock_b) {}
  void TestExcludeLockB() TA_EXCL(lock_b) {}
};

template <size_t Index>
struct Number {
  LOCK_DEP_INSTRUMENT(Number, Mutex) lock;

  void TestRequire() TA_REQ(lock) {}
  void TestExclude() TA_EXCL(lock) {}
};

template <typename GuardType>
lockdep::LockResult GetLastResult(const GuardType&) {
#if WITH_LOCK_DEP
  lockdep::ThreadLockState* state = lockdep::ThreadLockState::Get(GuardType::kLockFlags);
  return state->last_result();
#else
  return lockdep::LockResult::Success;
#endif
}

static void ResetTrackingState() {
#if WITH_LOCK_DEP
  for (auto& state : lockdep::ValidatorLockClassState::Iter())
    state.Reset();
#endif
}

// This is a simplified example of a hazard with lock assertions that arises in use cases where
// multiple objects share a synchronization domain (i.e. a lock) through a reference to a common
// resource. Lock annotations in these use cases can run into tricky conflicts, since more than one
// capability aliases the same lock.
//
// In this example, the Resource class models the common resource with a lock that is shared by
// multiple instances of the Object class. Each Object instance has a pointer to a Resource and an
// instance of Resource::State that may only be mutated while holding the resource lock.
//
// A tricky lock aliasing situation arises when attempting to call Resource::Merge on the state of
// two different Objects that use the same Resource: static analysis cannot tell the resources are
// the same and will complain if only one of the lock capabilities is acquired.
//
// A naive solution is to acquire one lock and assert the alias is held, however, this gives rise to
// the scope hazard demonstrated in Object::HazardousMerge. The lock assertion is not limited to the
// scope of the lock guard, causing static analysis to mistakenly allow an unprotected access to the
// second object state.
//
// A better solution is to use the AliasedLock Guard constructor, which acquires and validates one
// lock, checks the locks are aliases in debug builds, and acquires both capabilities.
//
class Resource {
 public:
  Resource() = default;
  ~Resource() = default;

  Lock<Mutex>* lock() const TA_RET_CAP(lock_) { return &lock_; }
  Lock<Mutex>& lock_ref() const TA_RET_CAP(lock_) { return lock_; }

  struct State {
    State() = default;
    ~State() = default;

    void Operation() {}
  };

  void Add(State* state) TA_REQ(lock()) {}
  void Remove(State* state) TA_REQ(lock()) {}
  void Merge(State* a, State* b) TA_REQ(lock()) {}

 private:
  mutable LOCK_DEP_INSTRUMENT(Resource, Mutex) lock_;
};

class Object {
 public:
  explicit Object(Resource* resource) : resource_{resource} { AddStateToResource(); }
  ~Object() { RemoveStateFromResource(); }

  Lock<Mutex>* lock() const TA_RET_CAP(resource_->lock()) { return resource_->lock(); }
  Lock<Mutex>& lock_ref() const TA_RET_CAP(resource_->lock()) { return resource_->lock_ref(); }

  void HazardousMerge(Object* other) {
    ZX_DEBUG_ASSERT(resource_ == other->resource_);
    {
      Guard<Mutex> guard{lock()};
      AssertHeld(other->lock_ref());
      MergeLocked(other);
    }
    // Static analysis cannot tell this is dangerous, since lock assertions don't respect scopes
    // unless the compiler can tell these are the same lock and sees the lock get released.
    other->OperationLocked();
  }

  void SafeMerge(Object* other) {
    ZX_DEBUG_ASSERT(resource_ == other->resource_);
    {
      Guard<Mutex> guard{AliasedLock, lock(), other->lock()};
      MergeLocked(other);
    }
#if TEST_WILL_NOT_COMPILE || 0
    other->OperationLocked();
#endif
  }

  void Operation() TA_EXCL(lock()) {
    Guard<Mutex> guard{lock()};
    state_.Operation();
  }
  void OperationLocked() TA_REQ(lock()) { state_.Operation(); }

 private:
  void AddStateToResource() {
    Guard<Mutex> guard{lock()};
    resource_->Add(&state_);
  }
  void RemoveStateFromResource() {
    Guard<Mutex> guard{lock()};
    resource_->Remove(&state_);
  }
  void MergeLocked(Object* other) TA_REQ(lock()) TA_REQ(other->lock()) {
    resource_->Merge(&state_, &other->state_);
  }

  Resource* resource_;
  Resource::State state_ TA_GUARDED(lock());
};

}  // namespace test

static bool lock_dep_dynamic_analysis_tests() {
  BEGIN_TEST;

  using lockdep::AssertNoLocksHeld;
  using lockdep::AssertOrderedLock;
  using lockdep::Guard;
  using lockdep::GuardMultiple;
  using lockdep::kInvalidLockClassId;
  using lockdep::LockFlagsLeaf;
  using lockdep::LockFlagsMultiAcquire;
  using lockdep::LockFlagsNestable;
  using lockdep::LockFlagsReAcquireFatal;
  using lockdep::LockResult;
  using lockdep::ThreadLockState;
  using lockdep::ValidatorLockClassState;
  using test::Bar;
  using test::Baz;
  using test::Foo;
  using test::GetLastResult;
  using test::IrqSave;
  using test::MultipleLocks;
  using test::Mutex;
  using test::Nestable;
  using test::NoIrqSave;
  using test::Number;
  using test::ReadWriteLock;
  using test::Spinlock;
  using test::spinlock_t;
  using test::TryIrqSave;
  using test::TryNoIrqSave;

  // Reset the tracking state before each test run.
  test::ResetTrackingState();

  // Initially no locks should be held.
  AssertNoLocksHeld();

  // Single lock.
  {
    Foo a{};

    Guard<Mutex> guard_a{&a.lock};
    EXPECT_TRUE(guard_a);
    EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));
  }

  // Single lock.
  {
    Bar a{};

    Guard<Mutex> guard_a{&a.lock};
    EXPECT_TRUE(guard_a);
    EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));
  }

  // Test order invariant.
  {
    Foo a{};
    Foo b{};

    Guard<Mutex> guard_a{&a.lock};
    EXPECT_TRUE(guard_a);
    EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

    Guard<Mutex> guard_b{&b.lock};
    EXPECT_TRUE(guard_b);
    EXPECT_EQ(LockResult::AlreadyAcquired, test::GetLastResult(guard_b));
  }

  // Test order invariant with a different lock class.
  {
    Bar a{};
    Bar b{};

    Guard<Mutex> guard_a{&a.lock};
    EXPECT_TRUE(guard_a);
    EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

    Guard<Mutex> guard_b{&b.lock};
    EXPECT_TRUE(guard_b);
    EXPECT_EQ(LockResult::AlreadyAcquired, test::GetLastResult(guard_b));
  }

  // Test address order invariant.
  {
    Foo a{};
    Foo b{};

    {
      GuardMultiple<2, Mutex> guard_all{&a.lock, &b.lock};
      EXPECT_TRUE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }

    {
      GuardMultiple<2, Mutex> guard_all{&b.lock, &a.lock};
      EXPECT_TRUE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }
  }

  // Test address order invariant with a different lock class.
  {
    Bar a{};
    Bar b{};

    {
      GuardMultiple<2, Mutex> guard_all{&a.lock, &b.lock};
      EXPECT_TRUE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }

    {
      GuardMultiple<2, Mutex> guard_all{&b.lock, &a.lock};
      EXPECT_TRUE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }
  }

  // Test address order invariant with spinlocks.
  {
    Baz<Spinlock> a{};
    Baz<Spinlock> b{};

    {
      GuardMultiple<2, Spinlock, NoIrqSave> guard_all{&a.lock, &b.lock};
      EXPECT_TRUE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }

    {
      GuardMultiple<2, Spinlock, NoIrqSave> guard_all{&b.lock, &a.lock};
      EXPECT_TRUE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }

    {
      test::g_try_lock_succeeds = true;
      GuardMultiple<2, Spinlock, TryNoIrqSave> guard_all{&a.lock, &b.lock};
      EXPECT_TRUE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }

    {
      test::g_try_lock_succeeds = true;
      GuardMultiple<2, Spinlock, TryNoIrqSave> guard_all{&b.lock, &a.lock};
      EXPECT_TRUE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }

    {
      test::g_try_lock_succeeds = false;
      GuardMultiple<2, Spinlock, TryNoIrqSave> guard_all{&a.lock, &b.lock};
      EXPECT_FALSE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }

    {
      test::g_try_lock_succeeds = false;
      GuardMultiple<2, Spinlock, TryNoIrqSave> guard_all{&b.lock, &a.lock};
      EXPECT_FALSE(guard_all);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_all));
    }
  }

  // Foo -> Bar -- establish order.
  {
    Foo a{};
    Bar b{};

    Guard<Mutex> guard_a{&a.lock};
    EXPECT_TRUE(guard_a);
    EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

    Guard<Mutex> guard_b{&b.lock};
    EXPECT_TRUE(guard_b);
    EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));
  }

  // Bar -> Foo -- check order invariant.
  {
    Foo a{};
    Bar b{};

    Guard<Mutex> guard_b{&b.lock};
    EXPECT_TRUE(guard_b);
    EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));

    Guard<Mutex> guard_a{&a.lock};
    EXPECT_TRUE(guard_a);
    EXPECT_EQ(LockResult::OutOfOrder, test::GetLastResult(guard_a));
  }

  // Test external order invariant.
  {
    Baz<Nestable> baz1;
    Baz<Nestable> baz2;

    {
      Guard<Nestable> auto_baz1{&baz1.lock, 0};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz1));

      Guard<Nestable> auto_baz2{&baz2.lock, 1};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));
    }

    {
      Guard<Nestable> auto_baz2{&baz2.lock, 0};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));

      Guard<Nestable> auto_baz1{&baz1.lock, 1};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));
    }

    {
      Guard<Nestable> auto_baz2{&baz2.lock, 1};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));

      Guard<Nestable> auto_baz1{&baz1.lock, 0};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::InvalidNesting, test::GetLastResult(auto_baz1));
    }
  }

  // Test external order invariant with nestable flag supplied on the lock
  // member, rather than the lock type.
  {
    Baz<Mutex, LockFlagsNestable> baz1;
    Baz<Mutex, LockFlagsNestable> baz2;

    {
      Guard<Mutex> auto_baz1{&baz1.lock, 0};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz1));

      Guard<Mutex> auto_baz2{&baz2.lock, 1};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));
    }

    {
      Guard<Mutex> auto_baz2{&baz2.lock, 0};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));

      Guard<Mutex> auto_baz1{&baz1.lock, 1};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz1));
    }

    {
      Guard<Mutex> auto_baz2{&baz2.lock, 1};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));

      Guard<Mutex> auto_baz1{&baz1.lock, 0};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::InvalidNesting, test::GetLastResult(auto_baz1));
    }
  }

  // Test external order invariant with nestable flag supplied on the lock
  // member, rather than the lock type, using the type-erased lock type.
  {
    Baz<Mutex, LockFlagsNestable> baz1;
    Baz<Mutex, LockFlagsNestable> baz2;

    {
      Guard<Mutex> auto_baz1{AssertOrderedLock, baz1.get_lock(), 0};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz1));

      Guard<Mutex> auto_baz2{AssertOrderedLock, baz2.get_lock(), 1};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));
    }

    {
      Guard<Mutex> auto_baz2{AssertOrderedLock, baz2.get_lock(), 0};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));

      Guard<Mutex> auto_baz1{AssertOrderedLock, baz1.get_lock(), 1};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz1));
    }

    {
      Guard<Mutex> auto_baz2{AssertOrderedLock, baz2.get_lock(), 1};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));

      Guard<Mutex> auto_baz1{AssertOrderedLock, baz1.get_lock(), 0};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::InvalidNesting, test::GetLastResult(auto_baz1));
    }
  }

  // Test using a non-nestable lock with the AssertOrderedLock tag.
  // TODO(33187): Enable the userspace death test when lockdep has a userspace
  // runtime and validation can be tested in userspace.
  {
#if TEST_WILL_DEBUG_ASSERT || 0
    Baz<Mutex> baz3;
    Guard<Mutex> auto_baz3{AssertOrderedLock, baz3.get_lock(), 0};
#endif
  }

  // Test that asserting no locks held while holding a log will fail.
  // TODO(338187): Enable the userspace death test when lockdep has a userspace
  // runtime and validation can be tested in userspace.
  {
#if TEST_WILL_DEBUG_ASSERT || 0
    Baz<Mutex> baz;
    Guard<Mutex> auto_baz{&baz.lock};
    AssertNoLocksHeld();
#endif
  }

  // Test that acquiring a lock after a leaf lock will fail.
  // TODO(338187): Enable the userspace death test when lockdep has a userspace
  // runtime and validation can be tested in userspace.
  {
#if TEST_WILL_PANIC || 0
    Baz<Mutex, LockFlagsLeaf> baz_leaf_lock;
    Baz<Mutex> baz;
    Guard<Mutex> leaf_guard{&baz_leaf_lock.lock};
    Guard<Mutex> guard{&baz.lock};
#endif
  }

  // Test irq-safety invariant.
  {
    Baz<Mutex> baz1;
    Baz<Spinlock> baz2;

    {
      Guard<Mutex> auto_baz1{&baz1.lock};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz1));

      Guard<Spinlock, NoIrqSave> auto_baz2{&baz2.lock};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));
    }

    // Note that this ordering (spinlock followed by mutex) cannot currently be
    // detected by lockdep in the kernel.  See https://fxbug.dev/42166171 and the discussion of
    // the solution for details.  The TL;DR is that spinlocks and mutexes end up
    // using different system provided contexts for their analysis, and rely on
    // asserts in the lock implementations themselves to provide protection
    // against either attempting to acquire a spinlock with IRQs still enabled,
    // or attempting to acquire a blocking mutex while holding a spinlock.
#if 0
    {
      Guard<Spinlock, NoIrqSave> auto_baz2{&baz2.lock};
      EXPECT_TRUE(auto_baz2);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(auto_baz2));

      Guard<Mutex> auto_baz1{&baz1.lock};
      EXPECT_TRUE(auto_baz1);
      EXPECT_EQ(LockResult::InvalidIrqSafety, test::GetLastResult(auto_baz1));
    }
#endif
  }

  // Test spinlock options compile and basic guard functions.
  // TODO(eieio): Add Guard<>::state() accessor and check state values.
  {
    Baz<Spinlock> baz1;
    Baz<spinlock_t> baz2;

    {
      Guard<Spinlock, NoIrqSave> guard{&baz1.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      Guard<Spinlock, IrqSave> guard{&baz1.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      Guard<spinlock_t, NoIrqSave> guard{&baz2.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      Guard<spinlock_t, IrqSave> guard{&baz2.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      test::g_try_lock_succeeds = true;
      Guard<Spinlock, TryNoIrqSave> guard{&baz1.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      test::g_try_lock_succeeds = true;
      Guard<Spinlock, TryIrqSave> guard{&baz1.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      test::g_try_lock_succeeds = false;
      Guard<spinlock_t, TryNoIrqSave> guard{&baz2.lock};
      EXPECT_FALSE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      test::g_try_lock_succeeds = false;
      Guard<spinlock_t, TryIrqSave> guard{&baz2.lock};
      EXPECT_FALSE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    // Test that Guard<LockType, Option> fails to compile when Option is
    // required by the policy config but not specified.
    {
#if TEST_WILL_NOT_COMPILE || 0
      Guard<Spinlock> guard1{&baz1.lock};
      Guard<spinlock_t> guard2{&baz2.lock};
#endif
    }
  }

  // Test read/write lock compiles and basic guard options.
  {
    Baz<ReadWriteLock> a{};
    Baz<ReadWriteLock> b{};

    {
      Guard<ReadWriteLock, ReadWriteLock::Read> guard{&a.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Read> guard{&b.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Write> guard{&a.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Write> guard{&b.lock};
      EXPECT_TRUE(guard);
      guard.Release();
      EXPECT_FALSE(guard);
    }
  }

  // Test read/write lock order invariants.
  {
    Baz<ReadWriteLock> a{};
    Baz<ReadWriteLock> b{};

    {
      Guard<ReadWriteLock, ReadWriteLock::Read> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<ReadWriteLock, ReadWriteLock::Read> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::AlreadyAcquired, test::GetLastResult(guard_b));
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Read> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      [&]() TA_NO_THREAD_SAFETY_ANALYSIS {
        Guard<ReadWriteLock, ReadWriteLock::Read> guard_b{&a.lock};
        EXPECT_TRUE(guard_b);
        EXPECT_EQ(LockResult::Reentrance, test::GetLastResult(guard_b));
        return true;
      }();
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Read> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<ReadWriteLock, ReadWriteLock::Write> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::AlreadyAcquired, test::GetLastResult(guard_b));
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Read> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      [&]() TA_NO_THREAD_SAFETY_ANALYSIS {
        Guard<ReadWriteLock, ReadWriteLock::Write> guard_b{&a.lock};
        EXPECT_TRUE(guard_b);
        EXPECT_EQ(LockResult::Reentrance, test::GetLastResult(guard_b));
        return true;
      }();
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Write> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<ReadWriteLock, ReadWriteLock::Read> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::AlreadyAcquired, test::GetLastResult(guard_b));
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Write> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      [&]() TA_NO_THREAD_SAFETY_ANALYSIS {
        Guard<ReadWriteLock, ReadWriteLock::Read> guard_b{&a.lock};
        EXPECT_TRUE(guard_b);
        EXPECT_EQ(LockResult::Reentrance, test::GetLastResult(guard_b));
        return true;
      }();
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Write> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<ReadWriteLock, ReadWriteLock::Write> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::AlreadyAcquired, test::GetLastResult(guard_b));
    }

    {
      Guard<ReadWriteLock, ReadWriteLock::Write> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      [&]() TA_NO_THREAD_SAFETY_ANALYSIS {
        Guard<ReadWriteLock, ReadWriteLock::Write> guard_b{&a.lock};
        EXPECT_TRUE(guard_b);
        EXPECT_EQ(LockResult::Reentrance, test::GetLastResult(guard_b));
        return true;
      }();
    }

    {
      GuardMultiple<2, ReadWriteLock, ReadWriteLock::Read> guard{&a.lock, &b.lock};
      EXPECT_TRUE(guard);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard));
    }

    {
      GuardMultiple<2, ReadWriteLock, ReadWriteLock::Read> guard{&a.lock, &a.lock};
      EXPECT_TRUE(guard);
      EXPECT_EQ(LockResult::Reentrance, test::GetLastResult(guard));
    }

    {
      GuardMultiple<2, ReadWriteLock, ReadWriteLock::Write> guard{&a.lock, &b.lock};
      EXPECT_TRUE(guard);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard));
    }

    {
      GuardMultiple<2, ReadWriteLock, ReadWriteLock::Write> guard{&a.lock, &a.lock};
      EXPECT_TRUE(guard);
      EXPECT_EQ(LockResult::Reentrance, test::GetLastResult(guard));
    }
  }

  // Test that each lock in a structure behaves as an individual lock class.
  {
    MultipleLocks value{};

    {
      Guard<Mutex> guard_a{&value.lock_a};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<Mutex> guard_b{&value.lock_b};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));
    }

    {
      Guard<Mutex> guard_b{&value.lock_b};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));

      Guard<Mutex> guard_a{&value.lock_a};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::OutOfOrder, test::GetLastResult(guard_a));
    }
  }

  // Test multi-acquire rule. Re-acquiring the same lock class is allowed,
  // however, ordering with other locks is still enforced.
  {
    Baz<Mutex, LockFlagsMultiAcquire> a;
    Baz<Mutex, LockFlagsMultiAcquire> b;
#if 0 || TEST_WILL_NOT_COMPILE
    // Test mutually exclusive flags fail to compile.
    Baz<Mutex, LockFlagsMultiAcquire | LockFlagsNestable> c;
    Baz<Mutex, LockFlagsMultiAcquire | LockFlagsReAcquireFatal> d;
#endif

    // Use a unique lock class for each of these order tests.
    Foo before;
    Bar after;
    Baz<Mutex> between;

    // Test re-acquiring the same lock class.
    {
      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));
    }

    // Test ordering with another lock class before this one.
    {
      Guard<Mutex> guard_before{&before.lock};
      EXPECT_TRUE(guard_before);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_before));

      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));
    }
    {
      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<Mutex> guard_before{&before.lock};
      EXPECT_TRUE(guard_before);
      EXPECT_EQ(LockResult::OutOfOrder, test::GetLastResult(guard_before));
    }
    {
      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));

      // Subsequent violations are not reported.
      Guard<Mutex> guard_before{&before.lock};
      EXPECT_TRUE(guard_before);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_before));
    }

    // Test ordering with another lock class after this one.
    {
      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));

      Guard<Mutex> guard_after{&after.lock};
      EXPECT_TRUE(guard_after);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_after));
    }
    {
      Guard<Mutex> guard_after{&after.lock};
      EXPECT_TRUE(guard_after);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_after));

      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::OutOfOrder, test::GetLastResult(guard_a));
    }
    {
      Guard<Mutex> guard_after{&after.lock};
      EXPECT_TRUE(guard_after);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_after));

      // Subsequent violations are not reported.
      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));
    }

    // Test ordering with another lock class between this one.
    {
      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<Mutex> guard_between{&between.lock};
      EXPECT_TRUE(guard_between);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_between));

      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::OutOfOrder, test::GetLastResult(guard_b));
    }
    {
      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));

      Guard<Mutex> guard_between{&between.lock};
      EXPECT_TRUE(guard_between);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_between));

      // Subsequent violations are not reported.
      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));
    }
  }

  // Test circular dependency detection.
  {
    Number<1> a{};  // Node A.
    Number<2> b{};  // Node B.
    Number<3> c{};  // Node C.
    Number<4> d{};  // Node D.

    // A -> B
    {
      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));
    }

    // B -> C
    {
      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));

      Guard<Mutex> guard_c{&c.lock};
      EXPECT_TRUE(guard_c);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_c));
    }

    // C -> A -- cycle in (A, B, C)
    {
      Guard<Mutex> guard_c{&c.lock};
      EXPECT_TRUE(guard_c);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_c));

      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));
    }

    // C -> D
    {
      Guard<Mutex> guard_c{&c.lock};
      EXPECT_TRUE(guard_c);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_c));

      Guard<Mutex> guard_d{&d.lock};
      EXPECT_TRUE(guard_d);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_d));
    }

    // D -> A -- cycle in (A, B, C, D)
    {
      Guard<Mutex> guard_d{&d.lock};
      EXPECT_TRUE(guard_d);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_d));

      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));
    }

#if WITH_LOCK_DEP
    // Ensure that the loop detection pass completes before the test ends to
    // avoid triggering lockdep failures in CQ/CI. Use an infinite timeout and
    // let test infra kill the test due to timeout instead.
    zx_status_t status = TriggerAndWaitForLoopDetection(ZX_TIME_INFINITE);
    EXPECT_EQ(ZX_OK, status);
#endif
  }

  // Test CallUntracked operation.
  {
    Number<0> a;
    Number<1> b;

    // A -> B
    {
      Guard<Mutex> guard_a{&a.lock};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));
    }
    // Now acquire B -> A using untracked to make it work.
    {
      Guard<Mutex> guard_b{&b.lock};
      EXPECT_TRUE(guard_b);
      guard_b.CallUntracked([&] {
        // B should still be held.
        ASSERT(guard_b);
        Guard<Mutex> guard_a{&a.lock};
      });
    }
    // Should be able to AssertNoLocks held as well.
    {
      Guard<Mutex> guard_a{&a.lock};
      guard_a.CallUntracked([&] {
        ASSERT(guard_a);
        AssertNoLocksHeld();
      });
    }
  }

  // Test aliased locks.
  {
    test::Resource resource_a;
    test::Resource resource_b;
    test::Object object_a{&resource_a};
    test::Object object_b{&resource_a};
    test::Object object_c{&resource_b};
    test::Object object_d{&resource_b};

    object_a.Operation();
    object_b.Operation();
    object_c.Operation();
    object_d.Operation();

    {
      Guard<Mutex> guard_a{AliasedLock, object_a.lock(), object_b.lock()};
      EXPECT_TRUE(guard_a);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_a));

      object_a.OperationLocked();
      object_b.OperationLocked();

#if TEST_WILL_NOT_COMPILE || 0
      object_c.OperationLocked();
      object_d.OperationLocked();
#endif
    }
    {
      Guard<Mutex> guard_b{AliasedLock, object_c.lock(), object_d.lock()};
      EXPECT_TRUE(guard_b);
      EXPECT_EQ(LockResult::Success, test::GetLastResult(guard_b));

      object_c.OperationLocked();
      object_d.OperationLocked();

#if TEST_WILL_NOT_COMPILE || 0
      object_a.OperationLocked();
      object_b.OperationLocked();
#endif
    }
    {
#if TEST_WILL_DEBUG_ASSERT || 0
      Guard<Mutex> guard{AliasedLock, object_a.lock(), object_c.lock()};
#endif
    }

    object_a.Operation();
    object_b.Operation();
    object_c.Operation();
    object_d.Operation();

    // The compiler will allow this unsafe operation becasue of the lock assertion error.
    object_a.HazardousMerge(&object_b);

    // This is safe because of the use of AliasedLock Guard.
    object_a.SafeMerge(&object_b);
  }

  // With all locks released, regardless of what we did in the middle, should still detect no locks
  // held.
  AssertNoLocksHeld();

  // Reset the tracking state to ensure that circular dependencies are not
  // reported outside of the test.
  test::ResetTrackingState();

  END_TEST;
}

// Basic compile-time tests of lockdep clang lock annotations.
static bool lock_dep_static_analysis_tests() {
  BEGIN_TEST;

  using lockdep::Guard;
  using lockdep::GuardMultiple;
  using lockdep::LockResult;
  using lockdep::ThreadLockState;
  using test::Bar;
  using test::Baz;
  using test::Foo;
  using test::MultipleLocks;
  using test::Mutex;
  using test::Nestable;
  using test::Number;
  using test::ReadWriteLock;
  using test::Spinlock;
  using test::TryNoIrqSave;

  // Test require and exclude annotations.
  {
    Foo a{};

    Guard<Mutex> guard_a{&a.lock};
    a.TestRequire();
#if TEST_WILL_NOT_COMPILE || 0
    a.TestExclude();
#endif

    guard_a.Release();
#if TEST_WILL_NOT_COMPILE || 0
    a.TestRequire();
#endif
    a.TestExclude();
  }

  // Test multiple acquire.
  {
    Foo a{};

    Guard<Mutex> guard_a{&a.lock};
#if TEST_WILL_NOT_COMPILE || 0
    Guard<Mutex> guard_b{&a.lock};
#endif
  }

  // Test sequential acquire/release.
  {
    Foo a{};

    Guard<Mutex> guard_a{&a.lock};
    guard_a.Release();
    Guard<Mutex> guard_b{&a.lock};
  }

  // Test shared.
  {
    Baz<ReadWriteLock> a{};

    Guard<ReadWriteLock, ReadWriteLock::Read> guard_a{&a.lock};
    a.TestShared();
#if TEST_WILL_NOT_COMPILE || 0
    a.TestRequire();
#endif
  }

  {
    Baz<ReadWriteLock> a{};

    Guard<ReadWriteLock, ReadWriteLock::Write> guard_a{&a.lock};
    a.TestShared();
    a.TestRequire();
  }

  END_TEST;
}

UNITTEST_START_TESTCASE(lock_dep_tests)
UNITTEST("lock_dep_dynamic_analysis_tests", lock_dep_dynamic_analysis_tests)
UNITTEST("lock_dep_static_analysis_tests", lock_dep_static_analysis_tests)
UNITTEST_END_TESTCASE(lock_dep_tests, "lock_dep_tests", "lock_dep_tests")
#endif

#if WITH_LOCK_DEP

// The test outline, and test states.
//
// The overall goal of this test is to engineer the following situation.
//
// 1) A thread T is inside of a spinlock X, with interrupts disabled.
// 2) Before dropping X, an IRQ becomes pending for the CPU which T is running
//    on.
// 3) T drops the lock and re-enables interrupts, then immediately ends up in an
//    IRQ which also wants to obtain X.
//
// When bug 84827 was still around, what would happen is that T would drop the
// lock and re-enable interrupts _before_ updating its lockdep bookkeeping to
// indicate that X was no longer owned by T.  When IRQs are re-enabled, and the
// IRQ handler attempts to obtain X, it thinks that we are still holding X, and
// it complains.
//
// Setting up this situation actually involves a bit of work, and needs to be
// done very carefully in order to prevent deadlock.  Basically, before the test
// can start, we need to set up a situation where there are two threads running
// on two different CPUs, each with interrupts disabled.  The "primary" CPU
// should be holding X, and the secondary CPU should post an IPI to the primary
// which causes the primary CPU to attempt to obtain X as soon as CPUs are
// re-enabled.
//
// The main test is pretty easy.  The primary enters the lock and sets a
// variable indicating it is ready for the test to start, then starts to wait
// for the secondary to tell it to go.  The secondary waits until the primary is
// ready, then sends it a task via mp_sync_exec (which sends an IPI).  The task
// will be run at IRQ time as soon as interrupts are re-enabled, and will cause
// the primary to bounce through the lock.  Finally, the secondary sets the
// state variable telling the primary to go, and then unwinds.
//
// See the notes in the test (below) for details of how we set up the initial
// situation without accidentally deadlocking (also, see https://fxbug.dev/42070437)
//
static bool bug_84827_regression_test() {
  BEGIN_TEST;

  cpu_mask_t online_cpus = mp_get_online_mask();
  if (ktl::popcount(online_cpus) < 2) {
    printf("Skipping test, must have at least 2 CPUs online\n");
    return true;
  }

  // See above for an outline of the overall test.  The comments here are meant
  // to describe the deadlock hazard, and how we avoid it.
  //
  // === The Hazard ===
  //
  // On x64, any time we need to update the PTEs to invalidate a TLB entry, we
  // need to make sure that all CPUs in the system take note of the
  // invalidation, and invalidate any entries in their CPU specific caches
  // before the invalidation operation can finish.
  //
  // When this "TLB-shootdown" needs to take place at IRQ time (usually during a
  // page-fault handler), we end up with a CPU with IRQs off which needs to send
  // a task to all other CPUs (via an IPI signal), and then spin (with IRQs off)
  // before it can finish the fault handler.
  //
  // Given the setup of the main test (above), if we have the primary CPU
  // sitting with interrupts off waiting for the secondary to signal it, but the
  // CPU the secondary is running on takes a page fault and ends up waiting for
  // all other CPUs to acknowledge the TLB-shootdown, we end up deadlocking.
  // The secondary CPU can't tell the primary to go because the shootdown is
  // happening.  The shootdown cannot finish, because the primary is waiting
  // with interrupts off for the secondary to tell it to go.
  //
  // === Avoiding the Hazard ===
  //
  // What we want is to have both threads ready to start the main test, but with
  // interrupts off ensuring that the test will run to completion even if a page
  // fault happens and a synchronous TLB shootdown needs to take place.  Here is
  // the sequence we use.
  //
  // The main test thread starts by picks a CPU and takes on the role of the
  // secondary, setting affinity for and migrating to the CPU it chose (CPU-S).
  // It then creates a thread to become the secondary which will (as soon as it
  // starts) set affinity for the primary CPU (CPU-P).
  //
  // The primary now does the following:
  //
  // 1) Enters the lock, disabling interrupts in the process.
  // 2) Sets the state to WaitingForSecondaryReady.  Now the secondary can tell
  //    that the primary has started, is running on the proper CPU, has interrupts
  //    off, and is ready to start the test.
  // 3) Now it spin waits for the secondary to declare that it is ready by
  //    setting the state to SecondaryIsReady.
  // 4) If a small amount of time has passed, and the secondary is still not
  //    ready, the primary will back up.  Specifically it will:
  // 4.1) CMPX the state variable, expecting WaitingForSecondaryReady, and
  //      attempting to exchange for WaitingForThreadStart.
  // 4.2) If the CMPX succeeds, then it can drop the lock (allowing any pending
  //      IRQs to be processed), re-enter the lock, then goto 2, starting the
  //      process over again.
  // 4.3) If the CMPX fails, then the secondary must have made it to the
  //      check-in point and observed that we are also ready.  We can simply
  //      proceed to #5.
  // 5) The primary is now ready to start the test.
  //
  // Meanwhile, the secondary is doing the following.
  //
  // 1) Start by shutting off interrupts.
  // 2) Attempt to CMPX the state variable, exchanging WaitingForSecondaryReady
  //    for SecondaryIsReady.
  // 2.1) If this succeeds, the test is ready to start and we can we can proceed
  //      to #3.
  // 2.2) If the CMPX fails, it must be because the primary has either not
  //      started, or it has rolled the state back.
  // 2.3) So, we check to see if we have been waiting for a while, and if so, we
  //      re-enable interrupts, allowing IRQs to be processed, then we goto #1
  //      and start again.
  // 2.4) If we have not timed out, we simply goto #2, starting the check again.
  // 3) The main test is now ready to start.
  //
  // Overall, if either thread gets stuck with interrupts off waiting for its
  // partner to show up, it will eventually re-enable interrupts (potentially
  // rolling the state machine back in the process) allowing any TLB shootdowns
  // to finish before starting its cycle again.
  //
  // One final note.  We don't _technically_ need to disable interrupts on the
  // secondary CPU.  It should be sufficient to simply disable preemption.
  // This, however, assumes that it is not possible for the kernel to ever take
  // any interrupt while preemption is disabled which would require an IPI which
  // synchronizes with the primary CPU.
  //
  // At the time that this comment was written, the only known potential cause
  // of this would have been a page fault taken on x64 from a user-mode process.
  // The kernel should never be taking any page faults in this context.  That
  // said, if (someday) this assumption becomes invalid, or if there is _any_
  // other reason that the secondary CPU might take a interrupt which requires
  // synchronization with the primary, it could cause problems.  Since this is
  // not production code, we choose to take the future-proofed option and simply
  // shut interrupts off instead.
  //
  enum class TestState : uint32_t {
    WaitingForThreadStart,
    WaitingForSecondaryReady,
    SecondaryIsReady,
    ThreadWaitingInLock,
    IPIHasBeenQueued,
  };

  struct TestArgs {
    DECLARE_SPINLOCK(TestArgs) lock;
    cpu_mask_t first_cpu_mask{0};
    cpu_mask_t second_cpu_mask{0};
    ktl::atomic<TestState> state{TestState::WaitingForThreadStart};
  } args;

  // Determine the cores that this test will use.
  for (cpu_num_t i = 0; !args.second_cpu_mask; ++i) {
    if ((online_cpus & cpu_num_to_mask(i)) != 0) {
      if (!args.first_cpu_mask) {
        args.first_cpu_mask = cpu_num_to_mask(i);
      } else {
        args.second_cpu_mask = cpu_num_to_mask(i);
      }
    }
  }

  // Migrate to the secondary CPU we have chosen.  No matter what happens after
  // this, don't forget to set our affinity mask back to what it was before.
  cpu_mask_t prev_affinity = Thread::Current::Get()->GetCpuAffinity();
  auto cleanup =
      fit::defer([prev_affinity]() { Thread::Current::Get()->SetCpuAffinity(prev_affinity); });
  Thread::Current::Get()->SetCpuAffinity(args.second_cpu_mask);

  // Start up our work thread.  It will migrate to our first chosen core and
  // then signal us via the test args state.
  constexpr zx_duration_mono_t kDropLockTimeout = ZX_USEC(100);
  Thread* work_thread = Thread::Create(
      "bug_84827_test_thread",
      [](void* ctx) -> int {
        TestArgs& args = *(reinterpret_cast<TestArgs*>(ctx));

        // Migrate to our chosen cpu.
        Thread::Current::Get()->SetCpuAffinity(args.first_cpu_mask);

        // Enter the lock (disabling interrupts) and signal that we are waiting
        // for our secondary buddy to join us.
        zx_instant_mono_t deadline = current_mono_time() + kDropLockTimeout;
        {  // Scope for our lock
          Guard<SpinLock, IrqSave> guard(&args.lock);
          args.state.store(TestState::WaitingForSecondaryReady);

          // Spin waiting for the signal from our secondary.
          while (args.state.load() != TestState::SecondaryIsReady) {
            // If the deadline has been exceeded, attempt to back up the state
            // machine.
            if (current_mono_time() >= deadline) {
              TestState expected = TestState::WaitingForSecondaryReady;

              // Try to exchange WaitingForSecondaryReady for
              // WaitingForThreadStart.
              if (args.state.compare_exchange_strong(expected, TestState::WaitingForThreadStart)) {
                // We succeeded in backing up the state machine.  Drop the lock
                // briefly, and let any pending interrupts take place.
                guard.CallUnlocked([]() { arch::Yield(); });
                DEBUG_ASSERT(args.state.load() == TestState::WaitingForThreadStart);

                // Now advance the state machine back to waiting for secondary,
                // reset our deadline, and go back to waiting for the secondary
                // to join us.
                args.state.store(TestState::WaitingForSecondaryReady);
                deadline = current_mono_time() + kDropLockTimeout;
              } else {
                // The CMPX succeeded!  We must be in the SecondaryIsReady state
                // at this point.  Break out of our loop so we can start the
                // test.
                DEBUG_ASSERT(expected == TestState::SecondaryIsReady);
                break;
              }
            }
          }

          // Advance to the WaitingInLock state, signaling to the secondary that
          // the main test has started.  Then wait until the secondary tells us
          // that an IPI has been queued so we can drop the lock and conclude
          // the test.
          args.state.store(TestState::ThreadWaitingInLock);
          while (args.state.load() != TestState::IPIHasBeenQueued) {
            // empty body, just spin.
          }
        }
        return 0;
      },
      &args, DEFAULT_PRIORITY);
  ASSERT_NONNULL(work_thread);
  work_thread->Resume();

  // Make sure we clean up our thread when we are done.
  auto cleanup_thread = fit::defer([&work_thread]() {
    int retcode;
    work_thread->Thread::Join(&retcode, ZX_TIME_INFINITE);
    work_thread = nullptr;
  });

  // Start by turning off interrupts and joining up with our primary buddy.
  {  // Explicit scope for the RAII guard.
    InterruptDisableGuard irqd;
    zx_instant_mono_t deadline = current_mono_time() + kDropLockTimeout;

    while (true) {
      // Try to advance from WaitingForSecondaryReady to SecondaryIsReady.  If
      // this succeeds, we can break out of the loop and start the main test.
      TestState expected = TestState::WaitingForSecondaryReady;
      if (args.state.compare_exchange_strong(expected, TestState::SecondaryIsReady)) {
        break;
      }

      // We have exceeded our deadline waiting for our primary.  Re-enable
      // interrupts briefly, allowing any pending IRQs to take place.  Then shut
      // them off again, reset our deadline, and go back to waiting.
      if (current_mono_time() >= deadline) {
        arch_disable_ints();
        arch::Yield();
        arch_enable_ints();
        deadline = current_mono_time() + kDropLockTimeout;
      }
    }

    // Wait until we know that our work_thread is in the waiting state.
    while (args.state.load() != TestState::ThreadWaitingInLock) {
    }

    // OK, our work thread is now inside of the test spinlock and spinning,
    // waiting for us to release it by setting the test state variable to
    // IPIHasBeenQueued.
    //
    // We now schedule a small lambda to run at IRQ time on both our core, as well
    // as the core the work_thread is on, using mp_sync_exec.
    //
    // What should happen here is that the task sent to the work_thread will become
    // pending, but not able to run because the thread we just started is spinning
    // in the spinlock.  _After_ this task has been queued and the IPI sent, the
    // local core will run its copy of the task.  Note that this is currently a
    // side effect of the behavior of mp_sync_exec, but one that we depend on.
    // The order is important here.
    //
    // The local core will spin for just a little bit (to make absolutely sure
    // that the IPI has been registered with the other core) and then release the
    // work thread by changing the state variable.
    //
    // When the work thread eventually drops the lock, the IPI should immediately
    // fire and cause the thread to re-obtain the lock again.  If the lockdep
    // bookkeeping has not been cleared at this point in time, it will appear to
    // the lockdep code that our thread is re-entering a lock it is already
    // holding (even though the lock has already been dropped) triggering an OOPS.
    mp_sync_exec(
        mp_ipi_target::MASK, args.first_cpu_mask | args.second_cpu_mask,
        [](void* ctx) {
          TestArgs& args = *(reinterpret_cast<TestArgs*>(ctx));

          if (args.first_cpu_mask == Thread::Current::Get()->GetCpuAffinity()) {
            bool prev_state = arch_blocking_disallowed();
            arch_set_blocking_disallowed(false);
            {
              Guard<SpinLock, IrqSave> guard(&args.lock);
            }
            arch_set_blocking_disallowed(prev_state);
          } else {
            zx_instant_mono_t spin_deadline = current_mono_time() + ZX_USEC(10);
            while (current_mono_time() < spin_deadline) {
              // empty body, just spin.
            }
            args.state.store(TestState::IPIHasBeenQueued);
          }
        },
        &args);
  }

  END_TEST;
}

UNITTEST_START_TESTCASE(lock_dep_regression_tests)
UNITTEST("Bug #84827 regression test", bug_84827_regression_test)
UNITTEST_END_TESTCASE(lock_dep_regression_tests, "lock_dep_regression_tests",
                      "lock_dep_regression_tests")
#endif
