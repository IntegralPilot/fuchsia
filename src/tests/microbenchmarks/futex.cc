// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/atomic.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zx/counter.h>
#include <unistd.h>
#include <zircon/syscalls.h>
#include <zircon/threads.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <format>
#include <random>
#include <thread>

#include <perftest/perftest.h>

namespace {

// Wait until |thread| is in state |state|.
void WaitThreadState(zx_handle_t thread, zx_thread_state_t state) {
  while (true) {
    zx_info_thread_t info;
    zx_status_t status =
        zx_object_get_info(thread, ZX_INFO_THREAD, &info, sizeof(info), nullptr, nullptr);
    FX_CHECK(status == ZX_OK);
    if (info.state == state) {
      return;
    }
    zx_nanosleep(zx_deadline_after(ZX_MSEC(1)));
  }
}

// Measure the time to futex wake a single waiter, as a function of the number of "active futexes".
bool FutexWakeOne(perftest::RepeatState* state, size_t num_futexes) {
  // Each iteration of this test has two phases, Prepare and Wake.  In the Prepare phase we wait
  // until every waiter thread has blocked on a futex, then we select one at random.  In the Wake
  // phase we wake the selected thread, measuring the amount of time spent in zx_futex_wake.
  state->DeclareStep("Prepare");
  state->DeclareStep("Wake");

  // A random number generator used to select one thread at random.
  std::mt19937 rng{std::random_device()()};

  // We use a counter to synchronize the waker and waiter threads.  Each waiter decrements the
  // counter just before it calls zx_futex_wait.  When the counter hits zero, the waker knows that
  // the waiters will all soon be blocked on their futexes.
  //
  // We maintain the invariant, 0 <= counter_value <= num_futexes.
  zx::counter num_running;
  zx_status_t status = zx::counter::create(0, &num_running);
  FX_CHECK(status == ZX_OK);
  FX_CHECK(num_running.write(num_futexes) == ZX_OK);

  auto waiter = [&num_running](zx_futex_t* f) {
    while (true) {
      // Notify that we are about to call futex wait.  It's critical that after we decrement, we do
      // not do anything that might cause this thread to block on a futex other than |f|.  If we
      // were to block on some futex other than |f| after we've decremented, then the waker thread
      // may think we're blocked on |f| and issue a wake on |f| before we wait on it, resulting in a
      // "lost wakeup".  Because various libraries (including libc) may internally make use of
      // futexes, avoid calling anything other than vDSO routines between decrement and wait to
      // avoid introducing an errant futex operation.
      //
      // What would a lost wakeup look like?  It would likely manifest as a measurement with a
      // smaller number of active futexes than intended rather than a hang.

      // Decrement.
      zx_status_t status = num_running.add(-1);
      FX_CHECK(status == ZX_OK);

      // Wait.  Block if the value is 0.
      status = zx_futex_wait(f, 0, ZX_HANDLE_INVALID, ZX_TIME_INFINITE);

      // If the value isn't 0 then we've been signaled to terminate.
      if (status == ZX_ERR_BAD_STATE) {
        return;
      }
      FX_CHECK(status == ZX_OK);
    }
  };

  // Create futexes and waiter threads.  The i'th thread will wait on the i'th futex.
  std::vector<zx_futex_t> futexes(num_futexes, 0);
  std::vector<std::thread> threads;
  threads.reserve(num_futexes);
  for (size_t i = 0; i < num_futexes; ++i) {
    zx_futex_t* f = &futexes[i];
    threads.emplace_back(waiter, f);
  }

  while (state->KeepRunning()) {
    // Because we want to measure the futex wake operation as a function of the number of active
    // futexes, we must wait until every thread is blocked on the futex before we wake one of them.
    // To ensure that we observe the threads are waiting on the right futex, we use a counter.  Each
    // thread will decrement the counter just before it calls futex wait.  We wait until the counter
    // hits zero before waiting to see that each thread is "blocked in futex".
    zx_signals_t signals = ZX_COUNTER_NON_POSITIVE;
    FX_CHECK(num_running.wait_one(signals, zx::time::infinite(), &signals) == ZX_OK);
    FX_CHECK(signals & ZX_COUNTER_NON_POSITIVE);
    for (size_t i = 0; i < num_futexes; ++i) {
      WaitThreadState(native_thread_get_zx_handle(threads[i].native_handle()),
                      ZX_THREAD_STATE_BLOCKED_FUTEX);
    }

    // Select one thread at random.
    const size_t selected = std::uniform_int_distribution<size_t>(0, num_futexes - 1)(rng);
    zx_futex_t* futex = &futexes[selected];

    // Increment the counter so that the subsequent loop iteration by the woken thread will set us
    // up for another trip through our KeepRunning loop.
    FX_CHECK(num_running.add(1) == ZX_OK);

    state->NextStep();

    // Wake it.
    FX_CHECK(zx_futex_wake(futex, 1) == ZX_OK);
  }

  // Unblock them all and join.
  for (size_t i = 0; i < num_futexes; ++i) {
    cpp20::atomic_ref<zx_futex_t>(futexes[i]).store(1, std::memory_order_release);
    FX_CHECK(zx_futex_wake(&futexes[i], 1) == ZX_OK);
    threads[i].join();
  }
  return true;
}

void RegisterTests() {
  for (size_t num_futexes : {1, 8, 32, 128, 256, 512, 1024, 2048}) {
    auto test_name = std::format("FutexWakeOne/{:04}Waiting", num_futexes);
    perftest::RegisterTest(test_name.c_str(), FutexWakeOne, num_futexes);
  }
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
