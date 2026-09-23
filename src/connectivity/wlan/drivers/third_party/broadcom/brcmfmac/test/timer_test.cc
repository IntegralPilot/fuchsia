/*
 * Copyright (c) 2020 The Fuchsia Authors
 *
 * Permission to use, copy, modify, and/or distribute this software for any
 * purpose with or without fee is hereby granted, provided that the above
 * copyright notice and this permission notice appear in all copies.
 *
 * THE SOFTWARE IS PROVIDED "AS IS" AND THE AUTHOR DISCLAIMS ALL WARRANTIES
 * WITH REGARD TO THIS SOFTWARE INCLUDING ALL IMPLIED WARRANTIES OF
 * MERCHANTABILITY AND FITNESS. IN NO EVENT SHALL THE AUTHOR BE LIABLE FOR ANY
 * SPECIAL, DIRECT, INDIRECT, OR CONSEQUENTIAL DAMAGES OR ANY DAMAGES
 * WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS, WHETHER IN AN ACTION
 * OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION, ARISING OUT OF OR IN
 * CONNECTION WITH THE USE OR PERFORMANCE OF THIS SOFTWARE.
 */

#include "src/connectivity/wlan/drivers/third_party/broadcom/brcmfmac/timer.h"

#include <lib/async/cpp/task.h>
#include <lib/driver/testing/cpp/driver_runtime.h>

#include <gtest/gtest.h>

namespace wlan::brcmfmac {

struct TimerTest : public testing::Test {
  [[nodiscard]] bool WaitForStop(Timer& timer) const {
    // Post the Stopped check on the same dispatcher to ensure that it runs after the timer has
    // completed all processing.
    libsync::Completion completed;
    bool stopped = false;
    async::PostTask(dispatcher_, [&] {
      // Get the value of Stopped inside the task to ensure that we don't return a modified value
      // as a result of a modification of a timer handler running after this task but before
      // WaitForStop returns.
      stopped = timer.Stopped();
      completed.Signal();
    });
    completed.Wait();
    return stopped;
  }

