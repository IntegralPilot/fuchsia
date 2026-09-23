// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <inttypes.h>
#include <lib/fit/defer.h>
#include <lib/zx/clock.h>
#include <lib/zx/event.h>
#include <lib/zx/process.h>
#include <lib/zx/suspend_token.h>
#include <lib/zx/thread.h>
#include <lib/zx/vmar.h>
#include <lib/zx/vmo.h>
#include <sched.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <array>
#include <atomic>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <ctime>
#include <iterator>
#include <limits>
#include <thread>

#include <fbl/algorithm.h>
#include <zxtest/zxtest.h>

namespace futex {
namespace {
constexpr zx::duration kDefaultPollInterval = zx::usec(100);
constexpr zx::duration kDefaultReportInterval = zx::sec(10);

constexpr uint32_t kThreadWakeAllCount = std::numeric_limits<uint32_t>::max();
constexpr uint32_t kThreadRequeueAllCount = std::numeric_limits<uint32_t>::max();
constexpr char kThreadName[] = "wakeup-test-thread";

// Poll until the user provided ShouldStopCallable |should_stop| tells us to stop by
// returning true.
template <typename ShouldStopCallable, typename ReportStatusCallable>
void WaitFor(const ShouldStopCallable& should_stop, const ReportStatusCallable& report_status,
             zx::duration poll_interval = kDefaultPollInterval,
             zx::duration report_interval = kDefaultReportInterval) {
  static_assert(std::is_same_v<decltype(should_stop()), bool>, "should_stop() must return a bool!");
  zx::time report_deadline = zx::deadline_after(report_interval);

  while (true) {
    const zx::time now = zx::clock::get_monotonic();

    if (should_stop()) {
      return;
    }

    if (now >= report_deadline) {
      report_status();
      report_deadline += report_interval;
    }

    zx::nanosleep(zx::deadline_after(poll_interval));
  }
}

void GetThreadState(const zx::thread& thread, zx_thread_state_t* out_state) {
  zx_info_thread_t info;

  ASSERT_NOT_NULL(out_state);
  ASSERT_TRUE(thread.is_valid());
  ASSERT_OK(thread.get_info(ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr));
  *out_state = info.state;
}

void WaitForKernelState(const zx::thread& thread, zx_thread_state_t target_state,
                        zx::duration timeout = zx::duration::infinite()) {
  zx_thread_state_t state = 0;

  WaitFor(
      [&]() {
        GetThreadState(thread, &state);
        // Stop if we have hit the state we want, or we have an error attempting
        // to fetch our kernel thread state.
        return (state == target_state);
      },
      [&]() {
        printf("still waiting for thread to achieve state (%u).  Last observed state (%u)\n", state,
               target_state);
      });

  // Verify that any of the helpers methods called has no assertion failures.
  ASSERT_NO_FATAL_FAILURE();
  ASSERT_EQ(state, target_state);
}

class TestThread {
 public:
  TestThread() = default;
  TestThread(const TestThread&) = delete;
  TestThread(TestThread&&) = delete;
  TestThread& operator=(const TestThread&) = delete;
  TestThread& operator=(TestThread&&) = delete;
  ~TestThread() { Shutdown(); }

  void Start(zx_futex_t* futex, zx::duration timeout = zx::duration::infinite(),
             zx_handle_t owner = ZX_HANDLE_INVALID) {
    ASSERT_FALSE(thread_handle_.is_valid(), "Attempting to start already started thread.");

    futex_.store(futex);
    owner_.store(owner);
    timeout_ = timeout;
    wait_result_.store(ZX_ERR_INTERNAL);

    // If our thread fails to stay blocked in the futex, make an attempt to
    // record what the result of its futex_wait operation was in order to assist
    // with debugging.  In theory, we should only ever see TIMED_OUT, and only
    // ever when running a test which has legitimate flake behavior.
    auto cleanup = fit::defer([this]() {
      if (state() == State::kWaitReturned) {
        EXPECT_TRUE(false, "Wait status was %d\n", static_cast<uint32_t>(wait_result_.load()));
      }
    });

    ASSERT_EQ(
        thrd_create_with_name(
            &thread_,
            [](void* thread_args) { return reinterpret_cast<TestThread*>(thread_args)->Run(); },
            this, kThreadName),
        thrd_success, "Thread creation failed.");

    // Make a copy of our thread's handle so that we have something to query
    // re: the thread's status, even if the thread exits out from under us
    // (which will invalidate the handled returned by thrd_get_zx_handle
    ASSERT_OK(zx::unowned_thread(thrd_get_zx_handle(thread_))
                  ->duplicate(ZX_RIGHT_SAME_RIGHTS, &thread_handle_));

    State last_state;
    WaitFor(
        [&]() { return (last_state = state()) != State::kWaitingToStart; },
        [&]() { printf("waiting for thread to start (%u)\n", static_cast<uint32_t>(last_state)); });

    // Note that this could fail if futex_wait() gets a spurious wakeup.
    EXPECT_EQ(state(), State::kAboutToWait, "Wrong thread state.");

    // We should only do this after state_ is State::kAboutToWait,
    // otherwise it could return when the thread has temporarily
    // blocked on a libc-internal futex.
    ASSERT_NO_FATAL_FAILURE(WaitForKernelState(thread_handle_, ZX_THREAD_STATE_BLOCKED_FUTEX));

    // This could also fail if futex_wait() gets a spurious wakeup.
    EXPECT_EQ(state(), State::kAboutToWait, "Wrong thread state.");

    if (CURRENT_TEST_HAS_FAILURES() == false) {
      cleanup.cancel();
    }
  }

