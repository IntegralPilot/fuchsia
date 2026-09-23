// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lockdep/lockdep.h>
#include <zxtest/zxtest.h>

namespace {

struct __TA_CAPABILITY("mutex") TestMutex {
  void Acquire() __TA_ACQUIRE() {}
  void Release() __TA_RELEASE() {}
  void AssertHeld() const __TA_ASSERT() {}
};

LOCK_DEP_TRAITS(TestMutex, lockdep::LockFlagsNone);

TEST(ZxLockdepRuntime, ThreadLockStateTracking) {
  struct Container {
    LOCK_DEP_INSTRUMENT(Container, TestMutex) lock;
  } container;

  // Verify we can get the thread lock state and it is not null.
  auto* state = lockdep::ThreadLockState::Get(lockdep::LockFlagsNone);
  ASSERT_NOT_NULL(state);

  // Verify that acquiring a lock works under the runtime and updates state.
  {
    lockdep::Guard<TestMutex> guard{&container.lock};
    EXPECT_EQ(lockdep::LockResult::Success, state->last_result());
  }
}

TEST(ZxLockdepRuntime, LoopDetectionTrigger) {
  // SystemTriggerLoopDetection should be safe to call.
  lockdep::SystemTriggerLoopDetection();
}

}  // namespace
