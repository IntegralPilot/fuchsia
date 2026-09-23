// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_LIB_SCHED_TEST_THREAD_H_
#define ZIRCON_KERNEL_LIB_SCHED_TEST_THREAD_H_

#include <lib/sched/thread-base.h>
#include <zircon/time.h>

// Utilities for readability's sake: using these inlined calls instead of raw
// literals adds implicit documentation and avoids the reader confusing, say,
// what a given duration among a soup of others represents.
constexpr sched::Duration Capacity(zx_duration_mono_t capacity) {
  return sched::Duration{capacity};
}
constexpr sched::Duration Period(zx_duration_mono_t period) { return sched::Duration{period}; }
constexpr sched::Time Start(zx_instant_mono_t time) { return sched::Time{time}; }
constexpr sched::Time Finish(zx_instant_mono_t time) { return sched::Time{time}; }

struct TestThread : public sched::ThreadBase<TestThread> {
  using sched::ThreadBase<TestThread>::ThreadBase;
};

#endif  // ZIRCON_KERNEL_LIB_SCHED_TEST_THREAD_H_
