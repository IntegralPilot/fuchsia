// Copyright 2016 The Fuchsia Authors
// Copyright (c) 2008-2014 Travis Geiselbrecht
// Copyright (c) 2012 Shantanu Gupta
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_MUTEX_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_MUTEX_H_

#include <assert.h>
#include <debug.h>
#include <lib/fxt/interned_string.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <stdint.h>
#include <zircon/compiler.h>
#include <zircon/time.h>

#include <fbl/canary.h>
#include <fbl/macros.h>
#include <kernel/lazy_owned_wait_queue.h>
#include <kernel/lock_validation_guard.h>
#include <kernel/lockdep.h>
#include <kernel/owned_wait_queue.h>
#include <kernel/spin_tracing_storage.h>
#include <kernel/thread.h>
#include <ktl/atomic.h>

// Kernel mutex support.
//
class TA_CAP("mutex") Mutex
    : public spin_tracing::LockNameStorage<spin_tracing::LockType::kMutex,
                                           kSchedulerLockSpinTracingEnabled |
                                               kLockNameTracingEnabled> {
 public:
  constexpr Mutex() = default;
  explicit Mutex(const fxt::InternedString& name_stringref)
      : LockNameStorage(name_stringref.id()) {}

  ~Mutex();

  // No moving or copying allowed.
  DISALLOW_COPY_ASSIGN_AND_MOVE(Mutex);

  // The maximum duration to spin before falling back to blocking.
  // TODO(https://fxbug.dev/42109976): Decide how to make this configurable per device/platform
  // and describe how to optimize this value.
  static constexpr zx_duration_t SPIN_MAX_DURATION = ZX_USEC(150);

  // A constant for users of AutoExpiringPreemptDisabler to select a value
  // consistent with the defaults for Mutex/CriticalMutex. Using this constant
  // also improves readablity, providing a link to the motivating use case for
  // timeslice extension.
  static constexpr zx_duration_t DEFAULT_TIMESLICE_EXTENSION = SPIN_MAX_DURATION;

  // Acquire the mutex.
  inline void Acquire(zx_duration_t spin_max_duration = SPIN_MAX_DURATION) TA_ACQ();

  // Release the mutex. Must be held by the current thread.
  //
  // Note: that this simply thunks directly to ReleaseInternal, which does not
  // have any annotations. Methods of capability objects (see TA_CAP, above)
  // which are annotated with TA_ACQ/TA_REL effectively have static thread
  // analysis disabled for them.  It is assumed that they will "do whatever it
  // takes" to either acquire or release the capability.  By thunking to a
  // non-annotated method, we can guarantee that the implementation of the
  // release operation *is* subject to static analysis, helping to guarantee
  // that we are holding (or not holding) all of the proper capabilities when
  // interacting with things like threads, wait queues, and the scheduler.
  void Release() TA_REL() { ReleaseInternal(); }

  // does the current thread hold the mutex?
  bool IsHeld() const { return (holder() == Thread::Current::Get()); }

  // Panic unless the given lock is held.
  //
  // Can be used when thread safety analysis can't prove you are holding
  // a lock. The asserts may be optimized away in release builds.
  void AssertHeld() const TA_ASSERT() { DEBUG_ASSERT(IsHeld()); }

  // <jedi_mindtrick>This is not the method you are looking for.</jedi_mindtrick>
  void MarkReleased() const TA_REL() {}

  // Is the mutex contested i.e. is at least one thread blocked waiting on it?
  //
  // The contested flag does not track threads which are spin-waiting on the
  // Mutex and have yet to enter a blocking phase.
  bool IsContested() const { return val() & STATE_FLAG_CONTESTED; }

 protected:
  // TimesliceExtension is used to control whether a timeslice extension will be
  // set and if so, what value will be used.
  template <bool Enabled>
  struct TimesliceExtension;

  // Used by both Mutex and CriticalMutex.
  //
  // If TimesliceExtensionEnabled is true, attempt to set the timeslice
  // extension after acquiring the mutex and return true if the extension was
  // set.  Otherwise, return false.
  template <bool TimesliceExtensionEnabled>
  bool AcquireCommon(zx_duration_t spin_max_duration,
                     TimesliceExtension<TimesliceExtensionEnabled> timeslice_extension);

 private:
  // The actual implementation of the Release operation.  See above for why this
  // is separate from |Release()|.
  void ReleaseInternal();

  // Attempts to release the mutex. Returns STATE_FREE if the mutex was
  // uncontested and released, otherwise returns the contested state of the
  // mutex.
  inline uintptr_t TryRelease(Thread* current_thread);

  // Acquire a lock held by another thread.
  //
  // This is a slowpath taken by |Acquire| if the mutex is found to already be held
  // by another thread.
  //
  // If TimesliceExtensionEnabled is true, attempt to set the timeslice
  // extension after acquiring the mutex and return true if the extension was
  // set.  Otherwise, return false.
  //
  // This function is deliberately moved out of line from |Acquire| to keep the stack
  // set up, tear down in the |Acquire| fastpath small.
  template <bool TimesliceExtensionEnabled>
  bool AcquireContendedMutex(zx_duration_t spin_max_duration, Thread* current_thread,
                             TimesliceExtension<TimesliceExtensionEnabled> timeslic_extension)
      TA_EXCL(chainlock_transaction_token);

  // Release a lock contended by another thread.
  //
  // This is the slowpath taken by |Release| when releasing a lock that is being
  // waited for by another thread.
  //
  // This function is deliberately moved out of line from |Release| to keep the
  // stack set up, tear down in the |Release| fastpath small.
  void ReleaseContendedMutex(Thread* current_thread, uintptr_t old_mutex_state)
      TA_EXCL(chainlock_transaction_token);

  void RecordInitialAssignedCpu() {
    maybe_acquired_on_cpu_.store(arch_curr_cpu_num(), ktl::memory_order_relaxed);
  }

  void ClearInitialAssignedCpu() {
    maybe_acquired_on_cpu_.store(INVALID_CPU, ktl::memory_order_relaxed);
  }

  static constexpr uint32_t MAGIC = 0x6D757478;  // 'mutx'
  static constexpr uintptr_t STATE_FREE = 0u;
  static constexpr uintptr_t STATE_FLAG_CONTESTED = 1u;

  // Accessors to extract the holder pointer from the val member
  uintptr_t val() const { return val_.load(ktl::memory_order_relaxed); }

  static Thread* holder_from_val(uintptr_t value) {
    return reinterpret_cast<Thread*>(value & ~STATE_FLAG_CONTESTED);
  }

  Thread* holder() const { return holder_from_val(val()); }

  fbl::Canary<MAGIC> magic_;

  // See the comment near the start of AcquireContendedMutex for details about
  // this variable, and how it affects the spin phase behavior of
  // Mutex::AcquireContendedMutex
  ktl::atomic<cpu_num_t> maybe_acquired_on_cpu_{INVALID_CPU};
  ktl::atomic<uintptr_t> val_{STATE_FREE};
  LazyOwnedWaitQueue wait_;
};

