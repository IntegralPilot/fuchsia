// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <assert.h>
#include <lib/standalone-test/standalone.h>
#include <lib/zx/clock.h>
#include <lib/zx/object.h>
#include <lib/zx/port.h>
#include <lib/zx/resource.h>
#include <lib/zx/result.h>
#include <lib/zx/time.h>
#include <lib/zx/timer.h>
#include <stdio.h>
#include <threads.h>
#include <unistd.h>
#include <zircon/syscalls-next.h>
#include <zircon/syscalls/port.h>
#include <zircon/syscalls/resource.h>

#include <zxtest/zxtest.h>

#include "../needs-next.h"

NEEDS_NEXT_SYSCALL(zx_system_suspend_enter);

namespace {

#define INVALID_CLOCK ((zx_clock_t)2)

void CheckInfo(const zx::timer& timer, uint32_t options, zx_time_t deadline, zx_duration_t slack) {
  zx_info_timer_t info = {};
  ASSERT_OK(timer.get_info(ZX_INFO_TIMER, &info, sizeof(info), nullptr, nullptr));
  EXPECT_EQ(info.options, options);
  EXPECT_EQ(info.deadline, deadline);
  EXPECT_EQ(info.slack, slack);
}

TEST(Timer, DeadlineAfter) {
  const zx::time then = zx::clock::get_monotonic();
  // The day we manage to boot and run this test in less than 1uS we need to fix this.
  ASSERT_GT(then, zx::time(1000u));

  const zx::time one_hour_later = zx::deadline_after(zx::hour(1));
  EXPECT_LT(then, one_hour_later);

  const zx::duration too_big = zx::duration(INT64_MAX - 100);
  const zx::time clamped = zx::deadline_after(too_big);
  EXPECT_EQ(clamped, zx::time::infinite());

  EXPECT_LT(zx::time(), zx::deadline_after(10 * 365 * zx::hour(24)));
  EXPECT_LT(zx::deadline_after(zx::duration::infinite_past()), zx::time());
}

TEST(Timer, SetNegativeDeadline) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));
  CheckInfo(timer, 0, 0, 0);
  zx::duration slack;
  ASSERT_OK(timer.set(zx::time(-1), slack));
  CheckInfo(timer, 0, 0, slack.get());
  zx_signals_t pending;
  ASSERT_OK(timer.wait_one(ZX_TIMER_SIGNALED, zx::time::infinite(), &pending));
  ASSERT_EQ(pending, ZX_TIMER_SIGNALED);
  CheckInfo(timer, 0, 0, 0);
}

TEST(Timer, SetNegativeDeadlineMax) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));
  zx::duration slack;
  ASSERT_OK(timer.set(zx::time(ZX_TIME_INFINITE_PAST), slack));
  CheckInfo(timer, 0, 0, slack.get());
  zx_signals_t pending;
  ASSERT_OK(timer.wait_one(ZX_TIMER_SIGNALED, zx::time::infinite(), &pending));
  ASSERT_EQ(pending, ZX_TIMER_SIGNALED);
  CheckInfo(timer, 0, 0, 0);
}

TEST(Timer, SetNegativeSlack) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));
  ASSERT_EQ(timer.set(zx::time(), zx::duration(-1)), ZX_ERR_OUT_OF_RANGE);
  CheckInfo(timer, 0, 0, 0);
}

TEST(Timer, AlreadyPassedDeadlineOnWaitOne) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));
  CheckInfo(timer, 0, 0, 0);

  zx::duration slack;
  ASSERT_OK(timer.set(zx::time(ZX_TIME_INFINITE_PAST), slack));
  CheckInfo(timer, 0, 0, slack.get());

  zx_signals_t pending;
  ASSERT_OK(timer.wait_one(ZX_TIMER_SIGNALED, zx::time::infinite_past(), &pending));
  ASSERT_EQ(pending, ZX_TIMER_SIGNALED);
  CheckInfo(timer, 0, 0, 0);
}

TEST(Timer, Basic) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));

  zx_signals_t pending;
  EXPECT_EQ(timer.wait_one(ZX_TIMER_SIGNALED, zx::time(), &pending), ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0u);

  for (int ix = 0; ix != 3; ++ix) {
    const zx::time deadline_timer = zx::deadline_after(zx::msec(10));
    const zx::time deadline_wait = zx::deadline_after(zx::sec(1000));
    // Timer should fire faster than the wait timeout.
    ASSERT_OK(timer.set(deadline_timer, zx::nsec(0)));

    EXPECT_OK(timer.wait_one(ZX_TIMER_SIGNALED, deadline_wait, &pending));
    EXPECT_EQ(pending, ZX_TIMER_SIGNALED);
    CheckInfo(timer, 0, 0, 0);
  }
}

