// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <zircon/time.h>

#include <gtest/gtest.h>

#include "lib/sched/thread-base.h"
#include "test-thread.h"

namespace {

using Duration = sched::Duration;
using Time = sched::Time;

TEST(ThreadBaseTests, InitialState) {
  TestThread thread{{Period(10), Capacity(5)}, Start(0)};
  EXPECT_EQ(sched::ThreadState::kInitial, thread.state());
}

TEST(ThreadBaseTests, Start) {
  {
    TestThread thread{{Period(10), Capacity(5)}, Start(0)};
    EXPECT_EQ(Time{0}, thread.start());
  }
  {
    TestThread thread{{Period(100), Capacity(10)}, Start(10)};
    EXPECT_EQ(Time{10}, thread.start());
  }
}

TEST(ThreadBaseTests, Finish) {
  {
    TestThread thread{{Period(10), Capacity(5)}, Start(0)};
    EXPECT_EQ(Time{10}, thread.finish());  // start + period
  }
  {
    TestThread thread{{Period(100), Capacity(10)}, Start(10)};
    EXPECT_EQ(Time{110}, thread.finish());  // start + period
  }
}

TEST(ThreadBaseTests, Tick) {
  TestThread thread{{Period(10), Capacity(5)}, Start(0)};
  EXPECT_EQ(Capacity(5), thread.firm_capacity());
  EXPECT_EQ(Duration{0}, thread.time_slice_used());

  thread.Tick(Duration{2});
  EXPECT_EQ(Capacity(5), thread.firm_capacity());
  EXPECT_EQ(Duration{2}, thread.time_slice_used());

  thread.Tick(Duration{3});
  EXPECT_EQ(Capacity(5), thread.firm_capacity());
  EXPECT_EQ(Duration{5}, thread.time_slice_used());
}

TEST(ThreadBaseTests, Reactivate) {
  TestThread thread{{Period(10), Capacity(5)}, Start(0)};
  EXPECT_EQ(Time{0}, thread.start());
  EXPECT_EQ(Time{10}, thread.finish());

  // Reactivation in the current period ([0, 10)) will result in the period
  // being reset as the next one ([10, 20)).
  thread.Reactivate(Start(0));
  EXPECT_EQ(Time{10}, thread.start());
  EXPECT_EQ(Time{20}, thread.finish());

  // Reactivation past the current period ([10, 20)) will result in a new period
  // beginning at that time ([25, 35)).
  thread.Reactivate(Start(25));
  EXPECT_EQ(Time{25}, thread.start());
  EXPECT_EQ(Time{35}, thread.finish());
}

}  // namespace
