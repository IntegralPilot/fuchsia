// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/utils/dispatcher_holder.h"

#include <lib/async-loop/default.h>
#include <lib/async/cpp/task.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/real_loop_fixture.h"

namespace utils::test {

// No special behavior, we just want to be able to use RunLoopUntilIdle() etc.
using DispatcherHolderTest = ::gtest::RealLoopFixture;

TEST_F(DispatcherHolderTest, DestroyLoop) {
  std::atomic_bool loop_was_destroyed = false;

  auto holder = std::make_unique<LoopDispatcherHolder>(
      &kAsyncLoopConfigNoAttachToCurrentThread,
      [&, test_dispatcher = dispatcher()](std::unique_ptr<async::Loop> loop_to_destroy) {
        async::PostTask(test_dispatcher,
                        [&, loop_to_destroy = std::move(loop_to_destroy)]() mutable {
                          loop_to_destroy.reset();
                          loop_was_destroyed = true;
                        });
      });
  holder->loop().StartThread();

  holder.reset();
  RunLoopUntilIdle();
  EXPECT_TRUE(loop_was_destroyed);
}

// Demonstrates that a loop *holder* can be destroyed on its own thread, by posting a task
// to destroy the *loop* on a different dispatcher.  This must be done because if you try to destroy
// the loop on its own dispatcher's thread, it will deadlock as the thread tries to join itself.
TEST_F(DispatcherHolderTest, DestroyLoopHolderOnItsOwnThread) {
  std::atomic_bool loop_was_destroyed = false;
  std::atomic_bool loop_holder_was_destroyed = false;

  auto holder = std::make_unique<LoopDispatcherHolder>(
      &kAsyncLoopConfigNoAttachToCurrentThread,
      [&, test_dispatcher = dispatcher()](std::unique_ptr<async::Loop> loop_to_destroy) {
        async::PostTask(test_dispatcher,
                        [&, loop_to_destroy = std::move(loop_to_destroy)]() mutable {
                          loop_to_destroy.reset();
                          loop_was_destroyed = true;
                        });
      });
  holder->loop().StartThread();

  {
    auto holder_dispatcher = holder->dispatcher();
    async::PostTask(holder_dispatcher, [&, holder_to_destroy = std::move(holder)]() mutable {
      // Destroy the holder on its own thread.
      holder_to_destroy.reset();
      loop_holder_was_destroyed = true;
    });
  }

  RunLoopUntil([&]() {
    bool was_destroyed = loop_holder_was_destroyed.load();

    if (!was_destroyed) {
      // If the loop holder wasn't destroyed, then the loop should not have been either.
      EXPECT_FALSE(loop_was_destroyed);
    }

    return was_destroyed;
  });

  // Now the at the loop holder was destroyed, we expect that a task was posted onto this loop;
  // once it has run the loop will be destroyed.
  RunLoopUntilIdle();
  EXPECT_TRUE(loop_was_destroyed);
}

}  // namespace utils::test
