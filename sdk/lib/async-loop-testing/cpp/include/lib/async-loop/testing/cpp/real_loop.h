// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_LOOP_TESTING_CPP_REAL_LOOP_H_
#define LIB_ASYNC_LOOP_TESTING_CPP_REAL_LOOP_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/async/cpp/executor.h>
#include <lib/fit/defer.h>
#include <lib/fit/function.h>
#include <lib/fpromise/bridge.h>
#include <lib/zx/time.h>

#include <thread>

namespace loop_fixture {

// A class which sets up a |async::Loop| and offers a variety of ways to run
// the loop until a certain condition is satisfied.
//
// Example:
//
//     // Creates a fixture that creates a message loop on this thread.
//     class FooTest : public ::testing::Test, public loop_fixture::RealLoop {};
//
//     TEST_F(FooTest, TestCase) {
//       // Schedule asynchronous operations, and quit the loop when done.
//       DoSomeAsyncWork([&] { QuitLoop(); });
//
//       // Run the loop until it is quit.
//       RunLoop();
//
//       // Check results of those asynchronous operations here.
//     }
//
class RealLoop {
 protected:
  RealLoop();
  ~RealLoop();

  // Returns the loop's asynchronous dispatch interface.
  async_dispatcher_t* dispatcher();

  // Runs the loop until it is exited.
  void RunLoop();

  // Runs the loop for at most |timeout|. Returns |true| if the timeout has been
  // reached.
  bool RunLoopWithTimeout(zx::duration timeout = zx::sec(1));

  // Runs the loop until the condition returns true.
  //
  // |step| specifies the interval at which this method should wake up to poll
  // |condition|. If |step| is |zx::duration::infinite()|, no polling timer is
  // set. Instead, the condition is checked initially and after anything happens
  // on the loop (e.g. a task executes). This is useful when the caller knows
  // that |condition| will be made true by a task running on the loop. This will
  // generally be the case unless |condition| is made true on a different
  // thread.
  void RunLoopUntil(fit::function<bool()> condition, zx::duration step = zx::msec(10));

  // Runs the loop until the condition returns true or the timeout is reached.
  // Returns |true| if the condition was met, and |false| if the timeout was
  // reached.
  //
  // |step| specifies the interval at which this method should wake up to poll
  // |condition|. If |step| is |zx::duration::infinite()|, no polling timer is
  // set. Instead, the condition is checked initially and after anything happens
  // on the loop (e.g. a task executes). This is useful when the caller knows
  // that |condition| will be made true by a task running on the loop. This will
  // generally be the case unless |condition| is made true on a different
  // thread.
  bool RunLoopWithTimeoutOrUntil(fit::function<bool()> condition, zx::duration timeout = zx::sec(1),
                                 zx::duration step = zx::msec(10));

  // Runs the message loop until idle.
  void RunLoopUntilIdle();

  // Runs the loop until the given |fpromise::promise| completes, and returns the
  // |fpromise::result| it produced.
  //
  // If the |fpromise::promise| never completes, this method will run forever.
  template <typename PromiseType>
  typename PromiseType::result_type RunPromise(PromiseType promise);

  // Runs the message loop while spawning a new thread to perform |work|, and
  // returns the result of calling |work|. Runs the loop until |work| returns.
  //
  // |Callable| should take zero arguments.
  template <typename Callable>
  auto PerformBlockingWork(Callable&& work) {
    using R = std::invoke_result_t<Callable>;
    if constexpr (std::is_same_v<R, void>) {
      // Use an |std::monostate| such that |PerformBlockingWorkImpl| does not
      // have to contend with void values which are tricky to work with.
      PerformBlockingWorkImpl<std::monostate>([work = std::forward<Callable>(work)]() mutable {
        work();
        return std::monostate{};
      });
      return;
    } else {
      return PerformBlockingWorkImpl<R>(std::forward<Callable>(work));
    }
  }

  // Quits the loop.
  void QuitLoop();

  // Creates a closure that quits the test message loop when executed.
  fit::closure QuitLoopClosure();

  async::Loop& loop() { return loop_; }

 private:
  template <typename Result, typename Callable>
  Result PerformBlockingWorkImpl(Callable&& work);

  // The message loop for the test.
  async::Loop loop_;

  RealLoop(const RealLoop&) = delete;
  RealLoop& operator=(const RealLoop&) = delete;
};

// Internal template implementation details
// ========================================

template <typename PromiseType>
typename PromiseType::result_type RealLoop::RunPromise(PromiseType promise) {
  async::Executor e(dispatcher());
  std::optional<typename PromiseType::result_type> res;
  e.schedule_task(
      promise.then([&res](typename PromiseType::result_type& v) { res = std::move(v); }));

  // We set |step| to infinity, as the automatic-wake-up timer shouldn't be
  // necessary. A well-behaved promise must always wake up the executor when
  // there's more work to be done.
  RunLoopUntil([&res]() { return res.has_value(); }, zx::duration::infinite());
  return std::move(res.value());
}

template <typename Result, typename Callable>
Result RealLoop::PerformBlockingWorkImpl(Callable&& work) {
  fpromise::bridge<Result> bridge;
  std::thread t([completer = std::move(bridge.completer),
                 work = std::forward<Callable>(work)]() mutable { completer.complete_ok(work()); });
  auto defer = fit::defer([&] { t.join(); });
  return RunPromise(bridge.consumer.promise()).take_value();
}

}  // namespace loop_fixture

#endif  // LIB_ASYNC_LOOP_TESTING_CPP_REAL_LOOP_H_