// TimeslicExtension specializations for Mutex::Acquire and
// Mutex::AcquireContendedMutex.
//
// No timeslice extension.
template <>
struct Mutex::TimesliceExtension<false> {};
// A timeslice extension of |value| nanoseconds.
template <>
struct Mutex::TimesliceExtension<true> {
  zx_duration_t value;
};

inline void Mutex::Acquire(zx_duration_t spin_max_duration) TA_ACQ() {
  AcquireCommon(spin_max_duration, TimesliceExtension<false>{});
}

// CriticalMutex is a mutex variant that uses a thread timeslice extension to
// disable preemption for a limited time during the critical section.
//
// Acquiring a CriticalMutex sets a timeslice extension that's cleared when the
// mutex is released or the extension expires, whichever comes first.
//
// This variant is useful to avoid a form of thrash where a thread holding a
// mutex is preempted by another thread that subsequently blocks while trying to
// acquire the already held mutex.  CriticalMutex is designed to mitigate this
// type of thrash by preventing the holder from being preempted, up to some
// maximum amount of time.  By limiting maximum amount of time the holder is
// non-preemptible, we can limit the degree to which other, logically higher
// priority tasks are delayed from executing.
//
// Good candidates for CriticalMutex are global or widely shared locks that
// typically, but not necessarily always, have very short critical sections
// (tens of microseconds or less) and high contention under load.
//
// CriticalMutex differs from SpinLock in the following ways:
// * Threads contending a CriticalMutex will block after the spin interval is
//   exceeded, avoiding extended monopolization of multiple CPUs.
// * Threads may block while holding a CriticalMutex, simplifying maintaining
//   invariants in slow paths.
// * Interrupts may remain enabled while holding a CriticalMutex, avoiding
//   undesirable IRQ latency.
//
class TA_CAP("mutex") CriticalMutex : private Mutex {
 public:
  CriticalMutex() = default;
  ~CriticalMutex() = default;
  explicit CriticalMutex(const fxt::InternedString& name_stringref) : Mutex(name_stringref) {}

  CriticalMutex(const CriticalMutex&) = delete;
  CriticalMutex& operator=(const CriticalMutex&) = delete;
  CriticalMutex(CriticalMutex&&) = delete;
  CriticalMutex& operator=(CriticalMutex&&) = delete;

  // Acquire the mutex. Returns whether or not a time slice extension was set, and must be
  // subsequently cleared on Release. More simply, the return value must be passed into the Release
  // method.
  enum class ShouldClear : bool { No = false, Yes = true };
  [[nodiscard]] ShouldClear Acquire(zx_duration_t spin_max_duration = Mutex::SPIN_MAX_DURATION)
      TA_ACQ() {
    // TODO(maniscalco): What's the right duration here?  Is it a function of
    // spin_max_duration?
    const TimesliceExtension<true> timeslice_extension{spin_max_duration};
    return static_cast<ShouldClear>(Mutex::AcquireCommon(spin_max_duration, timeslice_extension));
  }