  void Shutdown() {
    if (thread_handle_.is_valid()) {
      zx_status_t res =
          thread_handle_.wait_one(ZX_THREAD_TERMINATED, zx::deadline_after(zx::sec(10)), nullptr);
      ASSERT_OK(res, "Thread did not terminate in a timely fashion!");
      EXPECT_EQ(thrd_join(thread_, nullptr), thrd_success, "thrd_join failed");
      thread_handle_.reset();
    }
  }

  void WaitUntilWoken() const {
    State last_state;
    WaitFor(
        [&]() { return (last_state = state()) == State::kWaitReturned; },
        [&]() { printf("waiting for thread to wake (%u)\n", static_cast<uint32_t>(last_state)); });

    ASSERT_EQ(state(), State::kWaitReturned, "Thread in wrong state");
  }

  void CheckIsBlockedOnFutex() const {
    zx_thread_state_t state;
    ASSERT_NO_FATAL_FAILURE(GetThreadState(thread_handle_, &state));
    ASSERT_EQ(state, ZX_THREAD_STATE_BLOCKED_FUTEX);
  }

  const zx::thread& thread() const { return thread_handle_; }
  bool HasWaitReturned() const { return state() == State::kWaitReturned; }
  zx_status_t wait_result() const { return wait_result_.load(); }

  zx::result<zx_koid_t> GetKoid() const {
    zx_info_handle_basic_t info;
    if (zx_status_t status =
            thread_handle_.get_info(ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr, nullptr);
        status != ZX_OK) {
      return zx::error(status);
    }
    return zx::ok(info.koid);
  }

 private:
  enum class State {
    kWaitingToStart = 100,
    kAboutToWait = 200,
    kWaitReturned = 300,
  };

  int Run() {
    state_.store(State::kAboutToWait);

    zx::time deadline = zx::deadline_after(timeout_);
    wait_result_.store(zx_futex_wait(futex(), *futex(), owner_.load(), deadline.get()));
    state_.store(State::kWaitReturned);
    return 0;
  }

  State state() const { return state_.load(); }
  zx_futex_t* futex() const { return futex_.load(); }

