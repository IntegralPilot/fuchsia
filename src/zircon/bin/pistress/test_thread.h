// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_ZIRCON_BIN_PISTRESS_TEST_THREAD_H_
#define SRC_ZIRCON_BIN_PISTRESS_TEST_THREAD_H_

#include <lib/zx/job.h>
#include <lib/zx/profile.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <zircon/assert.h>
#include <zircon/syscalls/profile.h>
#include <zircon/time.h>

#include <array>
#include <atomic>
#include <memory>
#include <thread>

#include <fbl/mutex.h>

#include "behavior.h"
#include "sync_obj.h"

// A class which implements the behavior of the test threads, as well as holding
// the state shared between all test threads (such as the collection of
// synchronization objects that they fight over).
class TestThread {
 public:
  static zx_status_t InitStatics();
  static zx_status_t AddThread(const TestThreadBehavior& behavior);
  static void Shutdown();
  static const auto& threads() { return threads_; }
  static TestThread& random_thread();

  // No copy, no move.
  TestThread(const TestThread&) = delete;
  TestThread(TestThread&&) = delete;
  TestThread& operator=(const TestThread&) = delete;
  TestThread& operator=(TestThread&&) = delete;

  void Start();
  void ChangeProfile();

 private:
  friend class std::default_delete<TestThread>;

  static zx::result<zx::resource> GetSystemProfileResource();

  TestThread(const TestThreadBehavior& behavior, zx::profile profile);
  ~TestThread();

  void Join();
  void Run();
  void HoldLocks(size_t deck_ndx = 0);

  static constexpr size_t kNumMutexes = 28;
  static constexpr size_t kNumCondVars = 4;
  static constexpr size_t kNumSyncObjs = kNumMutexes + kNumCondVars;
  static inline zx::resource profile_rsrc_;
  static inline std::array<std::unique_ptr<SyncObj>, kNumSyncObjs> sync_objs_;
  static inline std::atomic<bool> shutdown_now_{false};
  static std::vector<std::unique_ptr<TestThread>> threads_;
  static inline std::uniform_int_distribution<size_t> thread_dist_{0, 0};

  std::optional<std::thread> thread_;
  TestThreadBehavior behavior_;
  const zx::profile profile_;

  fbl::Mutex profile_lock_;
  __TA_GUARDED(profile_lock_) bool profile_borrowed_ { false };
  __TA_GUARDED(profile_lock_) zx::unowned_thread self_;

  std::array<size_t, kNumSyncObjs> sync_obj_deck_;
  size_t path_len_ = 0;
};

// C++23 std::unique_ptr instantiation rules require this to be defined outside
// the class body, where TestThread is a complete type.
inline std::vector<std::unique_ptr<TestThread>> TestThread::threads_;

#endif  // SRC_ZIRCON_BIN_PISTRESS_TEST_THREAD_H_
