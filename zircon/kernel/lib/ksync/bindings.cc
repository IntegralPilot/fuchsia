// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stddef.h>

#include <new>

#include <kernel/brwlock.h>
#include <kernel/ffi.h>
#include <kernel/lock_validation_guard.h>
#include <kernel/mutex.h>
#include <kernel/spinlock.h>
#include <lockdep/lockdep.h>

class OwnedWaitQueue;

#ifdef WITH_LOCK_DEP
constexpr bool kWithLockDep = true;
#else
constexpr bool kWithLockDep = false;
#endif

constexpr size_t kExpectedMutexSize =
    kWithLockDep ? ((kSchedulerLockSpinTracingEnabled || kLockNameTracingEnabled) ? 40 : 32)
                 : ((kSchedulerLockSpinTracingEnabled || kLockNameTracingEnabled) ? 32 : 24);

#if WITH_LOCK_DEP
using SystemLockType = lockdep::LockDep<void, Mutex>;
#else
using SystemLockType = Mutex;
#endif
static_assert(sizeof(SystemLockType) == kExpectedMutexSize,
              "Rust KMutex size mismatch with C++ size");
static_assert(alignof(SystemLockType) == 8, "Mutex alignment must be exactly 8 bytes.");

#if WITH_LOCK_DEP
using SystemSpinlockType = lockdep::LockDep<void, SpinLock>;
#else
using SystemSpinlockType = SpinLock;
#endif

constexpr size_t kExpectedSpinlockSize = kWithLockDep ? (kSchedulerLockSpinTracingEnabled ? 24 : 16)
                                                      : (kSchedulerLockSpinTracingEnabled ? 16 : 4);

static_assert(sizeof(SystemSpinlockType) == kExpectedSpinlockSize,
              "Rust KSpinlock size mismatch with C++ size");

constexpr size_t kExpectedSpinlockAlign =
    (kWithLockDep || kSchedulerLockSpinTracingEnabled) ? 8 : 4;
static_assert(alignof(SystemSpinlockType) == kExpectedSpinlockAlign,
              "Rust KSpinlock alignment mismatch with C++ alignment");

#if WITH_LOCK_DEP
using SystemMonitoredSpinlockType = lockdep::LockDep<void, MonitoredSpinLock>;
#else
using SystemMonitoredSpinlockType = MonitoredSpinLock;
#endif

static_assert(sizeof(SystemMonitoredSpinlockType) == kExpectedSpinlockSize,
              "Rust KMonitoredSpinlock size mismatch with C++ size");
static_assert(alignof(SystemMonitoredSpinlockType) == kExpectedSpinlockAlign,
              "Rust KMonitoredSpinlock alignment mismatch with C++ alignment");

static_assert(sizeof(lockdep::AcquiredLockEntry) == 40, "AcquiredLockEntry size mismatch");
static_assert(alignof(lockdep::AcquiredLockEntry) == 8,
              "AcquiredLockEntry alignment must be exactly 8 bytes.");

#if defined(__x86_64__)
constexpr size_t kExpectedInterruptSavedStateSize = 8;
constexpr size_t kExpectedInterruptSavedStateAlign = 8;
#else
constexpr size_t kExpectedInterruptSavedStateSize = 1;
constexpr size_t kExpectedInterruptSavedStateAlign = 1;
#endif

static_assert(sizeof(interrupt_saved_state_t) == kExpectedInterruptSavedStateSize,
              "Rust InterruptSavedState size mismatch with C++");
static_assert(alignof(interrupt_saved_state_t) == kExpectedInterruptSavedStateAlign,
              "Rust InterruptSavedState alignment mismatch with C++");
#if WITH_LOCK_DEP
using SystemBrwLockType = lockdep::LockDep<void, BrwLockPi, lockdep::LockFlagsMultiAcquire>;
#else
using SystemBrwLockType = BrwLockPi;
#endif

#if defined(__riscv)
constexpr size_t kExpectedBrwLockSize = kWithLockDep ? 80 : 72;
constexpr size_t kExpectedBrwLockAlign = 8;
#else
constexpr size_t kExpectedBrwLockSize = kWithLockDep ? 144 : 128;
constexpr size_t kExpectedBrwLockAlign = 16;
#endif

static_assert(sizeof(SystemBrwLockType) == kExpectedBrwLockSize, "SystemBrwLockType size mismatch");
static_assert(alignof(SystemBrwLockType) == kExpectedBrwLockAlign,
              "SystemBrwLockType alignment mismatch");

