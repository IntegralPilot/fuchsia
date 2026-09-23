// Copyright 2026 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/unittest/unittest.h>

#include <kernel/scheduler_state.h>
#include <ktl/limits.h>

namespace {

bool test_packing_logic() {
  BEGIN_TEST;

  // Test case 1: Nominal values (multiple of 10us)
  // Capacity 1ms (100 in 10us units), Deadline 10ms (1000 in 10us units)
  // Expected packed value: -(1001000)
  {
    SchedulerState::BaseProfile profile{SchedDeadlineParams{SchedMs(1), SchedMs(10)}};
    SchedulerState::EffectiveProfile ep{profile};
    int32_t packed = ep.GetWeightOrPackedDeadlineParams();
    EXPECT_EQ(-1001000, packed, "Nominal values packing failed");
  }

  // Test case 2: Unaligned capacity
  // Capacity 1ms + 5us (1,005,000 ns). CCCC should be 100.
  // Deadline 10ms (10,000,000 ns). DDDD should be 1000.
  // Result: -(100 * 10,000 + 1000) = -1,001,000.
  {
    SchedulerState::BaseProfile profile{SchedDeadlineParams{SchedNs(1005000), SchedMs(10)}};
    SchedulerState::EffectiveProfile ep{profile};
    int32_t packed = ep.GetWeightOrPackedDeadlineParams();
    EXPECT_EQ(-1001000, packed, "Unaligned capacity packing failed");
  }

  // Test case 3: Clamping
  // Capacity < 10us -> clamped to 10us (1 unit)
  // Deadline < 10us -> clamped to 1 unit
  // Expected: -(1 * 10,000 + 1) = -10,001
  {
    SchedulerState::BaseProfile profile{SchedDeadlineParams{SchedNs(1000), SchedNs(2000)}};
    SchedulerState::EffectiveProfile ep{profile};
    int32_t packed = ep.GetWeightOrPackedDeadlineParams();
    EXPECT_EQ(-10001, packed, "Clamping small values failed");
  }

  // Test case 4: Max values
  // Capacity 1s. Clamped to 99.99ms (9,999 units)
  // Deadline 1s. Clamped to 9,999 units.
  // Expected: -(9999 * 10,000 + 9999) = -99,999,999
  {
    SchedulerState::BaseProfile profile{SchedDeadlineParams{SchedMs(1000), SchedMs(1000)}};
    SchedulerState::EffectiveProfile ep{profile};
    int32_t packed = ep.GetWeightOrPackedDeadlineParams();
    EXPECT_EQ(-99999999, packed, "Clamping large values failed");
  }

  END_TEST;
}

}  // namespace

UNITTEST_START_TESTCASE(scheduler_state_tests)
UNITTEST("packing_logic", test_packing_logic)
UNITTEST_END_TESTCASE(scheduler_state_tests, "scheduler_state", "SchedulerState tests")