TEST(Timer, Restart) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));

  zx_signals_t pending;
  // The timer deadline is set for one year in the future, and therefore should never race
  // with the wait_one call below.
  const zx::duration relative_deadline = zx::sec(60 * 60 * 24 * 365);
  for (int ix = 0; ix != 10; ++ix) {
    const zx::time deadline_timer = zx::deadline_after(relative_deadline);
    const zx::time deadline_wait = zx::deadline_after(zx::msec(1));
    // Setting a timer already running is equivalent to a cancel + set.
    ASSERT_OK(timer.set(deadline_timer, zx::nsec(0)));
    CheckInfo(timer, 0, deadline_timer.get(), 0);

    EXPECT_EQ(timer.wait_one(ZX_TIMER_SIGNALED, deadline_wait, &pending), ZX_ERR_TIMED_OUT);
    EXPECT_EQ(pending, 0u);
    CheckInfo(timer, 0, deadline_timer.get(), 0);
  }
}

TEST(Timer, InvalidCalls) {
  zx::timer timer;
  ASSERT_EQ(zx::timer::create(0, INVALID_CLOCK, &timer), ZX_ERR_INVALID_ARGS);
  ASSERT_EQ(zx::timer::create(ZX_TIMER_SLACK_LATE + 1, INVALID_CLOCK, &timer), ZX_ERR_INVALID_ARGS);
}

TEST(Timer, EdgeCases) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));
  ASSERT_OK(timer.set(zx::time(), zx::nsec(0)));
}

// furiously spin resetting the timer, trying to race with it going off to look for
// race conditions.
TEST(Timer, RestartRace) {
  const zx::duration kTestDuration = zx::msec(5);
  const zx::time start = zx::clock::get_monotonic();

  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));
  while (zx::clock::get_monotonic() - start < kTestDuration) {
    ASSERT_OK(timer.set(zx::deadline_after(zx::usec(100)), zx::nsec(0)));
  }

  EXPECT_OK(timer.cancel());
}

// If the timer is already due at the moment it is started then the signal should be
// asserted immediately.  Likewise canceling the timer should immediately deassert
// the signal.
TEST(Timer, SignalsAssertedImmediately) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));

  for (int i = 0; i < 100; i++) {
    const zx::time now = zx::clock::get_monotonic();

    EXPECT_OK(timer.set(now, zx::nsec(0)));

    zx_signals_t pending;
    EXPECT_OK(timer.wait_one(ZX_TIMER_SIGNALED, zx::time(), &pending));
    EXPECT_EQ(pending, ZX_TIMER_SIGNALED);

    EXPECT_OK(timer.cancel());

    EXPECT_EQ(timer.wait_one(ZX_TIMER_SIGNALED, zx::time(), &pending), ZX_ERR_TIMED_OUT);
    EXPECT_EQ(pending, 0u);
  }
}

// Tests using CheckCoalescing are disabled because they are flaky. The system might have a timer
// nearby |deadline_1| or |deadline_2| and as such the test will fire either earlier or later than
// expected. The precise behavior is still tested by the "k timer tests" command.
//
// See https://fxbug.dev/42105960 for the current owner.
void CheckCoalescing(uint32_t mode) {
  // The second timer will coalesce to the first one for ZX_TIMER_SLACK_LATE
  // but not for  ZX_TIMER_SLACK_EARLY. This test is not precise because the
  // system might have other timers that interfere with it. Precise tests are
  // avaliable as kernel tests.

  zx::timer timer_1;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer_1));
  zx::timer timer_2;
  ASSERT_OK(zx::timer::create(mode, ZX_CLOCK_MONOTONIC, &timer_2));

  const zx::time start = zx::clock::get_monotonic();

  const zx::time deadline_1 = start + zx::msec(350);
  const zx::time deadline_2 = start + zx::msec(250);

  ASSERT_OK(timer_1.set(deadline_1, zx::nsec(0)));
  ASSERT_OK(timer_2.set(deadline_2, zx::msec(110)));
  CheckInfo(timer_2, 0, deadline_2.get(), ZX_MSEC(110));

  zx_signals_t pending;
  EXPECT_OK(timer_2.wait_one(ZX_TIMER_SIGNALED, zx::time::infinite(), &pending));
  EXPECT_EQ(pending, ZX_TIMER_SIGNALED);
  CheckInfo(timer_2, 0, 0, 0);

  const zx::duration duration = zx::clock::get_monotonic() - start;

  if (mode == ZX_TIMER_SLACK_LATE) {
    EXPECT_GE(duration, zx::msec(350));
  } else if (mode == ZX_TIMER_SLACK_EARLY) {
    EXPECT_LE(duration, zx::msec(345));
  } else {
    assert(false);
  }
}