  std::atomic<zx_status_t> wait_result_{ZX_ERR_INTERNAL};
  std::atomic<zx_futex_t*> futex_{nullptr};
  std::atomic<zx_handle_t> owner_{ZX_HANDLE_INVALID};
  std::atomic<State> state_{State::kWaitingToStart};
  zx::duration timeout_ = zx::duration::infinite();
  zx::thread thread_handle_;
  thrd_t thread_;
};

void AssertWokeThreadCount(const TestThread threads[], uint32_t total_thread_count,
                           uint32_t target_woke_count) {
  ASSERT_LE(target_woke_count, total_thread_count);

  auto CountWoken = [&threads, total_thread_count]() -> uint32_t {
    uint32_t ret = 0;
    for (uint32_t i = 0; i < total_thread_count; ++i) {
      if (threads[i].HasWaitReturned()) {
        ++ret;
      }
    }
    return ret;
  };

  // Wait forever until we achieve the target count.  If threads are not
  // waking up as they should, the test framework should eventually kill
  // us.
  uint32_t woken;
  do {
    woken = CountWoken();
  } while (woken < target_woke_count);

  ASSERT_EQ(CountWoken(), target_woke_count);

  // Wait an arbitrary amount of time to be sure that no one else wakes
  // up.
  //
  // TODO(johngro) : It would be really nice if we didn't have to have an
  // arbitrary wait here.  Unfortunately, I'm not sure that there is any
  // amount of time that we can wait and prove that a thread might not
  // spuriously wake up in the future.
  zx::nanosleep(zx::deadline_after(zx::msec(300)));
  ASSERT_EQ(CountWoken(), target_woke_count);
}

TEST(FutexTest, WaitValueMismatch) {
  int32_t futex_value = 123;
  ASSERT_EQ(zx_futex_wait(&futex_value, futex_value + 1, ZX_HANDLE_INVALID, ZX_TIME_INFINITE),
            ZX_ERR_BAD_STATE, "Futex wait should have reurned bad state");
}

TEST(FutexTest, WaitTimeout) {
  int32_t futex_value = 123;

  ASSERT_EQ(zx_futex_wait(&futex_value, futex_value, ZX_HANDLE_INVALID, 0), ZX_ERR_TIMED_OUT,
            "Futex wait should have reurned timeout");
}

// This test checks that the timeout in futex_wait() is respected
TEST(FutexTest, WaitTimeoutElapsed) {
  int32_t futex_value = 0;
  constexpr zx::duration kRelativeDeadline = zx::msec(100);

  for (int i = 0; i < 5; ++i) {
    zx::time deadline = zx::deadline_after(kRelativeDeadline);

    ASSERT_EQ(zx_futex_wait(&futex_value, 0, ZX_HANDLE_INVALID, deadline.get()), ZX_ERR_TIMED_OUT,
              "wait should time out");
    EXPECT_GE(zx::clock::get_monotonic().get(), deadline.get(), "wait returned early");
  }
}

TEST(FutexTest, WaitBadAddress) {
  // Check that the wait address is checked for validity.
  ASSERT_EQ(zx_futex_wait(nullptr, 123, ZX_HANDLE_INVALID, ZX_TIME_INFINITE), ZX_ERR_INVALID_ARGS,
            "Futex wait should have reurned invalid_arg");
}

// Test that we can wake up a single thread.
TEST(FutexTest, Wakeup) {
  zx_futex_t futex{1};
  TestThread thread;

  ASSERT_NO_FATAL_FAILURE(thread.Start(&futex));

  // Clean up on exit.
  auto cleanup = fit::defer([&thread, &futex]() {
    EXPECT_OK(zx_futex_wake(&futex, kThreadWakeAllCount));
    thread.Shutdown();
  });

  ASSERT_OK(zx_futex_wake(&futex, kThreadWakeAllCount));
  ASSERT_NO_FATAL_FAILURE(thread.WaitUntilWoken());
  ASSERT_OK(thread.wait_result());
}

// Test that we can wake up multiple threads, and that futex_wake() heeds
// the wakeup limit.
TEST(FutexTest, WakeupLimit) {
  constexpr int kWakeCount = 2;
  zx_futex_t futex{1};
  TestThread threads[4];

  // If something goes wrong and we bail out early, do our best to shut down as cleanly as we
  auto cleanup = fit::defer([&]() {
    zx_futex_wake(&futex, kThreadWakeAllCount);
    for (auto& t : threads) {
      t.Shutdown();
    }
  });

  for (auto& t : threads) {
    ASSERT_NO_FATAL_FAILURE(t.Start(&futex));
  }

  ASSERT_OK(zx_futex_wake(&futex, kWakeCount));

  // Test that exactly |kWakeCount| threads wake up from the queue.  We do not know
  // which threads are going to wake up, just that two threads are going to
  // wake up.
  ASSERT_NO_FATAL_FAILURE(AssertWokeThreadCount(threads, std::size(threads), 2));

  // Clean up: Wake the remaining threads so that they can exit.
  ASSERT_OK(zx_futex_wake(&futex, kThreadWakeAllCount));
  ASSERT_NO_FATAL_FAILURE(AssertWokeThreadCount(threads, std::size(threads), std::size(threads)));

  for (auto& t : threads) {
    ASSERT_OK(t.wait_result());
    ASSERT_NO_FATAL_FAILURE(t.Shutdown());
  }

  cleanup.cancel();
}

// Check that futex_wait() and futex_wake() heed their address arguments
// properly.  A futex_wait() call on one address should not be woken by a
// futex_wake() call on another address.
TEST(FutexTest, WakeupAddress) {
  std::array<zx_futex_t, 3> futexes = {1, 1, 1};
  TestThread threads[2];

  // If something goes wrong and we bail out early, do our best to shut down as cleanly as we can.
  auto cleanup = fit::defer([&]() {
    zx_futex_wake(&futexes[0], kThreadWakeAllCount);
    zx_futex_wake(&futexes[1], kThreadWakeAllCount);
    for (auto& t : threads) {
      t.Shutdown();
    }
  });

  ASSERT_NO_FATAL_FAILURE(threads[0].Start(&futexes[0]));
  ASSERT_NO_FATAL_FAILURE(threads[1].Start(&futexes[1]));

  ASSERT_OK(zx_futex_wake(&futexes[2], kThreadWakeAllCount));
  ASSERT_NO_FATAL_FAILURE(threads[0].CheckIsBlockedOnFutex());
  ASSERT_NO_FATAL_FAILURE(threads[1].CheckIsBlockedOnFutex());

  ASSERT_OK(zx_futex_wake(&futexes[0], kThreadWakeAllCount));
  ASSERT_NO_FATAL_FAILURE(threads[0].WaitUntilWoken());
  ASSERT_NO_FATAL_FAILURE(threads[1].CheckIsBlockedOnFutex());

  // Clean up: Wake the remaining thread so that it can exit.
  ASSERT_OK(zx_futex_wake(&futexes[1], kThreadWakeAllCount));
  ASSERT_NO_FATAL_FAILURE(threads[1].WaitUntilWoken());

  for (auto& t : threads) {
    ASSERT_OK(t.wait_result());
    ASSERT_NO_FATAL_FAILURE(t.Shutdown());
  }

  cleanup.cancel();
}

TEST(FutexTest, RequeueValueMismatch) {
  zx_futex_t futex_value1 = 100;
  zx_futex_t futex_value2 = 200;

  ASSERT_EQ(
      zx_futex_requeue(&futex_value1, 1, futex_value1 + 1, &futex_value2, 1, ZX_HANDLE_INVALID),
      ZX_ERR_BAD_STATE, "requeue should have returned bad state");
}

TEST(FutexTest, RequeueSameAddr) {
  zx_futex_t futex_value = 100;

  ASSERT_EQ(zx_futex_requeue(&futex_value, 1, futex_value, &futex_value, 1, ZX_HANDLE_INVALID),
            ZX_ERR_INVALID_ARGS, "requeue should have returned invalid args");
}

// Test that futex_requeue() can wake up some threads and requeue others.
TEST(FutexTest, Requeue) {
  zx_futex_t futex_value1(100);
  zx_futex_t futex_value2(200);
  TestThread threads[6];

  // If something goes wrong and we bail out early, do our best to shut down as cleanly as we
  auto cleanup = fit::defer([&]() {
    zx_futex_wake(&futex_value1, kThreadWakeAllCount);
    zx_futex_wake(&futex_value2, kThreadWakeAllCount);
    for (auto& t : threads) {
      t.Shutdown();
    }
  });

  for (auto& t : threads) {
    ASSERT_NO_FATAL_FAILURE(t.Start(&futex_value1));
  }

  ASSERT_OK(zx_futex_requeue(&futex_value1, 3, 100, &futex_value2, 2, ZX_HANDLE_INVALID));

  // 3 of the threads should have been woken.
  ASSERT_NO_FATAL_FAILURE(AssertWokeThreadCount(threads, std::size(threads), 3));

  // Since 2 of the threads should have been requeued, waking all the
  // threads on futex_value2 should wake 2 more threads.
  ASSERT_OK(zx_futex_wake(&futex_value2, kThreadWakeAllCount));
  ASSERT_NO_FATAL_FAILURE(AssertWokeThreadCount(threads, std::size(threads), 5));

  // Clean up: Wake the remaining thread so that it can exit.
  ASSERT_OK(zx_futex_wake(&futex_value1, 1));
  ASSERT_NO_FATAL_FAILURE(AssertWokeThreadCount(threads, std::size(threads), std::size(threads)));

  for (auto& t : threads) {
    ASSERT_NO_FATAL_FAILURE(t.Shutdown());
  }

  cleanup.cancel();
}

// Test the case where futex_wait() times out after having been moved to a
// different queue by futex_requeue().  Check that futex_wait() removes
// itself from the correct queue in that case.
TEST(FutexTest, RequeueUnqueuedOnTimeout) {
  zx_futex_t futex_value1(100);
  zx_futex_t futex_value2(200);
  TestThread threads[2];

  // If something goes wrong and we bail out early, do our best to shut down as cleanly as we
  auto cleanup = fit::defer([&]() {
    zx_futex_wake(&futex_value1, kThreadWakeAllCount);
    zx_futex_wake(&futex_value2, kThreadWakeAllCount);
    for (auto& t : threads) {
      t.Shutdown();
    }
  });

  ASSERT_NO_FATAL_FAILURE(threads[0].Start(&futex_value1, zx::msec(1500)));
  ASSERT_OK(zx_futex_requeue(&futex_value1, 0, 100, &futex_value2, kThreadWakeAllCount,
                             ZX_HANDLE_INVALID));
  ASSERT_NO_FATAL_FAILURE(threads[1].Start(&futex_value2));

  // thread 0 and 1 should now both be waiting on futex_value2.  Thread 0
  // should timeout in a short while, but thread 1 should still be waiting.

  ASSERT_NO_FATAL_FAILURE(threads[0].WaitUntilWoken());
  ASSERT_EQ(threads[0].wait_result(), ZX_ERR_TIMED_OUT);
  ASSERT_NO_FATAL_FAILURE(threads[1].CheckIsBlockedOnFutex());

  // thread 0 should have removed itself from futex_value2's wait queue,
  // so only thread 1 should be waiting on futex_value2.  We can test that
  // by doing futex_wake() with count=1.
  ASSERT_OK(zx_futex_wake(&futex_value2, 1));
  ASSERT_NO_FATAL_FAILURE(threads[1].WaitUntilWoken());

  for (auto& t : threads) {
    ASSERT_NO_FATAL_FAILURE(t.Shutdown());
  }

  cleanup.cancel();
}

// Test that the futex_wait() syscall is restarted properly if the thread
// calling it gets suspended and resumed.  (This tests for a bug where the
// futex_wait() syscall would return ZX_ERR_TIMED_OUT and not get restarted by
// the syscall wrapper in the VDSO.)
TEST(FutexTest, ThreadSuspended) {
  zx_futex_t futex_value1(1);

  TestThread thread;

  // If something goes wrong and we bail out early, do our best to shut down as cleanly as we
  auto cleanup = fit::defer([&]() {
    zx_futex_wake(&futex_value1, kThreadWakeAllCount);
    thread.Shutdown();
  });

  ASSERT_NO_FATAL_FAILURE(thread.Start(&futex_value1));

  zx::suspend_token suspend_token;
  ASSERT_OK(thread.thread().suspend(&suspend_token));

  // Wait until the thread is suspended.
  ASSERT_NO_FATAL_FAILURE(WaitForKernelState(thread.thread(), ZX_THREAD_STATE_SUSPENDED));
  ASSERT_OK(zx_handle_close(suspend_token.release()));

  // Wait some time for the thread to resume and execute.
  ASSERT_NO_FATAL_FAILURE(WaitForKernelState(thread.thread(), ZX_THREAD_STATE_BLOCKED_FUTEX));
  ASSERT_NO_FATAL_FAILURE(thread.CheckIsBlockedOnFutex());

  ASSERT_OK(zx_futex_wake(&futex_value1, 1));
  ASSERT_NO_FATAL_FAILURE(AssertWokeThreadCount(&thread, 1, 1));
  ASSERT_NO_FATAL_FAILURE(thread.Shutdown());

  cleanup.cancel();
}

// Test that misaligned pointers cause futex syscalls to return a failure.
TEST(FutexTest, MisalignedFutextAddr) {
  // Make sure the whole thing is aligned, so the 'futex' member will
  // definitely be misaligned.
  alignas(zx_futex_t) struct {
    uint8_t misalign;
    zx_futex_t futex[2];
  } __attribute__((packed)) buffer;

  zx_futex_t* const futex = &buffer.futex[0];
  zx_futex_t* const futex_2 = &buffer.futex[1];

  ASSERT_GT(alignof(zx_futex_t), 1);
  ASSERT_NE(reinterpret_cast<uintptr_t>(futex) % alignof(zx_futex_t), 0);
  ASSERT_NE(reinterpret_cast<uintptr_t>(futex_2) % alignof(zx_futex_t), 0);

  // zx_futex_requeue might check the waited-for value before it
  // checks the second futex's alignment, so make sure the call is
  // valid other than the alignment.  (Also don't ask anybody to
  // look at uninitialized stack space!)
  memset(&buffer, 0, sizeof(buffer));

  ASSERT_EQ(zx_futex_wait(futex, 0, ZX_HANDLE_INVALID, ZX_TIME_INFINITE), ZX_ERR_INVALID_ARGS);
  ASSERT_EQ(zx_futex_wake(futex, 1), ZX_ERR_INVALID_ARGS);
  ASSERT_EQ(zx_futex_requeue(futex, 1, 0, futex_2, 1, ZX_HANDLE_INVALID), ZX_ERR_INVALID_ARGS);
}

void log(const char* str) {
  zx::time now = zx::clock::get_monotonic();
  fprintf(stderr, "[%08" PRIu64 ".%08" PRIu64 "]: %s", now.get() / 1000000000,
          now.get() % 1000000000, str);
}

class Event {
 public:
  Event() : signaled_(0) {}

