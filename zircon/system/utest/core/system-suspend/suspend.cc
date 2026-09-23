// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/defer.h>
#include <lib/standalone-test/standalone.h>
#include <lib/sync/completion.h>
#include <lib/zx/clock.h>
#include <lib/zx/event.h>
#include <lib/zx/interrupt.h>
#include <lib/zx/port.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <zircon/errors.h>
#include <zircon/status.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls.h>
#include <zircon/syscalls/port.h>
#include <zircon/syscalls/resource.h>
#include <zircon/syscalls/system.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <condition_variable>
#include <iostream>
#include <mutex>
#include <random>
#include <thread>
#include <vector>

#include <zxtest/zxtest.h>

#include "../needs-next.h"

NEEDS_NEXT_SYSCALL(zx_system_suspend_enter);

namespace {

zx::result<zx::resource> GetSystemCpuResource() {
  zx::resource system_cpu_resource;
  const zx_status_t status =
      zx::resource::create(*standalone::GetSystemResource(), ZX_RSRC_KIND_SYSTEM,
                           ZX_RSRC_SYSTEM_CPU_BASE, 1, nullptr, 0, &system_cpu_resource);
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(system_cpu_resource));
}

class Random {
 public:
  Random() = default;

  zx::duration GetUniform(zx::duration min, zx::duration max) {
    std::uniform_int_distribution<zx_duration_t> distribution{min.get(), max.get()};
    return zx::duration{distribution(generator_)};
  }

 private:
  std::mt19937_64 generator_{std::random_device{}()};
};

TEST(SystemSuspend, ResourceValidation) {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  const zx::result resource_result = GetSystemCpuResource();
  ASSERT_OK(resource_result.status_value());

  zx::event event;
  ASSERT_OK(zx::event::create(0, &event));

  // Confirm that calls to suspend_enter succeed when we pass the proper resource.  We are not
  // actually going to suspend here, as our deadline is INFINITE_PAST, but if we bothered to request
  // a wake-source report and check its results, we'd see that the system-wide deadline wake source
  // had fired and prevented our suspend.  We'll confirm this during the TimeoutIsPast tests.
  EXPECT_OK(zx_system_suspend_enter(resource_result->get(), ZX_TIME_INFINITE_PAST, 0, nullptr,
                                    nullptr, 0, nullptr));

  // Try to pass a handle to an event as our resource, instead of the actual resource token.  Our
  // call should fail and the error should be WRONG_TYPE.
  EXPECT_EQ(ZX_ERR_WRONG_TYPE, zx_system_suspend_enter(event.get(), ZX_TIME_INFINITE_PAST, 0,
                                                       nullptr, nullptr, 0, nullptr));

  // TODO(eieio): This syscall uses standard resource validation. Do we need to cover all cases in
  // this test?
}

TEST(SystemSuspend, TimeoutIsPast) {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  const zx::result resource_result = GetSystemCpuResource();
  ASSERT_OK(resource_result.status_value());

  zx_wake_source_report_header_t hdr;
  std::array<zx_wake_source_report_entry_t, 4> entries;
  uint32_t actual_entries;

  auto ResetReport = [&]() {
    ::memset(&hdr, 0, sizeof(hdr));
    for (zx_wake_source_report_entry_t& entry : entries) {
      ::memset(&entry, 0, sizeof(entry));
    }
    actual_entries = 0;
  };

  auto VerifyTimeout = [&]() {
    ASSERT_LE(actual_entries, entries.size());

    bool found_timeout = false;
    for (uint32_t i = 0; i < actual_entries; ++i) {
      if (entries[i].koid == ZX_KOID_KERNEL) {
        found_timeout = true;
        break;
      }
    }

    EXPECT_TRUE(found_timeout);
  };

  const zx::time_boot almost_now = zx::clock::get_boot() - zx::nsec(1);
  ResetReport();
  EXPECT_OK(zx_system_suspend_enter(resource_result->get(), almost_now.get(), 0, &hdr,
                                    entries.data(), entries.size(), &actual_entries));
  VerifyTimeout();

  ResetReport();
  EXPECT_OK(zx_system_suspend_enter(resource_result->get(), ZX_TIME_INFINITE_PAST, 0, &hdr,
                                    entries.data(), entries.size(), &actual_entries));
  VerifyTimeout();

  ResetReport();
  EXPECT_OK(zx_system_suspend_enter(resource_result->get(), 0, 0, &hdr, entries.data(),
                                    entries.size(), &actual_entries));
  VerifyTimeout();
}