#if WITH_LOCK_DEP
template <typename T>
using LockPtr = lockdep::Lock<T>*;

template <typename T>
struct LockInitHelper : public lockdep::Lock<T> {
  LockInitHelper(lockdep::LockClassId id) : lockdep::Lock<T>(id) {}
};
#else
template <typename T>
using LockPtr = T*;
#endif

extern "C" {

void cpp_mutex_destroy(LockPtr<Mutex> mutex);
void cpp_mutex_acquire(LockPtr<Mutex> lock, void* entry_storage);
void cpp_mutex_release(LockPtr<Mutex> lock, void* entry_storage);
void cpp_critical_mutex_destroy(LockPtr<CriticalMutex> mutex);
bool cpp_critical_mutex_acquire(LockPtr<CriticalMutex> lock, void* entry_storage);
void cpp_critical_mutex_release(LockPtr<CriticalMutex> lock, void* entry_storage,
                                bool should_clear);
void cpp_spinlock_init(LockPtr<SpinLock> lock, const void* class_id);
void cpp_spinlock_destroy(LockPtr<SpinLock> lock);
interrupt_saved_state_t cpp_spinlock_acquire_irqsave(LockPtr<SpinLock> lock, void* entry_storage);
void cpp_spinlock_release_irqrestore(LockPtr<SpinLock> lock, void* entry_storage,
                                     interrupt_saved_state_t state);
void cpp_spinlock_acquire_no_irqsave(LockPtr<SpinLock> lock, void* entry_storage);
void cpp_spinlock_release_no_irqrestore(LockPtr<SpinLock> lock, void* entry_storage);
void cpp_monitored_spinlock_init(LockPtr<MonitoredSpinLock> lock, const void* class_id);
void cpp_monitored_spinlock_destroy(LockPtr<MonitoredSpinLock> lock);
interrupt_saved_state_t cpp_monitored_spinlock_acquire_irqsave(LockPtr<MonitoredSpinLock> lock,
                                                               void* entry_storage,
                                                               const char* name);
void cpp_monitored_spinlock_release_irqrestore(LockPtr<MonitoredSpinLock> lock, void* entry_storage,
                                               interrupt_saved_state_t state);
void cpp_monitored_spinlock_acquire_no_irqsave(LockPtr<MonitoredSpinLock> lock, void* entry_storage,
                                               const char* name);
void cpp_monitored_spinlock_release_no_irqrestore(LockPtr<MonitoredSpinLock> lock,
                                                  void* entry_storage);
void cpp_brwlock_pi_init(LockPtr<BrwLockPi> lock, const void* class_id);
void cpp_brwlock_pi_destroy(LockPtr<BrwLockPi> lock);
void cpp_brwlock_pi_acquire_read(LockPtr<BrwLockPi> lock, void* entry_storage);
void cpp_brwlock_pi_release_read(LockPtr<BrwLockPi> lock, void* entry_storage);
void cpp_brwlock_pi_acquire_write(LockPtr<BrwLockPi> lock, void* entry_storage);
void cpp_brwlock_pi_release_write(LockPtr<BrwLockPi> lock, void* entry_storage);
void cpp_lock_validate_release(void* entry_storage);
void cpp_lock_validate_acquire(void* entry_storage);

FFI_ALWAYS_INLINE void cpp_mutex_destroy(LockPtr<Mutex> lock) {
#if WITH_LOCK_DEP
  using LockType = lockdep::Lock<Mutex>;
  lock->~LockType();
#else
  lock->~Mutex();
#endif
}

FFI_ALWAYS_INLINE void cpp_mutex_acquire(LockPtr<Mutex> lock,
                                         void* entry_storage) TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = new (entry_storage) lockdep::AcquiredLockEntry(&lock->lock(), lock->id(), 0);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsNone)->Acquire(entry);
  }
  lock->lock().Acquire();
#else
  lock->Acquire();
#endif
}

FFI_ALWAYS_INLINE void cpp_mutex_release(LockPtr<Mutex> lock,
                                         void* entry_storage) TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsNone)->Release(entry);
    entry->~AcquiredLockEntry();
  }
  lock->lock().Release();
#else
  lock->Release();
#endif
}

FFI_ALWAYS_INLINE void cpp_critical_mutex_destroy(LockPtr<CriticalMutex> lock) {
#if WITH_LOCK_DEP
  using LockType = lockdep::Lock<CriticalMutex>;
  lock->~LockType();
#else
  lock->~CriticalMutex();
#endif
}