  void Wait() {
    if (signaled_ == 0) {
      zx_futex_wait(&signaled_, signaled_, ZX_HANDLE_INVALID, ZX_TIME_INFINITE);
    }
  }

  void Signal() {
    if (signaled_ == 0) {
      signaled_ = 1;
      zx_futex_wake(&signaled_, kThreadWakeAllCount);
    }
  }

 private:
  int32_t signaled_;
};

void WaitUntilThreadBlockedOnFutex(thrd_t thread) {
  zx_handle_t thrd_handle = thrd_get_zx_handle(thread);
  ASSERT_NE(thrd_handle, ZX_HANDLE_INVALID);

  zx_info_thread_t info;
  zx_status_t get_info_res = ZX_ERR_INTERNAL;

  WaitFor(
      [&]() {
        get_info_res =
            zx_object_get_info(thrd_handle, ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr);
        return (get_info_res != ZX_OK) || (info.state == ZX_THREAD_STATE_BLOCKED_FUTEX);
      },
      [&]() {
        printf("Waiting for thread to block on futex (0x%x != 0x%x)", info.state,
               ZX_THREAD_STATE_BLOCKED_FUTEX);
      });

  EXPECT_OK(get_info_res);
  EXPECT_EQ(info.state, ZX_THREAD_STATE_BLOCKED_FUTEX);
}

TEST(FutexTest, EventSignaling) {
  thrd_t thread1, thread2, thread3;
  Event event;

  log("starting signal threads\n");
  thrd_create_with_name(
      &thread1,
      [](void* ctx) {
        Event* event = reinterpret_cast<Event*>(ctx);
        log("thread 1 waiting on event\n");
        event->Wait();
        log("thread 1 done\n");
        return 0;
      },
      &event, "thread 1");
  thrd_create_with_name(
      &thread2,
      [](void* ctx) {
        Event* event = reinterpret_cast<Event*>(ctx);
        log("thread 2 waiting on event\n");
        event->Wait();
        log("thread 2 done\n");
        return 0;
      },
      &event, "thread 2");
  thrd_create_with_name(
      &thread3,
      [](void* ctx) {
        Event* event = reinterpret_cast<Event*>(ctx);
        log("thread 3 waiting on event\n");
        event->Wait();
        log("thread 3 done\n");
        return 0;
      },
      &event, "thread 3");

  ASSERT_NO_FATAL_FAILURE(WaitUntilThreadBlockedOnFutex(thread1));
  ASSERT_NO_FATAL_FAILURE(WaitUntilThreadBlockedOnFutex(thread2));
  ASSERT_NO_FATAL_FAILURE(WaitUntilThreadBlockedOnFutex(thread3));

  log("signaling event\n");
  event.Signal();

  log("joining signal threads\n");
  thrd_join(thread1, nullptr);
  log("signal_thread 1 joined\n");
  thrd_join(thread2, nullptr);
  log("signal_thread 2 joined\n");
  thrd_join(thread3, nullptr);
  log("signal_thread 3 joined\n");
}

// Regression test for https://fxbug.dev/42082828.
//
// Verify that concurrently calling zx_futex_wait with an exiting thread as the new owner does not
// create a use-after-free.
TEST(FutexTest, WaitExitRace) {
  // The larger the value, the more likely we are to trigger an interesting race.  The smaller the
  // value, the faster this test will complete.
  constexpr size_t kIterations = 100;
  for (size_t i = 0; i < kIterations; ++i) {
    // We're going to create a thread (t2) that simply waits to be signaled and then exits.  In a
    // different thread, we'll call zx_futex_wait with t2 as the new owner.  Our goal is to race
    // zx_futex_wait with zx_thread_exit.
    std::atomic<bool> ready{false};
    auto wait_then_exit = [&]() {
      while (!ready.load()) {
      };
    };

    // In order to create more opportunity for a race, place the futex in an uncommitted page that
    // will need to be faulted in during zx_futex_wait.
    const size_t size = zx_system_get_page_size();
    zx::vmo vmo;
    ASSERT_OK(zx::vmo::create(size, 0, &vmo));
    zx_vaddr_t addr;
    ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0 /* vmar_offset */,
                                         vmo, 0 /* vmo_offset */, size, &addr));
    auto unmap = fit::defer([&]() { zx::vmar::root_self()->unmap(addr, size); });
    ASSERT_OK(vmo.op_range(ZX_VMO_OP_DECOMMIT, 0, size, nullptr, 0));
    auto* v = reinterpret_cast<zx_futex_t*>(addr);