  fdf_testing::DriverRuntime runtime_;
  async_dispatcher_t* dispatcher_ = runtime_.StartBackgroundDispatcher()->async_dispatcher();
};

// This test creates a one-shot timer and checks if the handler fired
TEST_F(TimerTest, OneShot) {
  libsync::Completion handler_fired;
  Timer timer(dispatcher_, [&] { handler_fired.Signal(); }, Timer::Type::OneShot);
  EXPECT_EQ(timer.Start(ZX_MSEC(10)), ZX_OK);
  handler_fired.Wait();

  ASSERT_TRUE(WaitForStop(timer));
}

TEST_F(TimerTest, Periodic) {
  struct CountedTimer {
    CountedTimer(async_dispatcher_t* dispatcher, int expected_count)
        : timer_(
              dispatcher, [this] { OnTimer(); }, Timer::Type::Periodic),
          expected_count_(expected_count) {}

    void WaitUntilDone() { done_.Wait(); }
    Timer* operator->() { return &timer_; }
    Timer& operator*() { return timer_; }

   private:
    void OnTimer() {
      if (++current_count_ == expected_count_) {
        done_.Signal();
      }
    }
    Timer timer_;
    const int expected_count_;
    int current_count_ = 0;
    libsync::Completion done_;
  };

  CountedTimer timer1(dispatcher_, 4);
  CountedTimer timer2(dispatcher_, 2);

  EXPECT_EQ(timer1->Start(ZX_MSEC(2)), ZX_OK);
  EXPECT_EQ(timer2->Start(ZX_MSEC(5)), ZX_OK);

  // Wait for the first timer to complete
  timer1.WaitUntilDone();
  timer1->Stop();

  // and wait for the second timer to complete
  timer2.WaitUntilDone();
  timer2->Stop();

  // Check to make sure the timers stopped correctly.
  EXPECT_TRUE(WaitForStop(*timer1));
  EXPECT_TRUE(WaitForStop(*timer2));
}

// This test creates a one-shot timer and checks if the timer can be Started from within the handler
// itself. A second timer is created to ensure that calling Start on the first timer in the handler
// does not have any side-effects.
TEST_F(TimerTest, StartInHandler) {
  constexpr zx_duration_t kTimer1Delay = ZX_MSEC(10);
  constexpr zx_duration_t kTimer2Delay = ZX_MSEC(25);

  int timer_count = 0;
  libsync::Completion timer1_done;
  Timer timer1(
      dispatcher_,
      [&] {
        if (timer_count == 0) {
          EXPECT_EQ(timer1.Start(kTimer1Delay), ZX_OK);
        }
        if (++timer_count == 2) {
          timer1_done.Signal();
        }
      },
      Timer::Type::OneShot);
  libsync::Completion timer2_done;
  Timer timer2(dispatcher_, [&] { timer2_done.Signal(); }, Timer::Type::OneShot);

  EXPECT_EQ(timer1.Start(kTimer1Delay), ZX_OK);
  EXPECT_EQ(timer2.Start(kTimer2Delay), ZX_OK);

  // Wait for the first timer to complete
  timer1_done.Wait();
  EXPECT_TRUE(WaitForStop(timer1));

  // and wait for the second timer to complete
  timer2_done.Wait();
  EXPECT_TRUE(WaitForStop(timer2));
}

// This test creates a periodic timer and checks if Stop can be called from within the handler
// itself. A second timer is created to ensure calling Stop from within the first handler does not
// have any side-effects.
TEST_F(TimerTest, StopInHandler) {
  // Set the delay for the first timer to be very short, even if the next timer is about to fire
  // right away the Stop call should still prevent further timer triggers.
  constexpr zx_duration_t kTimer1Delay = ZX_USEC(1);
  constexpr zx_duration_t kTimer2Delay = ZX_MSEC(10);

  int timer_count = 0;
  libsync::Completion timer1_overstepped;
  libsync::Completion timer1_stop_called;
  Timer timer1(
      dispatcher_,
      [&] {
        if (timer_count == 0) {
          timer1.Stop();
          timer1_stop_called.Signal();
        }
        if (++timer_count > 1) {
          timer1_overstepped.Signal();
        }
      },
      Timer::Type::OneShot);
  libsync::Completion timer2_done;
  Timer timer2(dispatcher_, [&] { timer2_done.Signal(); }, Timer::Type::OneShot);

  EXPECT_EQ(timer1.Start(kTimer1Delay), ZX_OK);
  EXPECT_EQ(timer2.Start(kTimer2Delay), ZX_OK);

  // Wait for the first timer to complete, it should not have signaled the completion.
  timer1_stop_called.Wait();
  EXPECT_TRUE(WaitForStop(timer1));
  EXPECT_FALSE(timer1_overstepped.signaled());
  EXPECT_EQ(timer_count, 1);

  // and wait for the second timer to complete
  timer2_done.Wait();
  timer2.Stop();
  EXPECT_TRUE(WaitForStop(timer2));
}

TEST_F(TimerTest, CallbackInProgressDuringStop) {
  libsync::Completion started;
  libsync::Completion proceed;
  libsync::Completion done;
  Timer timer(
      dispatcher_,
      [&] {
        started.Signal();
        proceed.Wait();
        done.Signal();
      },
      Timer::Type::Periodic);

  EXPECT_EQ(timer.Start(ZX_USEC(5)), ZX_OK);
  // Wait until the timer handler has started processing
  started.Wait();
  // Post a delayed task to let the timer handler complete
  constexpr zx::duration delay = zx::msec(10);
  // Post a delayed task to allow the timer handler to proceed. This has to be done on another
  // dispatcher or it will block on the timer handler.
  async_dispatcher_t* task_dispatcher = runtime_.StartBackgroundDispatcher()->async_dispatcher();
  // Keep track of how long the posting and stopping takes
  const zx::time start(zx_clock_get_monotonic());
  async::PostDelayedTask(task_dispatcher, [&] { proceed.Signal(); }, delay);
  timer.Stop();
  const zx::duration elapsed = zx::time(zx_clock_get_monotonic()) - start;
  // The posting and Stop call must have waited for at least as long as the delay. We have to
  // include the posting of the task. It is technically possible that the posted task completes
  // before we call Stop, even if it is delayed. This means the minimum time taken for Stop itself
  // is near zero. But the time taken for both operations together must equal or exceed the delay.
  EXPECT_GE(elapsed, delay);
  // done should already be signaled since Stop guarantees that the timer callback has finished.
  EXPECT_TRUE(done.signaled());
}

TEST_F(TimerTest, IntervalIsUsed) {
  libsync::Completion done;
  Timer timer(dispatcher_, [&] { done.Signal(); }, Timer::Type::OneShot);

  constexpr zx::duration delay = zx::msec(100);

  const zx::time start(zx_clock_get_monotonic());
  EXPECT_EQ(timer.Start(delay.get()), ZX_OK);
  done.Wait();
  const zx::duration elapsed = zx::time(zx_clock_get_monotonic()) - start;
  EXPECT_GE(elapsed.get(), delay.get());
}

TEST_F(TimerTest, DoubleStop) {
  Timer timer(dispatcher_, [] {}, Timer::Type::Periodic);

  EXPECT_EQ(timer.Start(ZX_USEC(10)), ZX_OK);

  // Calling stop twice without waiting should work.
  timer.Stop();
  timer.Stop();
}

TEST_F(TimerTest, SpamStartStop) {
  // This test verifies that Start and Stop can be called from multiple threads at the same time
  // without crashing or triggering any UBSAN or ASAN checks. Any flakes here should be considered
  // problematic as they may indicate a threading issue.
  constexpr size_t kNumThreads = 10;
  constexpr size_t kNumIterations = 1'000;
  Timer timer(dispatcher_, [] {}, Timer::Type::Periodic);

  std::vector<std::thread> threads;
  for (size_t i = 0; i < kNumThreads; ++i) {
    threads.emplace_back([i, &timer] {
      for (size_t iteration = 0; iteration < kNumIterations; ++iteration) {
        if (i % 2) {
          EXPECT_EQ(timer.Start(ZX_USEC(i)), ZX_OK);
        } else {
          timer.Stop();
        }
      }
    });
  }

  for (auto& thread : threads) {
    thread.join();
  }
}

TEST_F(TimerTest, UsesDispatcherTime) {
  // Verify that the timer uses the dispatcher's idea of time and not the system's idea of time or
  // anything else.

  struct FakeDispatcher : public async_dispatcher_t {
    FakeDispatcher() : async_dispatcher_t{&ops_} {}

    std::atomic<zx_time_t> current_time = 0;
    std::atomic<zx_time_t> posted_time = 0;
    libsync::Completion posted;

    async_ops_t ops_{
        .version = ASYNC_OPS_V1,
        .v1{
            .now = [](async_dispatcher_t* dispatcher) -> zx_time_t {
              return static_cast<FakeDispatcher*>(dispatcher)->current_time.load();
            },
            .post_task = [](async_dispatcher_t* dispatcher, async_task_t* task) -> zx_status_t {
              auto fake_dispatcher = static_cast<FakeDispatcher*>(dispatcher);
              fake_dispatcher->posted_time = task->deadline;
              fake_dispatcher->posted.Signal();
              // No need to actually run the task here, it doesn't matter, we just need to verify
              // the deadline it was posted with.
              return ZX_OK;
            },
            .cancel_task = [](async_dispatcher_t* dispatcher, async_task_t* task) -> zx_status_t {
              return ZX_OK;
            },
        }};
  };

  FakeDispatcher dispatcher;

  constexpr zx::duration first_delay = zx::msec(5);
  Timer timer(&dispatcher, [] {}, Timer::Type::OneShot);
  EXPECT_EQ(timer.Start(first_delay.get()), ZX_OK);
  dispatcher.posted.Wait();
  EXPECT_EQ(dispatcher.posted_time.load(), dispatcher.current_time.load() + first_delay.get());

  dispatcher.posted.Reset();

  // Travel forward in time a little bit.
  dispatcher.current_time = ZX_HOUR(1);
  constexpr zx::duration second_delay = zx::sec(15);
  EXPECT_EQ(timer.Start(second_delay.get()), ZX_OK);
  dispatcher.posted.Wait();
  EXPECT_EQ(dispatcher.posted_time.load(), dispatcher.current_time.load() + second_delay.get());
}

}  // namespace wlan::brcmfmac
