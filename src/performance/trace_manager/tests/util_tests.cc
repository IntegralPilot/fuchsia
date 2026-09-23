// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async/cpp/executor.h>

#include <gtest/gtest.h>

#include "src/lib/testing/loop_fixture/test_loop_fixture.h"
#include "src/performance/trace_manager/util.h"

class UtilsTest : public gtest::TestLoopFixture {
 public:
  UtilsTest() : executor_(dispatcher()) {}

  async::Executor executor_;
};

TEST_F(UtilsTest, PromiseTimeout) {
  // Check that an expired promise with a valid result returns the valid result instead of a timeout
  // error.
  auto check_ok = fpromise::make_promise([this](fpromise::context& context) {
    auto already_done_fut = fpromise::make_future(tracing::with_timeout(
        executor_, fpromise::make_ok_promise(1), zx::duration::infinite_past()));

    ASSERT_TRUE(already_done_fut(context));
    EXPECT_EQ(already_done_fut.value().value(), 1);
  });
  executor_.schedule_task(std::move(check_ok));

  // Check that an expired promise with an error result returns the error result instead of a
  // timeout error.
  auto check_err = fpromise::make_promise([this](fpromise::context& context) {
    auto already_done_fut = fpromise::make_future(tracing::with_timeout(
        executor_, fpromise::make_error_promise(2), zx::duration::infinite_past()));

    ASSERT_TRUE(already_done_fut(context));
    EXPECT_EQ(already_done_fut.value().error(), 2);
  });
  executor_.schedule_task(std::move(check_err));

  // Check that a pending promise that's not yet expired does indeed return pending.
  auto check_pending = fpromise::make_promise([this](fpromise::context& context) {
    auto always_pending = fpromise::make_promise([]() { return fpromise::pending(); });
    auto pending_fut = fpromise::make_future(
        tracing::with_timeout(executor_, std::move(always_pending), zx::sec(5)));
    EXPECT_FALSE(pending_fut(context));
  });

  executor_.schedule_task(std::move(check_pending));

  // Check that if the inner promise is still pending and the timer expired, we get an expired
  // error.
  auto check_expired = fpromise::make_promise([this](fpromise::context& context) {
    auto always_pending = fpromise::make_promise([]() { return fpromise::pending(); });
    auto pending_fut = fpromise::make_future(
        tracing::with_timeout(executor_, std::move(always_pending), zx::sec(-5)));

    ASSERT_TRUE(pending_fut(context));
    EXPECT_TRUE(pending_fut.is_error());
  });

  executor_.schedule_task(std::move(check_expired));
}

TEST_F(UtilsTest, IncrementalPromiseProgress) {
  // Check that if our promise returns pending and suspends itself it gets rescheduled when the
  // inner task finishes.
  auto not_yet_done =
      executor_.MakeDelayedPromise(zx::sec(3)).and_then(fpromise::make_ok_promise(5));

  auto pending_fut =
      fpromise::make_future(tracing::with_timeout(executor_, std::move(not_yet_done), zx::sec(5)));

  auto check_pending_then_finished = fpromise::make_promise(
      [&pending_fut](fpromise::context& context)
          -> fpromise::result<fpromise::result<int>, tracing::ResultTimedOut> {
        if (!pending_fut(context)) {
          return fpromise::pending();
        }
        return pending_fut.result();
      });

  executor_.schedule_task(std::move(check_pending_then_finished));

  EXPECT_TRUE(RunLoopFor(zx::sec(1)));
  ASSERT_TRUE(pending_fut.is_pending());

  EXPECT_FALSE(RunLoopFor(zx::sec(1)));
  ASSERT_TRUE(pending_fut.is_pending());

  ASSERT_TRUE(RunLoopFor(zx::sec(1)));
  ASSERT_TRUE(pending_fut.is_ok());
  EXPECT_TRUE(pending_fut.value().is_ok());
  EXPECT_EQ(pending_fut.value().value(), 5);
  // We should see the timeout expire
  EXPECT_TRUE(RunLoopFor(zx::sec(3)));

  // Check that if our promise returns pending and suspends itself it gets rescheduled when the
  // timer errors out.
  auto wont_finish_in_time =
      executor_.MakeDelayedPromise(zx::sec(5)).and_then(fpromise::make_ok_promise(5));

  auto wont_finish_pending_fut = fpromise::make_future(
      tracing::with_timeout(executor_, std::move(wont_finish_in_time), zx::sec(3)));

  auto check_pending_then_timeout = fpromise::make_promise(
      [&wont_finish_pending_fut](fpromise::context& context)
          -> fpromise::result<fpromise::result<int>, tracing::ResultTimedOut> {
        if (!wont_finish_pending_fut(context)) {
          return fpromise::pending();
        }
        return wont_finish_pending_fut.result();
      });

  executor_.schedule_task(std::move(check_pending_then_timeout));

  EXPECT_TRUE(RunLoopFor(zx::sec(1)));
  ASSERT_TRUE(wont_finish_pending_fut.is_pending());

  EXPECT_FALSE(RunLoopFor(zx::sec(1)));
  ASSERT_TRUE(wont_finish_pending_fut.is_pending());

  ASSERT_TRUE(RunLoopFor(zx::sec(1)));
  EXPECT_TRUE(wont_finish_pending_fut.is_error());
}

TEST_F(UtilsTest, JoinedPromisesTimeout) {
  // Check that a promise that suspends and resumes multiple times we don't get multiple timeouts.
  auto delay1 = executor_.MakeDelayedPromise(zx::sec(1)).and_then([]() { return fpromise::ok(1); });
  auto delay2 = executor_.MakeDelayedPromise(zx::sec(2)).and_then([]() { return fpromise::ok(2); });
  auto delay3 = executor_.MakeDelayedPromise(zx::sec(3)).and_then([]() { return fpromise::ok(3); });

  auto all_delays =
      fpromise::join_promises(std::move(delay1), std::move(delay2), std::move(delay3));
  auto with_timeout = tracing::with_timeout(executor_, std::move(all_delays), zx::sec(5));

  auto pending_fut = fpromise::make_future(std::move(with_timeout));

  auto check_pending_then_finished = fpromise::make_promise(
      [&pending_fut](fpromise::context& context)
          -> fpromise::result<
              fpromise::result<
                  std::tuple<fpromise::result<int>, fpromise::result<int>, fpromise::result<int>>>,
              tracing::ResultTimedOut> {
        if (!pending_fut(context)) {
          return fpromise::pending();
        }
        return pending_fut.result();
      });

  executor_.schedule_task(std::move(check_pending_then_finished));

  EXPECT_TRUE(RunLoopFor(zx::sec(1)));
  ASSERT_TRUE(pending_fut.is_pending());

  EXPECT_TRUE(RunLoopFor(zx::sec(1)));
  ASSERT_TRUE(pending_fut.is_pending());

  ASSERT_TRUE(RunLoopFor(zx::sec(1)));
  ASSERT_TRUE(pending_fut.is_ok());
  EXPECT_TRUE(pending_fut.value().is_ok());

  EXPECT_EQ(std::get<0>(pending_fut.value().value()).value(), 1);
  EXPECT_EQ(std::get<1>(pending_fut.value().value()).value(), 2);
  EXPECT_EQ(std::get<2>(pending_fut.value().value()).value(), 3);

  // We should get one additional task from the timeout completing.
  EXPECT_TRUE(RunLoopFor(zx::sec(2)));

  // But no additional tasks after that
  EXPECT_FALSE(RunLoopFor(zx::sec(1000000)));
}