  // Release the mutex. Must be held by the current thread. The |should_clear| parameter is the
  // return value of the matching |Acquire| call.
  void Release(ShouldClear should_clear) TA_REL() {
    Mutex::Release();

    if (should_clear == ShouldClear::Yes) {
      Thread::Current::preemption_state().ClearTimesliceExtension();
    }
  }

  // See |Mutex::IsHeld|.
  bool IsHeld() const { return Mutex::IsHeld(); }

  // See |Mutex::AssertHeld|.
  void AssertHeld() const TA_ASSERT() { Mutex::AssertHeld(); }

  // See |Mutex::IsContested|.
  bool IsContested() const { return Mutex::IsContested(); }

  // We inherited privately from Mutex, which hides the SetLockClassId method
  // from lockdep.  Explicitly pass the call it makes to SetLockClassId to the
  // underlying implementation so we get named locked when we are collecting
  // lock contention trace data.
  void SetLockClassId(lockdep::LockClassId lcid) { Mutex::SetLockClassId(lcid); }

 private:
};

// Lock policy for kernel mutexes
//
struct MutexPolicy {
  struct State {
    const zx_duration_t spin_max_duration{Mutex::SPIN_MAX_DURATION};
  };

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  // No special actions are needed during pre-validation.
  template <typename LockType>
  static void PreValidate(LockType*, State*) {}

  // Basic acquire and release operations.
  template <typename LockType>
  static bool Acquire(LockType* lock, State* state) TA_ACQ(lock) {
    lock->Acquire(state->spin_max_duration);
    return true;
  }

  template <typename LockType>
  static void Release(LockType* lock, State*) TA_REL(lock) {
    lock->Release();
  }

  // Runtime lock assertions.
  template <typename LockType>
  static void AssertHeld(const LockType& lock) TA_ASSERT(lock) {
    lock.AssertHeld();
  }
};

// Lock policy for critical kernel mutexes.
//
struct CriticalMutexPolicy {
  struct State {
    const uint32_t spin_max_duration{Mutex::SPIN_MAX_DURATION};
    CriticalMutex::ShouldClear should_clear = CriticalMutex::ShouldClear::No;
  };

  // Protects the thread local lock list and validation.
  using ValidationGuard = LockValidationGuard;

  // No special actions are needed during pre-validation.
  template <typename LockType>
  static void PreValidate(LockType*, State*) {}

  // Basic acquire and release operations.
  template <typename LockType>
  static bool Acquire(LockType* lock, State* state) TA_ACQ(lock) {
    state->should_clear = lock->Acquire(state->spin_max_duration);
    return true;
  }

  template <typename LockType>
  static void Release(LockType* lock, State* state) TA_REL(lock) {
    lock->Release(state->should_clear);
  }

  // Runtime lock assertions.
  template <typename LockType>
  static void AssertHeld(const LockType& lock) TA_ASSERT(lock) {
    lock.AssertHeld();
  }
};

// Configure the lockdep::Guard for kernel mutexes to use MutexPolicy.
LOCK_DEP_POLICY(Mutex, MutexPolicy);
LOCK_DEP_POLICY(CriticalMutex, CriticalMutexPolicy);

// Declares a Mutex member of the struct or class |containing_type|.
//
// Example usage:
//
// struct MyType {
//     DECLARE_MUTEX(MyType [, LockFlags]) lock;
// };
//
#define DECLARE_MUTEX(containing_type, ...) \
  LOCK_DEP_INSTRUMENT(containing_type, ::Mutex, ##__VA_ARGS__)

#define DECLARE_CRITICAL_MUTEX(containing_type, ...) \
  LOCK_DEP_INSTRUMENT(containing_type, ::CriticalMutex, ##__VA_ARGS__)

// Declares a |lock_type| member of the struct or class |containing_type|.
//
// Example usage:
//
// struct MyType {
//     DECLARE_LOCK(MyType, LockType [, LockFlags]) lock;
// };
//
#define DECLARE_LOCK(containing_type, lock_type, ...) \
  LOCK_DEP_INSTRUMENT(containing_type, lock_type, ##__VA_ARGS__)

// By default, singleton mutexes in the kernel use Mutex in order to avoid
// a useless global dtor
//
// Example usage:
//
//  DECLARE_SINGLETON_MUTEX(MyGlobalLock [, LockFlags]);
//
#define DECLARE_SINGLETON_MUTEX(name, ...) LOCK_DEP_SINGLETON_LOCK(name, ::Mutex, ##__VA_ARGS__)

#define DECLARE_SINGLETON_CRITICAL_MUTEX(name, ...) \
  LOCK_DEP_SINGLETON_LOCK(name, ::CriticalMutex, ##__VA_ARGS__)

// Declares a singleton |lock_type| with the name |name|.
//
// Example usage:
//
//  DECLARE_SINGLETON_LOCK(MyGlobalLock, LockType, [, LockFlags]);
//
#define DECLARE_SINGLETON_LOCK(name, lock_type, ...) \
  LOCK_DEP_SINGLETON_LOCK(name, lock_type, ##__VA_ARGS__)

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_MUTEX_H_