// This test verifies that timers on the boot timeline elapse during periods of suspension.
// TODO(https://fxbug.dev/341785588): Once we start pausing the monotonic clock during suspend, we
// should test that timers on the monotonic timeline do not elapse during periods of suspension.
TEST(Timer, BootTimersElapseDuringSuspend) {
  NEEDS_NEXT_SKIP(zx_system_suspend_enter);

  // This test verifies that boot timers fire during periods of suspension by:
  // 1. Setting a boot timer to elapse in 5 seconds.
  // 2. Suspending the system for 7 seconds.
  // 3. Verifying that the boot timer was triggered during the period of suspension.
  //
  // Note that step (3) is more complicated than one would assume, as we cannot simply assert that
  // the ZX_TIMER_SIGNALED signal is set on the timer object. This is due to a fundamental race in
  // the way the signal is set inside the kernel. Specifically, when a timer interrupt fires, it
  // schedules a DPC to set the ZX_TIMER_SIGNALED bit. However, this DPC thread will not run during
  // suspend-to-idle, and may not even run before this test thread after the system resumes.
  // As a result, we must rely on the kernel's zx_object_wait_async machinery to tell us when the
  // signal is set. Once we know the signal is set, we want to verify that it was set before
  // suspension ended, but all we have in the resulting port packet is the monotonic timestamp
  // at which the timer was signaled. Therefore, all we can really verify is that this signal was
  // asserted before the current time, but this should be a sufficiently tight bound for most cases.

  // Create a boot timer and a port that will wait on this timer firing.
  zx::timer timer_boot;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_BOOT, &timer_boot));
  zx::port timer_port;
  ASSERT_OK(zx::port::create(0, &timer_port));
  constexpr uint64_t kWaitKey = 1;
  ASSERT_OK(
      timer_boot.wait_async(timer_port, kWaitKey, ZX_TIMER_SIGNALED, ZX_WAIT_ASYNC_TIMESTAMP));

  // Set the timer's deadline to 5 seconds in the future.
  const zx::time_boot deadline_boot = zx::clock::get_boot() + zx::sec(5);
  ASSERT_OK(timer_boot.set(deadline_boot, zx::nsec(0)));

  // Suspend the system for 7 seconds.
  zx::unowned_resource system_resource = standalone::GetSystemResource();
  const zx::result resource_result =
      standalone::GetSystemResourceWithBase(system_resource, ZX_RSRC_SYSTEM_CPU_BASE);
  ASSERT_OK(resource_result.status_value());
  const zx::time_boot deadline_suspend = zx::clock::get_boot() + zx::sec(7);
  ASSERT_OK(zx_system_suspend_enter(resource_result->get(), deadline_suspend.get(), 0, nullptr,
                                    nullptr, 0, nullptr));

  // Verify that the boot timer was signaled before now, as explained in the block comment above.
  zx_port_packet_t pkt;
  ASSERT_OK(timer_port.wait(zx::time::infinite(), &pkt));
  ASSERT_EQ(pkt.type, ZX_PKT_TYPE_SIGNAL_ONE);
  EXPECT_LE(pkt.signal.timestamp, zx::clock::get_monotonic().get());
}

// Test is disabled, see |CheckCoalescing|.
TEST(Timer, DISABLED_CoalesceTestEarly) {
  ASSERT_NO_FAILURES(CheckCoalescing(ZX_TIMER_SLACK_EARLY));
}

// Test is disabled, see |CheckCoalescing|.
TEST(Timer, DISABLED_CoalesceTestLate) { ASSERT_NO_FAILURES(CheckCoalescing(ZX_TIMER_SLACK_LATE)); }

TEST(Timer, UserSignal) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));

  // Assert user signals.
  EXPECT_OK(timer.signal(0, ZX_USER_SIGNAL_0));
  zx_signals_t pending = 0;
  EXPECT_OK(timer.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite_past(), &pending));
  EXPECT_EQ(pending, ZX_USER_SIGNAL_0);

  // Clear user signals.
  EXPECT_OK(timer.signal(ZX_USER_SIGNAL_0, 0));
  EXPECT_EQ(timer.wait_one(ZX_USER_SIGNAL_0, zx::time::infinite_past(), &pending),
            ZX_ERR_TIMED_OUT);
  EXPECT_EQ(pending, 0u);
}

TEST(Timer, BasicInfo) {
  zx::timer timer;
  ASSERT_OK(zx::timer::create(0, ZX_CLOCK_MONOTONIC, &timer));

  zx_info_handle_basic_t basic_info = {};
  ASSERT_OK(
      timer.get_info(ZX_INFO_HANDLE_BASIC, &basic_info, sizeof(basic_info), nullptr, nullptr));
  EXPECT_EQ(basic_info.type, ZX_OBJ_TYPE_TIMER);
  EXPECT_EQ(basic_info.related_koid, ZX_KOID_INVALID);
  EXPECT_EQ(basic_info.rights, ZX_DEFAULT_TIMER_RIGHTS);
}

}  // anonymous namespace
