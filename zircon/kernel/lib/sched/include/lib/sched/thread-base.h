// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_THREAD_BASE_H_
#define ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_THREAD_BASE_H_

#include <zircon/assert.h>
#include <zircon/time.h>

#include <fbl/intrusive_wavl_tree.h>
#include <ffl/fixed.h>

#include "time.h"

namespace sched {

// Forward-declared; defined in <lib/sched/run-queue.h>
template <typename Thread>
class RunQueue;

// Flexible work weight.
//
// Gives a measure of priority for the flexible portion of a thread's work
// (i.e., that done on a best-effort basis all other required/firm work is
// accomplished).
//
// Weights should not be negative; however, the value is signed for consistency
// with Time and Duration, which are the primary types used in conjunction with
// FlexibleWeight. This is to make it less likely that expressions involving
// weights are accidentally promoted to unsigned.
using FlexibleWeight = ffl::Fixed<int64_t, 16>;

// The utilization factor of a thread's desired work, defined as the ratio
// between a thread's capacity (firm, flexible, or total) and its period.
//
// The 20bit fractional component represents the utilization with a precision
// of ~1us.
using Utilization = ffl::Fixed<int64_t, 20>;

// The parameters that specify a thread's activation period (i.e., the
// recurring cycle in which it is scheduled).
struct BandwidthParameters {
  // The duration of the thread's activation period.
  Duration period;

  // The duration within a given activation period in which a thread is expected
  // to complete its *required* work.
  Duration firm_capacity;

  // The weight of the flexible portion of a thread's work. Zero is equivalent
  // to the thread not having flexible work to do, its total capacity being
  // equal to its firm capacity.
  FlexibleWeight flexible_weight;
};

// The scheduling state of a thread.
enum class ThreadState : uint8_t {
  // The thread is in its initialed state and not yet schedulable.
  kInitial,

  // The thread is schedulable and not yet running.
  kReady,

  // The thread is currently running (or selected to be run).
  kRunning,

  // The thread is blocked.
  kBlocked,
};

// The base thread class from which we expect schedulable thread types to
// inherit (following the 'Curiously Recurring Template Pattern'). This class
// manages the scheduler-related thread state of interest to this library.
template <typename Thread>
class ThreadBase {
 public:
  constexpr ThreadBase(BandwidthParameters bandwidth, Time start)
      : period_(bandwidth.period),                //
        firm_capacity_(bandwidth.firm_capacity),  //
        flexible_weight_(bandwidth.flexible_weight) {
    ZX_DEBUG_ASSERT(bandwidth.period >= bandwidth.firm_capacity);
    ZX_DEBUG_ASSERT(bandwidth.period > 0);
    ZX_DEBUG_ASSERT(bandwidth.firm_capacity >= 0);
    ZX_DEBUG_ASSERT(bandwidth.flexible_weight >= 0);
    ZX_DEBUG_ASSERT(bandwidth.firm_capacity != 0 || bandwidth.flexible_weight != 0);
    Reactivate(start);
  }

  constexpr Duration period() const { return period_; }

  constexpr Duration firm_capacity() const { return firm_capacity_; }

  constexpr FlexibleWeight flexible_weight() const { return flexible_weight_; }

  // The total proportion of the period in which the thread is expected to run
  // in order to complete its required work.
  constexpr Utilization firm_utilization() const { return firm_capacity() / period(); }

  // The start of the thread's current activation period.
  constexpr Time start() const { return start_; }

  // The end of the thread's current activation period.
  constexpr Time finish() const { return start_ + period_; }

  // The cumulative amount of scheduled time within the current activation
  // period.
  constexpr Duration time_slice_used() const { return time_slice_used_; }

  // Returns the scheduling state of the thread.
  constexpr ThreadState state() const { return state_; }

  // Sets the scheduling state of the thread.
  void set_state(ThreadState state) { state_ = state; }

  // Whether the thread is active at the provided time (i.e., whether that time
  // falls within the current activation period).
  constexpr bool IsActive(Time now) const { return start() <= now && now < finish(); }

  // Reactivates the thread in a new activation period. If the provided time is
  // before the current period ends, then the new period will naturally begin at
  // the current's finish time; otherwise, the thread missed out on being
  // reactivated along the current period boundary (e.g., due to having been
  // blocked for an extended period of time) and the activation period will be
  // reset starting at `now`.
  constexpr void Reactivate(Time now) {
    start_ = std::max(now, finish());
    time_slice_used_ = Duration{0};
  }

  // Accounts for the time in which the thread was executed (as measured outside
  // this library).
  constexpr void Tick(Duration elapsed) { time_slice_used_ += elapsed; }

  // Whether the thread is currently queued to be scheduled (in a RunQueue).
  constexpr bool IsQueued() const { return run_queue_.ready.InContainer(); }

 private:
  // For access to `run_queue_` alone.
  friend class RunQueue<Thread>;

  Duration period_{0};
  Duration firm_capacity_{0};
  FlexibleWeight flexible_weight_{0};
  Time start_{Time::Min()};
  Duration time_slice_used_{0};

  ThreadState state_ = ThreadState::kInitial;

  // Encapsulates the state specific to the run queue.
  struct RunQueueState {
    // State relating to the tree of ready threads.
    fbl::WAVLTreeNodeState<Thread*> ready;

    // State relating to the tree of threads that are both blocked and in their
    // current activation period.
    fbl::WAVLTreeNodeState<Thread*> actively_blocked;

    // Minimum finish time of all the descendants of this node in the run queue.
    // See RunQueue<Thread>::SubtreeMinFinishObserverTraits for the management
    // and utility of this quantity.
    Time subtree_min_finish{Time::Min()};
  } run_queue_;
};

}  // namespace sched

#endif  // ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_THREAD_BASE_H_