    std::thread t2(wait_then_exit);

    const zx_instant_mono_t deadline = zx_deadline_after(ZX_USEC(100));
    const zx_handle_t new_futex_owner = thrd_get_zx_handle(t2.native_handle());

    ready.store(true);
    zx_futex_wait(v, 0, new_futex_owner, deadline);
    t2.join();
  }
}

// A helper for WakeWithWakeCountZero and RequeueWithWakeCountZero tests.
//
// Block a thread on a futex to ensure there's an active futex for the requeue
// to interact with.  Cleans up in its destructor.
struct WakeCountZeroHelper {
  zx_futex_t futex{};
  zx_futex_t other_futex{};
  std::thread waiter;

  WakeCountZeroHelper() {
    waiter = std::thread([&]() { zx_futex_wait(&futex, 0, ZX_HANDLE_INVALID, ZX_TIME_INFINITE); });
    ASSERT_NO_FATAL_FAILURE(WaitUntilThreadBlockedOnFutex(waiter.native_handle()));
  }
  ~WakeCountZeroHelper() {
    zx_futex_wake(&futex, kThreadWakeAllCount);
    zx_futex_wake(&other_futex, kThreadWakeAllCount);
    waiter.join();
  }
};

// See that it's valid to requeue an active futex with a wake count of zero.
TEST(FutexTest, WakeWithWakeCountZero) {
  WakeCountZeroHelper helper;
  // Now that we've got an active futex, wake zero and see that we don't crash.
  ASSERT_OK(zx_futex_wake(&helper.futex, 0));
}

