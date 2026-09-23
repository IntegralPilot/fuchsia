// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <time.h>

#include <perftest/perftest.h>

namespace {

// Performance test for clock_gettime()+CLOCK_MONOTONIC.  This is the
// main standard timer interface with nanosecond resolution on POSIX
// systems, including Linux.  This interface is worth testing because
// it is commonly used outside of Fuchsia.
bool ClockGettimeMonotonic() {
  timespec ts;
  ZX_ASSERT(clock_gettime(CLOCK_MONOTONIC, &ts) == 0);
  perftest::DoNotOptimize(&ts);
  return true;
}

// Fuchsia's code path for obtaining the real time (a.k.a. UTC time) under
// Starnix is somewhat nontrivial, so it's worth testing and tracking
// its performance, as it sees heavy use in filesystem code. Even under
// Fuchsia proper, we might gain useful insights from this code path.
bool ClockGettimeRealTime() {
  timespec ts;
  ZX_ASSERT(clock_gettime(CLOCK_REALTIME, &ts) == 0);
  perftest::DoNotOptimize(&ts);
  return true;
}

void RegisterTests() {
  perftest::RegisterSimpleTest<ClockGettimeMonotonic>("ClockGettimeMonotonic");
  perftest::RegisterSimpleTest<ClockGettimeRealTime>("ClockGettimeRealTime");
}
PERFTEST_CTOR(RegisterTests)

}  // namespace
