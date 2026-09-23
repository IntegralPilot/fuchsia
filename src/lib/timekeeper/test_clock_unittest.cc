// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/timekeeper/test_clock.h"

#include <lib/zx/time.h>

#include <gtest/gtest.h>

#include "src/lib/timekeeper/clock.h"

namespace timekeeper {
namespace {

TEST(TestClockTest, SetUtcUpdatesUtcTime) {
  TestClock clock;

  const time_utc expected_utc(42);
  clock.SetUtc(expected_utc);

  time_utc utc;
  ASSERT_EQ(ZX_OK, clock.UtcNow(&utc));
  EXPECT_EQ(expected_utc, utc);

  EXPECT_EQ(zx::time_monotonic(0), clock.MonotonicNow());
  EXPECT_EQ(zx::time_boot(0), clock.BootNow());
}

TEST(TestClockTest, SetMonotonicUpdatesMonotonicTime) {
  TestClock clock;

  const zx::time_monotonic expected_monotonic(42);
  clock.SetMonotonic(expected_monotonic);

  time_utc utc;
  ASSERT_EQ(ZX_OK, clock.UtcNow(&utc));
  EXPECT_EQ(time_utc(0), utc);

  EXPECT_EQ(expected_monotonic, clock.MonotonicNow());
  EXPECT_EQ(zx::time_boot(0), clock.BootNow());
}

TEST(TestClockTest, SetBootUpdatesBootTime) {
  TestClock clock;

  const zx::time_boot expected_boot(42);
  clock.SetBoot(expected_boot);

  time_utc utc;
  ASSERT_EQ(ZX_OK, clock.UtcNow(&utc));
  EXPECT_EQ(time_utc(0), utc);

  EXPECT_EQ(zx::time_monotonic(0), clock.MonotonicNow());
  EXPECT_EQ(expected_boot, clock.BootNow());
}

}  // namespace
}  // namespace timekeeper
