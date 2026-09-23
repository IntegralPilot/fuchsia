// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/sync/cpp/mutex.h>
#include <lib/zx/time.h>

#include <atomic>
#include <mutex>

#include <zxtest/zxtest.h>

// These tests are meant as simple smoke tests for the C++ wrapper.
// The comprehensive tests for |sync_mutex_t| are defined in mutex.cc.
namespace {

TEST(MutexWrapper, LockUnlock) {
  libsync::Mutex mutex;
  mutex.lock();
  std::atomic_bool flag{false};
  std::thread wait_thread([&] {
    mutex.lock();
    flag.store(true);
    mutex.unlock();
  });
  zx::nanosleep(zx::deadline_after(zx::msec(100)));
  EXPECT_FALSE(flag.load());
  mutex.unlock();
  wait_thread.join();
  EXPECT_TRUE(flag.load());
}

// In the following tests, we do not use `ASSERT_...` macros or `FAIL...` macros
// because they introduce control flow changes that the thread safety analyzer
// cannot understand.

TEST(MutexWrapper, TryLock) {
  libsync::Mutex mutex;
  if (!mutex.try_lock()) {
    ADD_FAILURE("Mutex try_lock failed when it should have succeeded");
    return;
  }
  EXPECT_FALSE(mutex.try_lock());
  mutex.unlock();
}

TEST(MutexWrapper, BasicLockable) {
  libsync::Mutex mutex;
  std::lock_guard guard(mutex);
}

TEST(MutexWrapper, Get) {
  libsync::Mutex mutex;
  sync_mutex_t* c_mutex = mutex.get();

  sync_mutex_lock(c_mutex);
  if (mutex.try_lock()) {
    mutex.unlock();
    ADD_FAILURE("Mutex try_lock succeeded when it should have failed");
  }

  sync_mutex_unlock(c_mutex);
  if (!mutex.try_lock()) {
    ADD_FAILURE("Mutex try_lock failed when it should have succeeded");
    return;
  }
  mutex.unlock();
}

}  // namespace
