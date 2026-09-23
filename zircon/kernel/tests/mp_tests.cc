// Copyright 2025 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <inttypes.h>
#include <lib/unittest/unittest.h>
#include <zircon/time.h>
#include <zircon/types.h>

#include <kernel/auto_preempt_disabler.h>
#include <kernel/cpu.h>
#include <kernel/mp.h>
#include <kernel/thread.h>

#include "tests.h"

#include <ktl/enforce.h>

namespace {

// Verify that mp_reschedule_self() ensures a preemption is pending (almost) immediately after
// re-enabling interrupts.
bool mp_reschedule_self_test() {
  BEGIN_TEST;

  while (true) {
    AutoPreemptDisabler apd;
    const cpu_mask_t local_mask = cpu_num_to_mask(arch_curr_cpu_num());
    {
      InterruptDisableGuard irqd;

      // Now that interrupts are disabled, make sure that we don't already have a preemption
      // pending.  It's possible that another CPU in the system is trying to preempt this CPU.  If
      // that has happened, just try again.
      if (local_mask & Thread::Current::preemption_state().preempts_pending()) {
        continue;
      }

      mp_reschedule_self();
      // We've just IPI'd our CPU.  However, interrupts are disabled so that IPI won't have resulted
      // in a pending preemption just yet.
      ASSERT_EQ(0u, local_mask & Thread::Current::preemption_state().preempts_pending());
    }
    // We've just now re-enabled interrupts.  Either the IPI should have fired or it should fire
    // soon.
    zx_instant_mono_t last = current_mono_time();
    while ((local_mask & Thread::Current::preemption_state().preempts_pending()) == 0) {
      arch::Yield();
      zx_instant_mono_t now = current_mono_time();
      if (now > last + ZX_SEC(5)) {
        printf("still waiting for preemption...\n");
        last = now;
      }
    }

    break;
  }

  END_TEST;
}

// Verify that preemption is disabled during mp_sync_exec'd tasks.
bool mp_sync_exec_preempt_disabled_test() {
  BEGIN_TEST;

  auto test_with_target_mask = [&](mp_ipi_target target, cpu_mask_t mask) -> bool {
    BEGIN_TEST;

    ASSERT_TRUE(Thread::Current::Get()->preemption_state().PreemptIsEnabled());

    bool preempt_enabled = false;
    const mp_sync_task_t task = [](void* context) {
      if (Thread::Current::Get()->preemption_state().PreemptIsEnabled()) {
        *reinterpret_cast<bool*>(context) = true;
      }
    };

    mp_sync_exec(target, mask, task, &preempt_enabled);

    ASSERT_FALSE(preempt_enabled);

    ASSERT_TRUE(Thread::Current::Get()->preemption_state().PreemptIsEnabled());

    END_TEST;
  };

  // 1. ALL
  ASSERT_TRUE(test_with_target_mask(mp_ipi_target::ALL, /* ignored */ 0));
  {
    InterruptDisableGuard irqd;
    ASSERT_TRUE(test_with_target_mask(mp_ipi_target::ALL, /* ignored */ 0));
  }

  // 2. ALL_BUT_LOCAL
  {
    InterruptDisableGuard irqd;
    ASSERT_TRUE(test_with_target_mask(mp_ipi_target::ALL_BUT_LOCAL, /* ignored */ 0));
  }

  // 3. MASK
  ASSERT_TRUE(test_with_target_mask(mp_ipi_target::MASK, CPU_MASK_ALL));
  {
    InterruptDisableGuard irqd;
    ASSERT_TRUE(test_with_target_mask(mp_ipi_target::MASK, CPU_MASK_ALL));
  }

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(mp_tests)
UNITTEST("mp_reschedule_self", mp_reschedule_self_test)
UNITTEST("mp_sync_exec_preempt_disabled", mp_sync_exec_preempt_disabled_test)
UNITTEST_END_TESTCASE(mp_tests, "mp", "tests for mp subsystem")
