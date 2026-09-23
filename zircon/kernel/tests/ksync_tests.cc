// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <stdint.h>

#include <kernel/brwlock.h>
#include <kernel/mutex.h>
#include <kernel/spinlock.h>
#include <lockdep/lockdep.h>

DECLARE_SINGLETON_MUTEX(CppTestSingletonMutex);
DECLARE_SINGLETON_CRITICAL_MUTEX(CppTestSingletonCriticalMutex);

extern "C" {

bool cpp_verify_mutex_id(const void* lock_ptr, const void* expected_id);
bool cpp_verify_critical_mutex_id(const void* lock_ptr, const void* expected_id);
bool cpp_verify_spinlock_id(const void* lock_ptr, const void* expected_id);
bool cpp_verify_monitored_spinlock_id(const void* lock_ptr, const void* expected_id);
bool cpp_verify_brwlock_id(const void* lock_ptr, const void* expected_id);

const void* cpp_get_test_singleton_mutex_ptr();
size_t cpp_get_test_singleton_mutex_size();
const void* cpp_get_test_singleton_mutex_raw_lock_ptr();
size_t cpp_get_test_singleton_mutex_raw_lock_size();

const void* cpp_get_test_singleton_critical_mutex_ptr();
size_t cpp_get_test_singleton_critical_mutex_size();
const void* cpp_get_test_singleton_critical_mutex_raw_lock_ptr();
size_t cpp_get_test_singleton_critical_mutex_raw_lock_size();

bool cpp_verify_mutex_id(const void* lock_ptr, const void* expected_id) {
  const auto* lock = static_cast<const lockdep::Lock<Mutex>*>(lock_ptr);
  return lock->id() == reinterpret_cast<lockdep::LockClassId>(expected_id);
}

bool cpp_verify_critical_mutex_id(const void* lock_ptr, const void* expected_id) {
  const auto* lock = static_cast<const lockdep::Lock<CriticalMutex>*>(lock_ptr);
  return lock->id() == reinterpret_cast<lockdep::LockClassId>(expected_id);
}

bool cpp_verify_spinlock_id(const void* lock_ptr, const void* expected_id) {
  const auto* lock = static_cast<const lockdep::Lock<SpinLock>*>(lock_ptr);
  return lock->id() == reinterpret_cast<lockdep::LockClassId>(expected_id);
}

bool cpp_verify_monitored_spinlock_id(const void* lock_ptr, const void* expected_id) {
  const auto* lock = static_cast<const lockdep::Lock<MonitoredSpinLock>*>(lock_ptr);
  return lock->id() == reinterpret_cast<lockdep::LockClassId>(expected_id);
}

bool cpp_verify_brwlock_id(const void* lock_ptr, const void* expected_id) {
  const auto* lock = static_cast<const lockdep::Lock<BrwLockPi>*>(lock_ptr);
  return lock->id() == reinterpret_cast<lockdep::LockClassId>(expected_id);
}

const void* cpp_get_test_singleton_mutex_ptr() { return CppTestSingletonMutex::Get(); }

size_t cpp_get_test_singleton_mutex_size() { return sizeof(*CppTestSingletonMutex::Get()); }

const void* cpp_get_test_singleton_mutex_raw_lock_ptr() {
  return &CppTestSingletonMutex::Get()->lock();
}

size_t cpp_get_test_singleton_mutex_raw_lock_size() { return sizeof(Mutex); }

const void* cpp_get_test_singleton_critical_mutex_ptr() {
  return CppTestSingletonCriticalMutex::Get();
}

size_t cpp_get_test_singleton_critical_mutex_size() {
  return sizeof(*CppTestSingletonCriticalMutex::Get());
}

const void* cpp_get_test_singleton_critical_mutex_raw_lock_ptr() {
  return &CppTestSingletonCriticalMutex::Get()->lock();
}

size_t cpp_get_test_singleton_critical_mutex_raw_lock_size() { return sizeof(CriticalMutex); }

}  // extern "C"
