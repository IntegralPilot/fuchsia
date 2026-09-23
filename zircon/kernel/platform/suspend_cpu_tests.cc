// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <assert.h>
#include <lib/unittest/unittest.h>
#include <platform.h>
#include <zircon/errors.h>

#include <arch/interrupt.h>
#include <kernel/auto_preempt_disabler.h>
#include <kernel/mp.h>
#include <kernel/scheduler.h>

namespace {

bool pending_ipi() {
  BEGIN_TEST;

  if (!platform_supports_suspend_cpu()) {
    printf("platform_suspend_cpu is not supported, skipping test\n");
    END_TEST;
  }

  {
    AutoPreemptDisabler apd;
    InterruptDisableGuard irqd;
    const cpu_num_t curr_cpu = arch_curr_cpu_num();

    // Pend an interrupt to ourselves to ensure that platform_suspend_cpu returns quickly.
    mp_interrupt(mp_ipi_target::MASK, cpu_num_to_mask(curr_cpu));

    const zx_status_t status = platform_suspend_cpu(PlatformAllowDomainPowerDown::No);
    ASSERT_OK(status);
  }

  END_TEST;
}

// This test creates two threads, A and B.  Each on their own CPU.  A will suspend and wait to be
// woken by B.  B will wait to observe that A has suspended before waking it via IPI.  Because A
// might wake from suspend due to a stray interrupt, we retry the test until it succeeds, an error
// occurs or the timeout has elapsed.
bool one_wakes_another() {
  BEGIN_TEST;

  constexpr zx_duration_boot_t kTimeout = ZX_SEC(10);

  if (!platform_supports_suspend_cpu()) {
    printf("platform_suspend_cpu is not supported, skipping test\n");
    END_TEST;
  }

  cpu_mask_t active_cpus = Scheduler::PeekActiveMask();
  if (ktl::popcount(active_cpus) <= 1) {
    printf("not enough active CPUs, skipping test\n");
    END_TEST;
  }

  enum class State : uint32_t {
    READY = 0,
    SUSPENDING,
    DONE,
  };
  struct Arg {
    const cpu_num_t target;
    ktl::atomic<State> target_state;
  };

  thread_start_routine a_entry = [](void* arg_) -> int {
    auto* arg = reinterpret_cast<Arg*>(arg_);

    AutoPreemptDisabler apd;
    InterruptDisableGuard irqd;

    arg->target_state.store(State::SUSPENDING, ktl::memory_order_release);
    const zx_status_t status = platform_suspend_cpu(PlatformAllowDomainPowerDown::No);
    arg->target_state.store(State::DONE, ktl::memory_order_release);

    return status;
  };

  thread_start_routine b_entry = [](void* arg_) -> int {
    auto* arg = reinterpret_cast<Arg*>(arg_);

    // Wait until the target reports that it's about to suspend.
    State reported;
    while ((reported = arg->target_state.load(ktl::memory_order_acquire)) == State::READY) {
      arch::Yield();
    }

    if (reported == State::SUSPENDING) {
      // Wait a short amount of time, see that the thread is still suspended, then wake it.
      Thread::Current::SleepRelative(ZX_USEC(200));
      if (arg->target_state.load(ktl::memory_order_acquire) == State::SUSPENDING) {
        mp_interrupt(mp_ipi_target::MASK, cpu_num_to_mask(arg->target));
        return ZX_OK;
      }
    }

    // Somehow the CPU must have returned from suspend.
    return ZX_ERR_BAD_STATE;
  };

  // First, select the CPU that will wake the other.  Choose the boot CPU because we know it's must
  // be active and because we want the suspending CPU to be one of the secondaries since they
  // receive fewer (if any) hardware interrupts (SPIs).
  ASSERT_TRUE(active_cpus & cpu_num_to_mask(BOOT_CPU_ID));
  const cpu_num_t cpu_b = BOOT_CPU_ID;
  active_cpus &= ~cpu_num_to_mask(BOOT_CPU_ID);
  const cpu_num_t cpu_a = remove_cpu_from_mask(active_cpus);

  const auto deadline = Deadline::after_boot(kTimeout);
  for (size_t attempt = 0; current_boot_time() < deadline.when(); ++attempt) {
    printf("attempt %zu\n", attempt);

    Arg arg(cpu_a, State::READY);

    Thread* a = Thread::Create("one_wakes_another-a", a_entry, &arg, DEFAULT_PRIORITY);
    ASSERT_NONNULL(a);
    a->SetCpuAffinity(cpu_num_to_mask(cpu_a));
    a->Resume();

    Thread* b = Thread::Create("one_wakes_another-b", b_entry, &arg, DEFAULT_PRIORITY);
    ASSERT_NONNULL(a);
    b->SetCpuAffinity(cpu_num_to_mask(cpu_b));
    b->Resume();

    zx_status_t a_status;
    a->Join(&a_status, ZX_TIME_INFINITE);
    ASSERT_OK(a_status);

    zx_status_t b_status;
    b->Join(&b_status, ZX_TIME_INFINITE);

    // Perhaps CPU-A woke from suspend early.  Try again.
    if (b_status == ZX_ERR_BAD_STATE) {
      continue;
    }

    ASSERT_OK(b_status);
    END_TEST;
  }

  printf("failed to complete successfully before timeout\n");
  ASSERT_TRUE(false);

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(platform_suspend_cpu_tests)
UNITTEST("pending_ipi", pending_ipi)
UNITTEST("one_wakes_another", one_wakes_another)
UNITTEST_END_TESTCASE(platform_suspend_cpu_tests, "platform_suspend_cpu_tests",
                      "platform_suspend_cpu tests")
