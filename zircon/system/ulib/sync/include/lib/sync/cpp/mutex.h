// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_SYNC_CPP_MUTEX_H_
#define LIB_SYNC_CPP_MUTEX_H_

#include <lib/sync/mutex.h>
#include <zircon/compiler.h>
#include <zircon/threads.h>

namespace libsync {

// C++ wrapper around |sync_mutex_t|. This can be more efficient than the
// standard C |mtx_t| or C++ |std::mutex|.
//
// It satisfies the BasicLockable C++ requirement.
class __TA_CAPABILITY("mutex") Mutex {
 public:
  constexpr Mutex() = default;
  ~Mutex() = default;
  Mutex(const Mutex&) = delete;
  Mutex& operator=(const Mutex&) = delete;
  Mutex(Mutex&&) = delete;
  Mutex& operator=(Mutex&&) = delete;

  // Locks the mutex.
  //
  // The current thread will block until the mutex is acquired. The mutex is
  // non-recursive, which means attempting to lock a mutex that is already held by
  // this thread will deadlock.
  void lock() __TA_ACQUIRE() { sync_mutex_lock(&mutex_); }

  // Attempts to lock the mutex without blocking.
  //
  // Returns true if the lock is obtained, and false if not.
  bool try_lock() __TA_TRY_ACQUIRE(true) { return sync_mutex_trylock(&mutex_) == ZX_OK; }

  // Unlocks the mutex.
  //
  // Do not unlock the mutex again if it is already unlocked. Do not rely
  // on any behavior from duplicated unlocking.
  void unlock() __TA_RELEASE() { sync_mutex_unlock(&mutex_); }

  // Returns the wrapped |sync_mutex_t|.
  sync_mutex_t* get() __TA_RETURN_CAPABILITY(mutex_) { return &mutex_; }

 private:
  sync_mutex_t mutex_;
};

}  // namespace libsync

#endif  // LIB_SYNC_CPP_MUTEX_H_