TEST(FutexTest, RequeueWithWakeCountZero) {
  WakeCountZeroHelper helper;
  // Now that we've got an active futex, wake zero and see that we don't crash.
  ASSERT_OK(zx_futex_requeue(&helper.futex, 0, 0, &helper.other_futex, kThreadRequeueAllCount,
                             ZX_HANDLE_INVALID));
}

TEST(FutexTest, RequeueCycleStressTest) {
  // This test was written by Gemini, and serves as a regression test for bug
  // 502179440.  See b/502179440 for details.
  constexpr int kNumThreads = 20;
  constexpr int kNumFutexes = 20;
  constexpr int kIterations = 100;

  alignas(zx_futex_t) zx_futex_t futexes[kNumFutexes] = {0};
  std::thread threads[kNumThreads];
  zx_handle_t thread_handles[kNumThreads];
  std::atomic<bool> stop{false};
  std::atomic<bool> ready{false};

  auto thread_func = [&](int id) {
    while (!ready.load()) {
      zx_nanosleep(zx_deadline_after(ZX_MSEC(1)));
    }
    while (!stop.load()) {
      int f_idx = id % kNumFutexes;
      int other_idx = (id + 1) % kNumThreads;
      zx_futex_wait(&futexes[f_idx], 0, thread_handles[other_idx], zx_deadline_after(ZX_MSEC(10)));
    }
  };

  for (int i = 0; i < kNumThreads; i++) {
    threads[i] = std::thread(thread_func, i);
    thread_handles[i] = thrd_get_zx_handle(threads[i].native_handle());
  }

  ready.store(true);

  for (int i = 0; i < kIterations; i++) {
    int f1 = i % kNumFutexes;
    int f2 = (i + 1) % kNumFutexes;
    int t = (i + 2) % kNumThreads;
    zx_futex_requeue(&futexes[f1], 0, 0, &futexes[f2], 100, thread_handles[t]);

    if (i % 10 == 0) {
      zx_nanosleep(zx_deadline_after(ZX_MSEC(1)));
    }
  }

  stop.store(true);
  for (int i = 0; i < kNumFutexes; i++) {
    zx_futex_wake(&futexes[i], kThreadWakeAllCount);
  }

  for (auto& t : threads) {
    t.join();
  }
}

// Regression test for b/505738128.
TEST(FutexTest, FutexRequeueTbiReturnsInvalidArgs) {
#if defined(__aarch64__)
  zx_futex_t futex = 0;
  zx_futex_t* wake_ptr = &futex;

  // Apply a Top-Byte tag (bit 56)
  uintptr_t wake_ptr_int = reinterpret_cast<uintptr_t>(&futex);
  uintptr_t requeue_ptr_int = wake_ptr_int | (1ULL << 56);
  zx_futex_t* requeue_ptr = reinterpret_cast<zx_futex_t*>(requeue_ptr_int);

  // This should not cause a panic.
  zx_status_t status = zx_futex_requeue(wake_ptr, 1, 0, requeue_ptr, 1, ZX_HANDLE_INVALID);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
#else
  ZXTEST_SKIP("TBI is only enabled on arm64");
#endif
}