TEST(SystemSuspend, SuspendAndResumeByTimer) {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  const zx::result resource_result = GetSystemCpuResource();
  ASSERT_OK(resource_result.status_value());

  // Avoid flakes due to VM pauses by using increasing resume durations until timeouts do not occur.
  zx::time_boot resume_at;
  zx_status_t suspend_status;
  zx::duration suspend_duration = zx::sec(1);
  do {
    resume_at = zx::clock::get_boot() + suspend_duration;
    suspend_status = zx_system_suspend_enter(resource_result->get(), resume_at.get(), 0, nullptr,
                                             nullptr, 0, nullptr);
    suspend_duration *= 2;
  } while (suspend_status == ZX_ERR_TIMED_OUT);

  EXPECT_OK(suspend_status);
  EXPECT_GT(zx::clock::get_boot(), resume_at);
}

TEST(SystemSuspend, ConcurrentSuspend) {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  const zx::result resource_result = GetSystemCpuResource();
  ASSERT_OK(resource_result.status_value());

  const size_t num_threads = 4;
  const size_t suspend_tries_per_thread = 10;

  std::mutex lock;
  std::condition_variable initialized_condition;
  std::condition_variable start_condition;
  size_t initialized_count = 0;
  bool start_flag = 0;

  const auto suspend_tester = [&](const size_t id) {
    std::cout << "Suspend tester " << id << " starting up...\n";
    {
      std::unique_lock<std::mutex> guard{lock};
      initialized_count++;
      initialized_condition.notify_one();
      start_condition.wait(guard, [&] { return start_flag; });
    }

    Random random;

    for (size_t i = 0; i < suspend_tries_per_thread; i++) {
      // Vary the suspend duration so that some attempts are likely to succeed and some are likely
      // to abort due to reaching the resume time before suspend completes.
      const zx::duration suspend_duration = random.GetUniform(zx::msec(10), zx::sec(1));

      std::cout << "Suspend tester " << id << " attempt " << i << "...\n";
      const zx::time_boot resume_at = zx::clock::get_boot() + suspend_duration;
      const zx_status_t suspend_status = zx_system_suspend_enter(
          resource_result->get(), resume_at.get(), 0, nullptr, nullptr, 0, nullptr);
      EXPECT_TRUE(suspend_status == ZX_OK || suspend_status == ZX_ERR_TIMED_OUT);
      std::cout << "Suspend tester " << id << " attempt " << i
                << ": status=" << zx_status_get_string(suspend_status) << "\n";
    }
  };

  std::vector<std::thread> threads;
  threads.reserve(num_threads);
  for (size_t i = 0; i < num_threads; i++) {
    threads.emplace_back(suspend_tester, i);
  }

  {
    std::unique_lock<std::mutex> guard{lock};
    std::cout << "Waiting for suspend test threads to start...\n";
    initialized_condition.wait(guard, [&] { return initialized_count == num_threads; });
    start_flag = true;
  }
  start_condition.notify_all();

  for (std::thread& thread : threads) {
    thread.join();
  }
}