FFI_ALWAYS_INLINE bool cpp_critical_mutex_acquire(LockPtr<CriticalMutex> lock, void* entry_storage)
    TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = new (entry_storage) lockdep::AcquiredLockEntry(&lock->lock(), lock->id(), 0);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsNone)->Acquire(entry);
  }
  auto should_clear = lock->lock().Acquire();
#else
  auto should_clear = lock->Acquire();
#endif
  return should_clear == CriticalMutex::ShouldClear::Yes;
}

FFI_ALWAYS_INLINE void cpp_critical_mutex_release(LockPtr<CriticalMutex> lock, void* entry_storage,
                                                  bool should_clear) TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsNone)->Release(entry);
    entry->~AcquiredLockEntry();
  }
  lock->lock().Release(should_clear ? CriticalMutex::ShouldClear::Yes
                                    : CriticalMutex::ShouldClear::No);
#else
  lock->Release(should_clear ? CriticalMutex::ShouldClear::Yes : CriticalMutex::ShouldClear::No);
#endif
}

FFI_ALWAYS_INLINE void cpp_spinlock_init(LockPtr<SpinLock> lock, const void* class_id) {
#if WITH_LOCK_DEP
  new (lock) LockInitHelper<SpinLock>(reinterpret_cast<lockdep::LockClassId>(class_id));
#else
  new (lock) SpinLock();
#endif
}

FFI_ALWAYS_INLINE void cpp_spinlock_destroy(LockPtr<SpinLock> lock) {
#if WITH_LOCK_DEP
  using LockType = lockdep::Lock<SpinLock>;
  lock->~LockType();
#else
  lock->~SpinLock();
#endif
}

FFI_ALWAYS_INLINE interrupt_saved_state_t cpp_spinlock_acquire_irqsave(
    LockPtr<SpinLock> lock, void* entry_storage) TA_NO_THREAD_SAFETY_ANALYSIS {
  interrupt_saved_state_t state = arch_interrupt_save();
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    auto* entry = new (entry_storage) lockdep::AcquiredLockEntry(&lock->lock(), lock->id(), 0);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsIrqSafe)->Acquire(entry);
  }
  lock->lock().Acquire();
#else
  lock->Acquire();
#endif
  return state;
}

FFI_ALWAYS_INLINE void cpp_spinlock_release_irqrestore(LockPtr<SpinLock> lock, void* entry_storage,
                                                       interrupt_saved_state_t state)
    TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsIrqSafe)->Release(entry);
    entry->~AcquiredLockEntry();
  }
  lock->lock().ReleaseIrqRestore(state);
#else
  lock->ReleaseIrqRestore(state);
#endif
}

FFI_ALWAYS_INLINE void cpp_spinlock_acquire_no_irqsave(LockPtr<SpinLock> lock, void* entry_storage)
    TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    auto* entry = new (entry_storage) lockdep::AcquiredLockEntry(&lock->lock(), lock->id(), 0);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsIrqSafe)->Acquire(entry);
  }
  lock->lock().Acquire();
#else
  lock->Acquire();
#endif
}

FFI_ALWAYS_INLINE void cpp_spinlock_release_no_irqrestore(
    LockPtr<SpinLock> lock, void* entry_storage) TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsIrqSafe)->Release(entry);
    entry->~AcquiredLockEntry();
  }
  lock->lock().Release();
#else
  lock->Release();
#endif
}

FFI_ALWAYS_INLINE void cpp_monitored_spinlock_init(LockPtr<MonitoredSpinLock> lock,
                                                   const void* class_id) {
#if WITH_LOCK_DEP
  new (lock) LockInitHelper<MonitoredSpinLock>(reinterpret_cast<lockdep::LockClassId>(class_id));
#else
  new (lock) MonitoredSpinLock();
#endif
}

FFI_ALWAYS_INLINE void cpp_monitored_spinlock_destroy(LockPtr<MonitoredSpinLock> lock) {
#if WITH_LOCK_DEP
  using LockType = lockdep::Lock<MonitoredSpinLock>;
  lock->~LockType();
#else
  lock->~MonitoredSpinLock();
#endif
}

