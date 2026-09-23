// Copyright 2018 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/unittest/unittest.h>

#include <arch/interrupt.h>
#include <arch/ops.h>

#ifdef __aarch64__
static bool arm64_interrupt_mask_test() {
  BEGIN_TEST;
  const bool initial_fiq_mask = arch_fiqs_disabled();
  bool outer_irq, outer_fiq, inner_irq, inner_fiq, restored_irq, restored_fiq;
  {
    InterruptDisableGuard outer;
    outer_irq = arch_ints_disabled();
    outer_fiq = arch_fiqs_disabled();
    {
      InterruptDisableGuard inner;
      inner_irq = arch_ints_disabled();
      inner_fiq = arch_fiqs_disabled();
    }
    restored_irq = arch_ints_disabled();
    restored_fiq = arch_fiqs_disabled();
  }
  EXPECT_TRUE(outer_irq && inner_irq && restored_irq);
#ifdef EXPERIMENTAL_APPLE
  EXPECT_FALSE(initial_fiq_mask);
  EXPECT_TRUE(outer_fiq && inner_fiq && restored_fiq);
#else
  EXPECT_EQ(outer_fiq, initial_fiq_mask);
  EXPECT_EQ(inner_fiq, initial_fiq_mask);
  EXPECT_EQ(restored_fiq, initial_fiq_mask);
#endif
  EXPECT_FALSE(arch_ints_disabled());
  EXPECT_EQ(arch_fiqs_disabled(), initial_fiq_mask);
  END_TEST;
}
#endif

static bool interrupt_disable_test() {
  BEGIN_TEST;

  // make sure ints are disabled and that a simple enable/disable tracks
  ASSERT_EQ(false, arch_ints_disabled());
  arch_disable_ints();
  ASSERT_EQ(true, arch_ints_disabled());
  arch_enable_ints();
  ASSERT_EQ(false, arch_ints_disabled());

  END_TEST;
}

static bool interrupt_save_restore_test() {
  BEGIN_TEST;

  // validate that a simple save/restore works
  {
    ASSERT_EQ(false, arch_ints_disabled());
    interrupt_saved_state_t state = arch_interrupt_save();
    ASSERT_EQ(true, arch_ints_disabled());
    arch_interrupt_restore(state);
    ASSERT_EQ(false, arch_ints_disabled());
  }

  // validate that a nested save/restore works
  {
    ASSERT_EQ(false, arch_ints_disabled());
    interrupt_saved_state_t state = arch_interrupt_save();
    ASSERT_EQ(true, arch_ints_disabled());
    interrupt_saved_state_t state2 = arch_interrupt_save();
    ASSERT_EQ(true, arch_ints_disabled());
    arch_interrupt_restore(state2);
    ASSERT_EQ(true, arch_ints_disabled());
    arch_interrupt_restore(state);
    ASSERT_EQ(false, arch_ints_disabled());
  }

  END_TEST;
}

static bool interrupt_save_restore_guard_test() {
  BEGIN_TEST;

  // validate that a save/restore C++ guard works
  ASSERT_EQ(false, arch_ints_disabled());
  {
    InterruptDisableGuard irqd;
    ASSERT_EQ(true, arch_ints_disabled());
  }
  ASSERT_EQ(false, arch_ints_disabled());

  // validate that a nested guard works
  {
    InterruptDisableGuard irqd;
    ASSERT_EQ(true, arch_ints_disabled());
    {
      InterruptDisableGuard irqd2;
      ASSERT_EQ(true, arch_ints_disabled());
    }
    ASSERT_EQ(true, arch_ints_disabled());
  }
  ASSERT_EQ(false, arch_ints_disabled());

  // validate that reenable works
  {
    InterruptDisableGuard irqd;
    ASSERT_EQ(true, arch_ints_disabled());
    irqd.Reenable();
    ASSERT_EQ(false, arch_ints_disabled());
    irqd.Reenable();
    ASSERT_EQ(false, arch_ints_disabled());
  }
  ASSERT_EQ(false, arch_ints_disabled());

  // validate that nested reenable works
  {
    InterruptDisableGuard irqd;
    ASSERT_EQ(true, arch_ints_disabled());
    {
      InterruptDisableGuard irqd2;
      ASSERT_EQ(true, arch_ints_disabled());
      irqd2.Reenable();
      ASSERT_EQ(true, arch_ints_disabled());
      irqd2.Reenable();
      ASSERT_EQ(true, arch_ints_disabled());
    }
    ASSERT_EQ(true, arch_ints_disabled());
  }
  ASSERT_EQ(false, arch_ints_disabled());

  END_TEST;
}

UNITTEST_START_TESTCASE(interrupt_disable_tests)
#ifdef __aarch64__
UNITTEST("ARM64 IRQ/FIQ mask", arm64_interrupt_mask_test)
#endif
UNITTEST("interrupt_disable_test", interrupt_disable_test)
UNITTEST("interrupt_save_restore_test", interrupt_save_restore_test)
UNITTEST("interrupt_save_restore_guard_test", interrupt_save_restore_guard_test)
UNITTEST_END_TESTCASE(interrupt_disable_tests, "interrupt_tests", "Test arch enable/disable interrupt routines.")