TEST(FutexTest, GetOwner) {
  zx_futex_t futex = 1;
  zx_koid_t koid = 0x12345;

  // Invalid pointers.
  EXPECT_EQ(zx_futex_get_owner(nullptr, &koid), ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_futex_get_owner(&futex, nullptr), ZX_ERR_INVALID_ARGS);

  // Misaligned pointer.
  alignas(zx_futex_t) uint8_t buffer[sizeof(zx_futex_t) + 1] = {};
  zx_futex_t* misaligned_futex = reinterpret_cast<zx_futex_t*>(buffer + 1);
  EXPECT_EQ(zx_futex_get_owner(misaligned_futex, &koid), ZX_ERR_INVALID_ARGS);

  // Futex with no waiters / no owner.
  EXPECT_OK(zx_futex_get_owner(&futex, &koid));
  EXPECT_EQ(koid, ZX_KOID_INVALID);

  // Futex with a waiter and an owner assigned.
  TestThread waiter;
  Event shutdown_event;

  std::thread owner_thread([&]() { shutdown_event.Wait(); });

  auto join_owner = fit::defer([&]() {
    shutdown_event.Signal();
    owner_thread.join();
  });

  zx_handle_t owner_handle = native_thread_get_zx_handle(owner_thread.native_handle());
  zx_info_handle_basic_t info;
  ASSERT_OK(zx_object_get_info(owner_handle, ZX_INFO_HANDLE_BASIC, &info, sizeof(info), nullptr,
                               nullptr));
  zx_koid_t expected_koid = info.koid;

  ASSERT_NO_FATAL_FAILURE(waiter.Start(&futex, zx::duration::infinite(), owner_handle));

  EXPECT_OK(zx_futex_get_owner(&futex, &koid));
  EXPECT_EQ(koid, expected_koid);

  // Wake waiter, futex returns to no owner.
  EXPECT_OK(zx_futex_wake(&futex, 1));
  ASSERT_NO_FATAL_FAILURE(waiter.WaitUntilWoken());
  waiter.Shutdown();

  // Signal and join owner thread immediately.
  join_owner.call();

  EXPECT_OK(zx_futex_get_owner(&futex, &koid));
  EXPECT_EQ(koid, ZX_KOID_INVALID);
}

TEST(FutexTest, RequeueSingleOwner) {
  zx_futex_t futex1 = 100;
  zx_futex_t futex2 = 200;

  // Invalid arguments: bad pointers or same addr.
  EXPECT_EQ(zx_futex_requeue_single_owner(nullptr, 100, &futex2, 1, ZX_HANDLE_INVALID),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_futex_requeue_single_owner(&futex1, 100, nullptr, 1, ZX_HANDLE_INVALID),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_futex_requeue_single_owner(&futex1, 100, &futex1, 1, ZX_HANDLE_INVALID),
            ZX_ERR_INVALID_ARGS);

  // Value mismatch.
  EXPECT_EQ(zx_futex_requeue_single_owner(&futex1, 999, &futex2, 1, ZX_HANDLE_INVALID),
            ZX_ERR_BAD_STATE);

  TestThread threads[3];
  auto cleanup = fit::defer([&]() {
    zx_futex_wake(&futex1, kThreadWakeAllCount);
    zx_futex_wake(&futex2, kThreadWakeAllCount);
    for (auto& t : threads) {
      t.Shutdown();
    }
  });

  for (auto& t : threads) {
    ASSERT_NO_FATAL_FAILURE(t.Start(&futex1));
  }

  // Requeue single owner: wake 1 thread, assign owner, requeue 2 threads to futex2.
  ASSERT_OK(zx_futex_requeue_single_owner(&futex1, 100, &futex2, 2, ZX_HANDLE_INVALID));

  // Exactly 1 thread was woken.
  ASSERT_NO_FATAL_FAILURE(AssertWokeThreadCount(threads, std::size(threads), 1));

  // Wake the requeued threads on futex2.
  ASSERT_OK(zx_futex_wake(&futex2, kThreadWakeAllCount));
  ASSERT_NO_FATAL_FAILURE(AssertWokeThreadCount(threads, std::size(threads), 3));

  for (auto& t : threads) {
    ASSERT_NO_FATAL_FAILURE(t.Shutdown());
  }

  cleanup.cancel();
}

void CreateDeadThread(zx::thread* out_thread) {
  std::thread dead_thread([]() {});
  ASSERT_OK(zx::unowned_thread(native_thread_get_zx_handle(dead_thread.native_handle()))
                ->duplicate(ZX_RIGHT_SAME_RIGHTS, out_thread));
  dead_thread.join();
  ASSERT_NO_FATAL_FAILURE(WaitForKernelState(*out_thread, ZX_THREAD_STATE_DEAD));
}

TEST(FutexTest, WaitInvalidOwner) {
  zx_futex_t futex = 42;

  // 1. Wrong handle type (e.g., event handle).
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  EXPECT_EQ(zx_futex_wait(&futex, 42, event.get(), ZX_TIME_INFINITE), ZX_ERR_WRONG_TYPE);

  // 2. Unstarted thread handle.
  static constexpr std::string_view kUnstartedWaitName = "unstarted-wait";
  zx::thread unstarted_thread;
  ASSERT_OK(zx::thread::create(*zx::process::self(), kUnstartedWaitName.data(),
                               static_cast<uint32_t>(kUnstartedWaitName.size()), 0,
                               &unstarted_thread));
  EXPECT_EQ(zx_futex_wait(&futex, 42, unstarted_thread.get(), ZX_TIME_INFINITE),
            ZX_ERR_INVALID_ARGS);

  // 3. Current thread as owner.
  EXPECT_EQ(zx_futex_wait(&futex, 42, zx_thread_self(), ZX_TIME_INFINITE), ZX_ERR_INVALID_ARGS);

  // 4. Proposed owner is already waiting on this futex.
  TestThread waiter;
  ASSERT_NO_FATAL_FAILURE(waiter.Start(&futex));
  EXPECT_EQ(zx_futex_wait(&futex, 42, waiter.thread().get(), ZX_TIME_INFINITE),
            ZX_ERR_INVALID_ARGS);
  EXPECT_OK(zx_futex_wake(&futex, 1));
  ASSERT_NO_FATAL_FAILURE(waiter.WaitUntilWoken());
  waiter.Shutdown();

  // 5. Dead / terminated thread as owner.
  zx::thread dead_thread;
  ASSERT_NO_FATAL_FAILURE(CreateDeadThread(&dead_thread));

  // Waiting with a dead thread owner succeeds (owner is ignored/reset) and times out cleanly.
  EXPECT_EQ(zx_futex_wait(&futex, 42, dead_thread.get(), zx_deadline_after(ZX_USEC(100))),
            ZX_ERR_TIMED_OUT);
}