FFI_ALWAYS_INLINE interrupt_saved_state_t
cpp_monitored_spinlock_acquire_irqsave(LockPtr<MonitoredSpinLock> lock, void* entry_storage,
                                       const char* name) TA_NO_THREAD_SAFETY_ANALYSIS {
  interrupt_saved_state_t state = arch_interrupt_save();
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    auto* entry = new (entry_storage) lockdep::AcquiredLockEntry(&lock->lock(), lock->id(), 0);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsIrqSafe)->Acquire(entry);
  }
  lock->lock().Acquire(name);
#else
  lock->Acquire(name);
#endif
  return state;
}

FFI_ALWAYS_INLINE void cpp_monitored_spinlock_release_irqrestore(
    LockPtr<MonitoredSpinLock> lock, void* entry_storage,
    interrupt_saved_state_t state) TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsIrqSafe)->Release(entry);
    entry->~AcquiredLockEntry();
  }
  lock->lock().ReleaseIrqRestore(state);
#else
  lock->ReleaseIrqRestore(state);
#endif
}

FFI_ALWAYS_INLINE void cpp_monitored_spinlock_acquire_no_irqsave(
    LockPtr<MonitoredSpinLock> lock, void* entry_storage,
    const char* name) TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    auto* entry = new (entry_storage) lockdep::AcquiredLockEntry(&lock->lock(), lock->id(), 0);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsIrqSafe)->Acquire(entry);
  }
  lock->lock().Acquire(name);
#else
  lock->Acquire(name);
#endif
}

FFI_ALWAYS_INLINE void cpp_monitored_spinlock_release_no_irqrestore(
    LockPtr<MonitoredSpinLock> lock, void* entry_storage) TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsIrqSafe)->Release(entry);
    entry->~AcquiredLockEntry();
  }
  lock->lock().Release();
#else
  lock->Release();
#endif
}

FFI_ALWAYS_INLINE void cpp_brwlock_pi_init(LockPtr<BrwLockPi> lock, const void* class_id) {
#if WITH_LOCK_DEP
  new (lock) LockInitHelper<BrwLockPi>(reinterpret_cast<lockdep::LockClassId>(class_id));
#else
  new (lock) BrwLockPi();
#endif
}

FFI_ALWAYS_INLINE void cpp_brwlock_pi_destroy(LockPtr<BrwLockPi> lock) {
#if WITH_LOCK_DEP
  using LockType = lockdep::Lock<BrwLockPi>;
  lock->~LockType();
#else
  lock->~BrwLockPi();
#endif
}

FFI_ALWAYS_INLINE void cpp_brwlock_pi_acquire_read(LockPtr<BrwLockPi> lock, void* entry_storage)
    TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = new (entry_storage) lockdep::AcquiredLockEntry(&lock->lock(), lock->id(), 0);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsMultiAcquire)->Acquire(entry);
  }
  lock->lock().ReadAcquire();
#else
  lock->ReadAcquire();
#endif
}

FFI_ALWAYS_INLINE void cpp_brwlock_pi_release_read(LockPtr<BrwLockPi> lock, void* entry_storage)
    TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsMultiAcquire)->Release(entry);
    entry->~AcquiredLockEntry();
  }
  lock->lock().ReadRelease();
#else
  lock->ReadRelease();
#endif
}

FFI_ALWAYS_INLINE void cpp_brwlock_pi_acquire_write(LockPtr<BrwLockPi> lock, void* entry_storage)
    TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = new (entry_storage) lockdep::AcquiredLockEntry(&lock->lock(), lock->id(), 0);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsMultiAcquire)->Acquire(entry);
  }
  lock->lock().WriteAcquire();
#else
  lock->WriteAcquire();
#endif
}

FFI_ALWAYS_INLINE void cpp_brwlock_pi_release_write(LockPtr<BrwLockPi> lock, void* entry_storage)
    TA_NO_THREAD_SAFETY_ANALYSIS {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsMultiAcquire)->Release(entry);
    entry->~AcquiredLockEntry();
  }
  lock->lock().WriteRelease();
#else
  lock->WriteRelease();
#endif
}

FFI_ALWAYS_INLINE void cpp_lock_validate_release(void* entry_storage) {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsNone)->Release(entry);
  }
#endif
}

FFI_ALWAYS_INLINE void cpp_lock_validate_acquire(void* entry_storage) {
#if WITH_LOCK_DEP
  if (entry_storage != nullptr) {
    LockValidationGuard guard;
    auto* entry = static_cast<lockdep::AcquiredLockEntry*>(entry_storage);
    lockdep::ThreadLockState::Get(lockdep::LockFlagsNone)->Acquire(entry);
  }
#endif
}

}  // extern "C"