// This is a regression test for https://fxbug.dev/368687980
TEST(SystemSuspend, FailureToEnterSuspendRegressionTest) {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  const zx::result resource_result = GetSystemCpuResource();
  ASSERT_OK(resource_result.status_value());

  std::atomic<bool> stop = false;
  sync_completion_t started{};

  // Set up a thread who just spins.  This is the thread which (with the
  // original bug in place) will prevent the scheduler from properly choosing
  // the idle-power thread when secondary CPUs have been asked to suspend.
  std::thread t1([&]() {
    sync_completion_signal(&started);
    while (!stop.load()) {
      // spin until the test ends.
    }
  });

  // No matter what happens, make sure we clean up our spinning thread as we exit.
  auto cleanup = fit::defer([&]() {
    stop.store(true);
    t1.join();
  });

  // Wait until our spinning thread has started.
  sync_completion_wait(&started, ZX_TIME_INFINITE);

  for (int i = 0; i < 5; i++) {
    const zx_instant_boot_t deadline = zx_clock_get_boot() + ZX_MSEC(100);
    const zx_status_t suspend_result =
        zx_system_suspend_enter(resource_result->get(), deadline, ZX_SYSTEM_SUSPEND_OPTION_DISCARD,
                                nullptr, nullptr, 0, nullptr);

    // Note; if/when this test fails, it will not return an explicit error code.
    // Instead, it will time out attempting to transition secondary CPUs into
    // their suspended state and report a KERNEL OOPS.  We rely on the tefmo
    // checker seeing this OOPS in the log and declaring the test to have been a
    // failure as a result.
    ASSERT_EQ(ZX_OK, suspend_result);
  }
}

// This is a regression test for https://fxbug.dev/511517231
TEST(SystemSuspend, VirtualWakeEventRacingWithSuspendOpPanicRepro) {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  const zx::result resource_result = GetSystemCpuResource();
  ASSERT_OK(resource_result.status_value());

  // Create a virtual wake-vector interrupt, and a port, then bind the two
  // together.  Using a port makes it much easier to explicitly acknowledge the
  // interrupt when setting up the sequences of races to follow.
  zx::interrupt irq;
  constexpr uint32_t options = ZX_INTERRUPT_VIRTUAL | ZX_INTERRUPT_WAKE_VECTOR;
  ASSERT_OK(zx::interrupt::create(*standalone::GetIrqResource(), 0, options, &irq));

  zx::port port;
  constexpr uint32_t port_options = ZX_PORT_BIND_TO_INTERRUPT;
  ASSERT_OK(zx::port::create(port_options, &port));
  ASSERT_OK(irq.bind(port, 0, ZX_INTERRUPT_BIND));

  enum class State {
    Initial = 0,
    Go,
    Triggered,
    DoReset,
    TriggerThreadReady,
    Shutdown,
  };
  std::atomic<State> state{State::Initial};

  enum class WaitResult { Ok, StopTest };
  auto WaitState = [&state](State expected) -> WaitResult {
    while (true) {
      const State observed = state.load();
      if (observed == State::Shutdown) {
        return WaitResult::StopTest;
      }

      if (observed == expected) {
        return WaitResult::Ok;
      }
    }
  };

  std::thread t1([&]() {
    while (true) {
      // Let the coordinator know we are ready to start another cycle, then wait
      // for them to tell us to Go.
      state.store(State::TriggerThreadReady);
      if (WaitState(State::Go) == WaitResult::StopTest) {
        break;
      }
      irq.trigger(0, zx::clock::get_monotonic());

      // Let the coordinator know that we have triggered the interrupt, and we
      // are ready to reset and start again.
      state.store(State::Triggered);
      if (WaitState(State::DoReset) == WaitResult::StopTest) {
        break;
      }

      // Perform the actual reset (reading our packet and acking our interrupt),
      // then start a new cycle.
      zx_port_packet_t irq_packet;
      port.wait(zx::time::infinite(), &irq_packet);
      irq.ack();
    }
  });

  for (int i = 0; i < 10; i++) {
    // Wait for the trigger thread to be ready, then tell it to Go and start the
    // race by calling suspend.
    WaitState(State::TriggerThreadReady);
    const zx_instant_boot_t deadline = zx_clock_get_boot() + ZX_MSEC(10);
    state.store(State::Go);
    zx_system_suspend_enter(resource_result->get(), deadline, ZX_SYSTEM_SUSPEND_OPTION_DISCARD,
                            nullptr, nullptr, 0, nullptr);

    // Wait until we know that the interrupt was triggered, then tell the
    // trigger thread that we are ready to start the next cycle.
    WaitState(State::Triggered);
    state.store(State::DoReset);
  }

  // Make sure that the trigger thread is parked and waiting for the race to
  // start before telling it to shutdown.
  WaitState(State::TriggerThreadReady);
  state.store(State::Shutdown);
  t1.join();
}

}  // anonymous namespace
