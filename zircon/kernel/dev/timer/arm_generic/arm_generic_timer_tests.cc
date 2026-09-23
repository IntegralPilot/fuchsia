// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/affine/ratio.h>
#include <lib/boot-options/boot-options.h>
#include <lib/fit/defer.h>
#include <lib/unittest/unittest.h>

#include <dev/timer/arm_generic.h>
#include <kernel/percpu.h>
#include <kernel/scheduler.h>
#include <kernel/thread.h>
#include <ktl/atomic.h>
#include <ktl/bit.h>  // for popcount? In standard C++ it is <bit>, in ktl it might be different.
#include <ktl/limits.h>
#include <platform/timer.h>  // for ticks_per_second

// We need to declare things that are not in public headers if they are needed.
// ticks_per_second() is in <platform/timer.h>
// timer_get_ticks_to_time_ratio() is in <platform/timer.h>

namespace {

[[maybe_unused]] constexpr uint32_t kMinTestFreq = 1;
[[maybe_unused]] constexpr uint32_t kMaxTestFreq = ktl::numeric_limits<uint32_t>::max();
[[maybe_unused]] constexpr uint32_t kCurTestFreq = 0;

inline uint64_t abs_int64(int64_t a) { return (a > 0) ? a : static_cast<uint64_t>(-a); }

bool test_time_conversion_check_result(uint64_t a, uint64_t b, uint64_t limit) {
  BEGIN_TEST;

  if (a != b) {
    uint64_t diff = abs_int64(a - b);
    ASSERT_LE(diff, limit);
  }

  END_TEST;
}

bool test_time_to_ticks(uint32_t cntfrq) {
  BEGIN_TEST;

  affine::Ratio time_to_ticks;
  if (cntfrq == kCurTestFreq) {
    uint64_t tps = ticks_per_second();
    ASSERT_LE(tps, ktl::numeric_limits<uint32_t>::max());
    cntfrq = static_cast<uint32_t>(tps);
    time_to_ticks = timer_get_ticks_to_time_ratio().Inverse();
  } else {
    time_to_ticks = arm_generic_timer_compute_conversion_factors(cntfrq).Inverse();
  }

  constexpr uint64_t VECTORS[] = {
      0,
      1,
      60 * 60 * 24,
      60 * 60 * 24 * 365,
      60 * 60 * 24 * (365 * 10 + 2),
      60ULL * 60 * 24 * (365 * 100 + 2),
  };

  for (auto vec : VECTORS) {
    uint64_t cntpct = time_to_ticks.Scale(vec);
    constexpr uint32_t nanos_per_sec = ZX_SEC(1);
    uint64_t expected_cntpct = ((uint64_t)cntfrq * vec + (nanos_per_sec / 2)) / nanos_per_sec;

    if (!test_time_conversion_check_result(cntpct, expected_cntpct, 1)) {
      printf("FAIL: zx_time_to_ticks(%" PRIu64 "): got %" PRIu64 ", expect %" PRIu64 "\n", vec,
             cntpct, expected_cntpct);
      ASSERT_TRUE(false);
    }
  }

  END_TEST;
}

bool test_ticks_to_time(uint32_t cntfrq) {
  BEGIN_TEST;

  affine::Ratio ticks_to_time;
  if (cntfrq == kCurTestFreq) {
    uint64_t tps = ticks_per_second();
    ASSERT_LE(tps, ktl::numeric_limits<uint32_t>::max());
    cntfrq = static_cast<uint32_t>(tps);
    ticks_to_time = timer_get_ticks_to_time_ratio();
  } else {
    ticks_to_time = arm_generic_timer_compute_conversion_factors(cntfrq);
  }

  constexpr uint64_t VECTORS[] = {
      1,
      60 * 60 * 24,
      60 * 60 * 24 * 365,
      60 * 60 * 24 * (365 * 10 + 2),
      60ULL * 60 * 24 * (365 * 50 + 2),
  };

  for (auto vec : VECTORS) {
    zx_time_t expected_zx_time = ZX_SEC(vec);
    uint64_t cntpct = (uint64_t)cntfrq * vec;
    zx_time_t zx_time = ticks_to_time.Scale(cntpct);

    const uint64_t limit = (1000 * 1000 + cntfrq - 1) / cntfrq;
    if (!test_time_conversion_check_result(zx_time, expected_zx_time, limit)) {
      printf("ticks_to_zx_time(0x%" PRIx64 "): got 0x%" PRIx64 ", expect 0x%" PRIx64 "\n", cntpct,
             static_cast<uint64_t>(zx_time), static_cast<uint64_t>(expected_zx_time));
      ASSERT_TRUE(false);
    }
  }

  END_TEST;
}

// Verify that the event stream will break CPUs out of WFE.
//
// Start one thread for each CPU that's online and active.  Each thread will then disable
// interrupts and issue a series of WFEs.  If the event stream is working as expected, each thread
// will eventually complete its series of WFEs and terminate.  If the event stream is not working
// as expected, one or more threads will hang.
bool test_event_stream() {
  BEGIN_TEST;

  if (!BootOptions::Get()->arm64_event_stream_enabled) {
    printf("event stream disabled, skipping test\n");
    END_TEST;
  }

  struct Args {
    ktl::atomic<uint32_t> waiting{0};
  };

  auto func = [](void* args_) -> int {
    auto* args = reinterpret_cast<Args*>(args_);
    {
      InterruptDisableGuard guard;

      // Signal that we are ready.
      args->waiting.fetch_sub(1);
      // Wait until everyone else is ready.
      while (args->waiting.load() > 0) {
      }

      // If the event stream is working, it (or something else) will break us out on each iteration.
      for (int i = 0; i < 1000; ++i) {
        // The SEVL sets the event flag for this CPU.  The first WFE consumes the now set event
        // flag.  By setting then consuming, we can be sure the second WFE will actually wait for an
        // event.
        __asm__ volatile("sevl;wfe;wfe");
      }
    }
    printf("cpu-%u done\n", arch_curr_cpu_num());
    return 0;
  };

  Args args;
  Thread* threads[SMP_MAX_CPUS]{};

  // How many online+active CPUs do we have?
  uint32_t num_cpus = ktl::popcount(mp_get_online_mask() & Scheduler::PeekActiveMask());
  args.waiting.store(num_cpus);

  // Create a thread bound to each online+active CPU, but don't start them just yet.
  cpu_num_t last = 0;
  for (cpu_num_t i = 0; i < percpu::processor_count(); ++i) {
    if (mp_is_cpu_online(i) && Scheduler::PeekIsActive(i)) {
      threads[i] = Thread::Create("test_event_stream", func, &args, DEFAULT_PRIORITY);
      threads[i]->SetCpuAffinity(cpu_num_to_mask(i));
      last = i;
    }
  }

  // Because these threads have hard affinity and will disable interrupts we need to take care in
  // how we start them.  If we start one that's bound to our current CPU, we may get preempted
  // deadlock.  To avoid this, bind the current thread to the *last* online+active CPU.
  const cpu_mask_t orig_mask = Thread::Current::Get()->GetCpuAffinity();
  Thread::Current::Get()->SetCpuAffinity(cpu_num_to_mask(last));
  auto restore_mask =
      fit::defer([&orig_mask]() { Thread::Current::Get()->SetCpuAffinity(orig_mask); });

  // Now that we're running on the last online+active CPU we can simply start them in order.
  for (cpu_num_t i = 0; i < percpu::processor_count(); ++i) {
    if (threads[i] != nullptr) {
      threads[i]->Resume();
    }
  }

  // Finally, wait for them to complete.
  for (size_t i = 0; i < percpu::processor_count(); ++i) {
    if (threads[i] != nullptr) {
      threads[i]->Join(nullptr, ZX_TIME_INFINITE);
    }
  }

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(arm_clock_tests)
UNITTEST("Time --> Ticks (min freq)", []() -> bool { return test_time_to_ticks(kMinTestFreq); })
UNITTEST("Time --> Ticks (max freq)", []() -> bool { return test_time_to_ticks(kMaxTestFreq); })
UNITTEST("Time --> Ticks (cur freq)", []() -> bool { return test_time_to_ticks(kCurTestFreq); })
UNITTEST("Ticks --> Time (min freq)", []() -> bool { return test_ticks_to_time(kMinTestFreq); })
UNITTEST("Ticks --> Time (max freq)", []() -> bool { return test_ticks_to_time(kMaxTestFreq); })
UNITTEST("Ticks --> Time (cur freq)", []() -> bool { return test_ticks_to_time(kCurTestFreq); })
UNITTEST("Event Stream", test_event_stream)
UNITTEST_END_TESTCASE(arm_clock_tests, "arm_clock", "Tests for ARM tick count and current time")