TEST(FutexTest, RequeueInvalidOwner) {
  zx_futex_t futex1 = 100;
  zx_futex_t futex2 = 200;

  // 1. Wrong handle type (e.g., event handle).
  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));
  EXPECT_EQ(zx_futex_requeue(&futex1, 1, 100, &futex2, 1, event.get()), ZX_ERR_WRONG_TYPE);

  // 2. Unstarted thread handle.
  static constexpr std::string_view kUnstartedReqName = "unstarted-req";
  zx::thread unstarted_thread;
  ASSERT_OK(zx::thread::create(*zx::process::self(), kUnstartedReqName.data(),
                               static_cast<uint32_t>(kUnstartedReqName.size()), 0,
                               &unstarted_thread));
  EXPECT_EQ(zx_futex_requeue(&futex1, 1, 100, &futex2, 1, unstarted_thread.get()),
            ZX_ERR_INVALID_ARGS);

  // 3. Proposed owner is waiting on wake or requeue futex.
  TestThread waiter_wake;
  TestThread waiter_requeue;
  ASSERT_NO_FATAL_FAILURE(waiter_wake.Start(&futex1));
  ASSERT_NO_FATAL_FAILURE(waiter_requeue.Start(&futex2));
  EXPECT_EQ(zx_futex_requeue(&futex1, 1, 100, &futex2, 1, waiter_wake.thread().get()),
            ZX_ERR_INVALID_ARGS);
  EXPECT_EQ(zx_futex_requeue(&futex1, 1, 100, &futex2, 1, waiter_requeue.thread().get()),
            ZX_ERR_INVALID_ARGS);
  EXPECT_OK(zx_futex_wake(&futex1, 1));
  EXPECT_OK(zx_futex_wake(&futex2, 1));
  ASSERT_NO_FATAL_FAILURE(waiter_wake.WaitUntilWoken());
  ASSERT_NO_FATAL_FAILURE(waiter_requeue.WaitUntilWoken());
  waiter_wake.Shutdown();
  waiter_requeue.Shutdown();

  // 4. Dead / terminated thread as owner.
  zx::thread dead_thread;
  ASSERT_NO_FATAL_FAILURE(CreateDeadThread(&dead_thread));

  EXPECT_OK(zx_futex_requeue(&futex1, 0, 100, &futex2, 0, dead_thread.get()));
}

TEST(FutexTest, UnmappedAddress) {
  const size_t size = zx_system_get_page_size();
  zx::vmo vmo;
  ASSERT_OK(zx::vmo::create(size, 0, &vmo));
  zx_vaddr_t addr;
  ASSERT_OK(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0 /* vmar_offset */, vmo,
                                       0 /* vmo_offset */, size, &addr));
  ASSERT_OK(zx::vmar::root_self()->unmap(addr, size));
  auto* unmapped_futex = reinterpret_cast<zx_futex_t*>(addr);

  // futex wait on an unmapped address returns ZX_ERR_NOT_FOUND.
  EXPECT_EQ(zx_futex_wait(unmapped_futex, 0, ZX_HANDLE_INVALID, 0), ZX_ERR_NOT_FOUND);

  // futex requeue on an unmapped address returns ZX_ERR_NOT_FOUND.
  zx_futex_t valid_futex = 0;
  EXPECT_EQ(zx_futex_requeue(unmapped_futex, 1, 0, &valid_futex, 1, ZX_HANDLE_INVALID),
            ZX_ERR_NOT_FOUND);
}

#if defined(__aarch64__)
// Regression tests for b/505923588
#define NULL_FUTEX_PTR ((zx_futex_t*)0xff00000000000000ULL)

TEST(FutexTest, WaitWithNullIdReturnsInvalidArgs) {
  zx_status_t status = zx_futex_wait(NULL_FUTEX_PTR, 0, ZX_HANDLE_INVALID, ZX_TIME_INFINITE_PAST);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
}

TEST(FutexTest, RequeueWithFirstNullIdReturnsInvalidArgs) {
  zx_futex_t valid_futex = 0;
  zx_status_t status = zx_futex_requeue(NULL_FUTEX_PTR, 1, 0, &valid_futex, 1, ZX_HANDLE_INVALID);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
}

TEST(FutexTest, RequeueWithSecondNullIdReturnsInvalidArgs) {
  zx_futex_t valid_futex = 0;
  zx_status_t status = zx_futex_requeue(&valid_futex, 1, 0, NULL_FUTEX_PTR, 1, ZX_HANDLE_INVALID);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
}

TEST(FutexTest, WakeWithNullIdReturnsInvalidArgs) {
  zx_status_t status = zx_futex_wake(NULL_FUTEX_PTR, 1);
  EXPECT_EQ(ZX_ERR_INVALID_ARGS, status);
}
#endif  // defined(__aarch64__)
}  // namespace
}  // namespace futex
