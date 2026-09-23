// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>
#include <lib/unittest/unittest.h>
#include <zircon/compiler.h>

#include <kernel/scheduler_state.h>
#include <kernel/thread.h>
#include <ktl/algorithm.h>
#include <ktl/array.h>
#include <ktl/unique_ptr.h>

#include "tests.h"

#include <ktl/enforce.h>

struct WaitQueueOrderingTests {
 public:
  // Note that we disable static thread analysis for this test.  Typically,
  // working with the internal state of threads and wait queues requires holding
  // particular locks at particular times in order to guarantee consistency and
  // proper memory ordering semantics.
  //
  // In these tests, however, are threads and wait queue collections are
  // basically fake.  The threads are only ever partially initialized, and are
  // never run or made available to be scheduled.  Neither the "threads" nor the
  // "wqc" will ever be exposed outside of the test, and since
  // inserting/removing/peeking WQCs never interacts with any global state, it
  // should be fine to disable static analysis for this test.
  static bool Test() __TA_NO_THREAD_SAFETY_ANALYSIS {
    BEGIN_TEST;

    // Set up the things we will need to run our basic tests.  We need a few
    // Thread structures (although we don't need or even want them to ever run),
    // and a WaitQueueCollection (encapsulated by WaitQueue, this is the object
    // which determines the wake order)
    fbl::AllocChecker ac;
    constexpr size_t kThreadCount = 4;
    ktl::array<ktl::unique_ptr<Thread>, kThreadCount> threads;

    for (size_t i = 0; i < ktl::size(threads); ++i) {
      threads[i].reset(new (&ac) Thread{});
      ASSERT_TRUE(ac.check());
    }

    ktl::unique_ptr<WaitQueueCollection> wqc{new (&ac) WaitQueueCollection{}};
    ASSERT_TRUE(ac.check());
    auto cleanup = fit::defer([&wqc]() __TA_NO_THREAD_SAFETY_ANALYSIS {
      while (wqc->Count() > 0) {
        wqc->Remove(wqc->PeekFront());
      }
    });

    // Sort the thread array by address to simplify tests that involve address
    // based tie breaking.
    ktl::stable_sort(threads.begin(), threads.end(),
                     [](const auto& a, const auto& b) { return a.get() < b.get(); });

    // Aliases to reduce the typing just a bit.
    Thread& t0 = *threads[0];
    Thread& t1 = *threads[1];
    Thread& t2 = *threads[2];
    Thread& t3 = *threads[3];

    SchedTime now{ZX_SEC(300)};

    // The wait queue is empty.
    ASSERT_NULL(wqc->PeekFront());

    // Add a fair thread to the collection, which is at the front of the queue
    // by definition.
    ResetFair(t0, kDefaultWeight, now);
    wqc->Insert(&t0);
    ASSERT_EQ(&t0, wqc->PeekFront());

    // Add a higher weight thread with the same start time to the collection.
    // The thread selected should have a lower address, since the finish times
    // are the same.
    ResetFair(t1, kHighWeight, now);
    wqc->Insert(&t1);
    ASSERT_EQ(&t0, wqc->PeekFront());

    // Reduce the start and finish times of the thread we just added and try
    // again. The thread with the earlier finish time should be selected.
    wqc->Remove(&t1);
    ResetFair(t1, kLowWeight, now - SchedNs(1));
    wqc->Insert(&t1);
    ASSERT_EQ(&t1, wqc->PeekFront());

    // Add a deadline thread whose absolute deadline is far in the future.
    // The thread with the earliest finish time should be selected.
    ResetDeadline(t2, kLongDeadline, now);
    wqc->Insert(&t2);
    ASSERT_EQ(&t1, wqc->PeekFront());

    // Add another deadline thread with a shorter relative deadline. This
    // should become the new choice.
    ResetDeadline(t3, kShortDeadline, now);
    wqc->Insert(&t3);
    ASSERT_EQ(&t3, wqc->PeekFront());

    // Finally, unwind by "unblocking" all of the threads from the queue and
    // making sure that the come out in the order we expect.
    ktl::array expected_order{&t3, &t1, &t0, &t2};
    for (Thread* expected_thread : expected_order) {
      Thread* actual_thread = wqc->PeekFront();
      ASSERT_EQ(expected_thread, actual_thread);
      wqc->Remove(actual_thread);
    }
    // And the queue should finally be empty now.
    ASSERT_NULL(wqc->PeekFront());

    END_TEST;
  }

 private:
  static constexpr SchedWeight kLowWeight = SchedWeight{10};
  static constexpr SchedWeight kDefaultWeight = SchedWeight{20};
  static constexpr SchedWeight kHighWeight = SchedWeight{40};

  static constexpr SchedDuration kShortDeadline = SchedUs(500);
  static constexpr SchedDuration kLongDeadline = SchedMs(20);

  static void ResetFair(Thread& t, SchedWeight weight,
                        SchedTime start_time) __TA_NO_THREAD_SAFETY_ANALYSIS {
    SchedulerState& ss = t.scheduler_state();

    ss.base_profile_.fair.weight = weight;
    ss.base_profile_.discipline = SchedDiscipline::Fair;

    ss.effective_profile_.SetFair(ss.base_profile_.fair.weight);

    ss.start_time_ = start_time;

    ss.finish_time_ = start_time + SchedDefaultFairPeriod;
    ss.time_slice_ns_ = SchedDefaultFairPeriod;
    ss.time_slice_used_ns_ = SchedDuration{0};
  }

  static void ResetDeadline(Thread& t, SchedDuration rel_deadline,
                            SchedTime start_time) __TA_NO_THREAD_SAFETY_ANALYSIS {
    SchedulerState& ss = t.scheduler_state();

    // Just use 20% for all of our utilizations.  It does not really matter what
    // we pick as our utilization/capacity/timeslice-remaining should not factor
    // into queue ordering right now.
    constexpr SchedUtilization kUtil = SchedUtilization{1} / 5;
    const SchedDuration capacity = kUtil * rel_deadline;
    ss.base_profile_.discipline = SchedDiscipline::Deadline;
    ss.base_profile_.deadline = SchedDeadlineParams{capacity, rel_deadline};

    ss.effective_profile_.SetDeadline(ss.base_profile_.deadline, ss.base_profile_.critical);

    ss.start_time_ = start_time;
    ss.time_slice_ns_ = ss.effective_profile().deadline().capacity_ns;
    ss.time_slice_used_ns_ = SchedDuration{0};
    ss.finish_time_ = ss.start_time_ + ss.effective_profile_.deadline().deadline_ns;
  }
};

UNITTEST_START_TESTCASE(wq_order_tests)
UNITTEST("basic", WaitQueueOrderingTests::Test)
UNITTEST_END_TESTCASE(wq_order_tests, "wq_order", "WaitQueue ordering tests")
