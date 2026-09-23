// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/testing/cpp/real_loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async/cpp/time.h>
#include <lib/async/dispatcher.h>
#include <lib/fpromise/bridge.h>
#include <lib/fpromise/promise.h>
#include <lib/sync/cpp/completion.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/time.h>

#include <gtest/gtest.h>

#include "src/lib/testing/predicates/status.h"

namespace {

class RealLoopFixtureTest : public loop_fixture::RealLoop, public testing::Test {};

TEST_F(RealLoopFixtureTest, Timeout) {
  zx::time posted_at = async::Now(dispatcher());
  zx::time called_at = zx::time::infinite_past();
  zx::duration timeout = zx::msec(100);
  async::PostDelayedTask(
      dispatcher(), [&called_at, dispatcher = dispatcher()] { called_at = async::Now(dispatcher); },
      timeout);
  // Run until `called_at` is updated.
  RunLoopWithTimeoutOrUntil([&called_at]() { return called_at > zx::time::infinite_past(); },
                            zx::sec(1));
  // The task should have been called only after `timeout` has elapsed.
  EXPECT_GE(called_at, posted_at + timeout);
}

TEST_F(RealLoopFixtureTest, NoTimeout) {
  // Check that the first run loop doesn't hit the timeout.
  QuitLoop();
  EXPECT_FALSE(RunLoopWithTimeout(zx::msec(10)));
  // But the second does.
  EXPECT_TRUE(RunLoopWithTimeout(zx::msec(10)));
}

TEST_F(RealLoopFixtureTest, RunPromiseResolved) {
  {
    auto res = RunPromise(fpromise::make_ok_promise("hello"));
    ASSERT_TRUE(res.is_ok());
    EXPECT_EQ(res.value(), "hello");
  }
  {
    auto res = RunPromise(fpromise::make_error_promise(1234));
    ASSERT_TRUE(!res.is_ok());
    EXPECT_EQ(res.error(), 1234);
  }
}

TEST_F(RealLoopFixtureTest, RunPromiseRequiresMultipleLoops) {
  // Make a promise whose closure needs to run 5 times to complete, and which
  // wakes itself up after each loop.
  fpromise::result<std::string> res = RunPromise(fpromise::make_promise(
      [count = 0](fpromise::context& ctx) mutable -> fpromise::result<std::string> {
        if (count < 5) {
          count++;
          // Tell the executor to call us again.
          ctx.suspend_task().resume_task();
          return fpromise::pending();
        }
        return fpromise::ok("finished");
      }));
  ASSERT_TRUE(res.is_ok());
  EXPECT_EQ(res.value(), "finished");
}

// Returns a promise that completes after |delay|.
fpromise::promise<> DelayedPromise(async_dispatcher_t* dispatcher, zx::duration delay) {
  fpromise::bridge<> bridge;
  async::PostDelayedTask(dispatcher, bridge.completer.bind(), delay);
  return bridge.consumer.promise();
}

TEST_F(RealLoopFixtureTest, RunPromiseDelayed) {
  fpromise::result<> res = RunPromise(DelayedPromise(dispatcher(), zx::msec(100)));
  EXPECT_TRUE(res.is_ok());
}

TEST_F(RealLoopFixtureTest, RunPromiseVoidValue) {
  auto res = RunPromise(fpromise::make_ok_promise());
  ASSERT_TRUE(res.is_ok());
  static_assert(std::is_same_v<decltype(res)::value_type, void>);
}

TEST_F(RealLoopFixtureTest, RunPromiseMoveOnlyValue) {
  auto res = RunPromise(fpromise::make_ok_promise(std::make_unique<int>(42)));
  ASSERT_TRUE(res.is_ok());
  ASSERT_NE(res.value().get(), nullptr);
  ASSERT_EQ(*res.value(), 42);
}

TEST_F(RealLoopFixtureTest, PerformBlockingWork) {
  // Check that both the async loop and the blocking work are running simultaneously.
  auto result = PerformBlockingWork([&] {
    libsync::Completion work_done;
    EXPECT_OK(async::PostTask(dispatcher(), [&] { work_done.Signal(); }));
    work_done.Wait();
    return 42;
  });

  static_assert(std::is_same_v<decltype(result), int>);
  ASSERT_EQ(result, 42);
}

TEST_F(RealLoopFixtureTest, PerformBlockingWorkVoidValue) {
  struct C {
    static void ReturnVoid() {}
  };
  static_assert(std::is_same_v<decltype(PerformBlockingWork(&C::ReturnVoid)), void>);
  PerformBlockingWork(&C::ReturnVoid);
}

TEST_F(RealLoopFixtureTest, PerformBlockingWorkMoveOnlyValue) {
  std::unique_ptr res = PerformBlockingWork([] { return std::make_unique<int>(42); });
  ASSERT_NE(res.get(), nullptr);
  ASSERT_EQ(*res, 42);
}

}  // namespace
