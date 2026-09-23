// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_RUN_QUEUE_H_
#define ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_RUN_QUEUE_H_

#include <zircon/assert.h>

#include <cstdint>
#include <type_traits>
#include <utility>

#include <fbl/intrusive_wavl_tree.h>
#include <fbl/wavl_tree_best_node_observer.h>

#include "thread-base.h"

namespace sched {

// The RunQueue manages the scheduling business logic of its containing threads.
//
// We say a thread is *eligible* at a given time if that time falls within the
// thread's current activation period. At a high level, we try to always
// schedule the currently eligible with the minimal finish time - and it is this
// policy that RunQueue implements.
//
// `Thread` must inherit from `ThreadBase<Thread>`.
template <typename Thread>
class RunQueue {
 private:
  // Forward-declared; defined below.
  struct ReadyNodeTraits;
  struct SubtreeMinFinishObserverTraits;

  using ReadyTree = fbl::WAVLTree<typename ReadyNodeTraits::KeyType, Thread*, ReadyNodeTraits,
                                  fbl::DefaultObjectTag, fbl::SizeOrder::Constant, ReadyNodeTraits,
                                  fbl::WAVLTreeBestNodeObserver<SubtreeMinFinishObserverTraits>>;

 public:
  static_assert(std::is_base_of_v<ThreadBase<Thread>, Thread>);

  ~RunQueue() {
    ready_.clear();
    actively_blocked_.clear();
  }

  // Threads are iterated through in order of start time.
  using iterator = typename ReadyTree::const_iterator;

  iterator begin() const { return ready_.begin(); }
  iterator end() const { return ready_.end(); }

  // Returns the number of queued, ready threads.
  size_t size() const { return ready_.size(); }

  // Whether no threads are currently queued.
  bool empty() const { return ready_.is_empty(); }

  // Returns the currently scheduled thread, if there is one.
  const Thread* current_thread() const { return current_; }

  // The total duration within the thread's activation period in which the
  // thread is expected to complete its firm and flexible work . This can be
  // regarded as a measure of the expected worst-case runtime given current
  // bandwidth subscription.
  Duration CapacityOf(const Thread& thread) const {
    return thread.firm_capacity() + FlexibleCapacityOf(thread);
  }

  // Given current bandwidth subscription, the duration of time we expect to be
  // allotted to a thread's flexible work within its period.
  Duration FlexibleCapacityOf(const Thread& thread) const {
    return (Utilization{1} - total_firm_utilization()) * FlexibleUtilizationOf(thread) *
           thread.period();
  }

  // The remaining time expected for the thread to be scheduled within its
  // current activation period.
  Duration TimesliceRemainingOn(const Thread& thread) const {
    return CapacityOf(thread) - thread.time_slice_used();
  }

  // Whether the thread has completed its work for the current activation
  // period or activation period itself has ended.
  bool IsExpired(const Thread& thread, Time now) const {
    return TimesliceRemainingOn(thread) <= 0 || now >= thread.finish();
  }

  // Queues the provided thread, given the current time (so as to ensure that
  // the thread is or will be active).
  void Queue(Thread& thread, Time now) {
    ZX_DEBUG_ASSERT(!thread.IsQueued());

    // Account for demand ahead of the IsExpired() check, as that is dependent
    // on this quantity.
    if (InActivelyBlockedTree(thread)) {
      DequeueActivelyBlocked(thread);
    }
    ready_and_actively_blocked_firm_utilization_ += thread.firm_utilization();
    ready_flexible_demand_ += thread.flexible_weight();

    if (IsExpired(thread, now)) {
      thread.Reactivate(now);
    }
    ready_.insert(&thread);
    thread.set_state(ThreadState::kReady);
  }

  // Dequeues the thread from the run queue (provided the thread was already
  // contained).
  void Dequeue(Thread& thread) {
    ZX_DEBUG_ASSERT(thread.IsQueued());
    ZX_DEBUG_ASSERT(thread.state() == ThreadState::kReady);
    ready_.erase(thread);
    ready_and_actively_blocked_firm_utilization_ -= thread.firm_utilization();
    ready_flexible_demand_ -= thread.flexible_weight();
  }

  struct SelectNextThreadResult {
    Thread* next = nullptr;
    Time preemption_time = Time::Min();
  };

  // Selects the next thread to run and also gives the time at which the
  // preemption timer should fire for the subsequent round of scheduling.
  //
  // See //zircon/kernel/lib/sched/README.md#thread-selection for more detail on
  // the behavior of this method.
  SelectNextThreadResult SelectNextThread(Time now) {
    // Reaccount for blocked threads and their impacts on bandwidth before
    // further bandwidth-dependent decisions are made: tracked blocked threads
    // outside the period in which they blocked are dropped, reducing firm
    // utilization.
    while (!actively_blocked_.is_empty() && actively_blocked_.front().finish() <= now) {
      DequeueActivelyBlocked(actively_blocked_.front());
    }

    // Ensure current_ is up-to-date:
    // * If it is now blocked, track it as such if it is still in its current
    //   active period (retaining its firm utilization), and unset it as the
    //   currently running.
    // * If it is now expired, reactivate it so that its true activation period
    //   can factor into the next round of scheduling decisions.
    if (current_) {
      if (current_->state() == ThreadState::kBlocked) {
        if (current_->IsActive(now)) {
          QueueActivelyBlocked(*current_);
        }
        current_ = nullptr;
      } else if (IsExpired(*current_, now)) {
        current_->Reactivate(now);
      }
    }

    // The next eligible might actually be expired (e.g., due to bandwidth
    // oversubscription), in which case it should be reactivated and
    // requeued, and our search should begin again for an eligible thread
    // still in its current period.
    Thread* next = FindNextEligibleThread(now).CopyPointer();
    while (next && now >= next->finish()) {
      Dequeue(*next);
      Queue(*next, now);
      next = FindNextEligibleThread(now).CopyPointer();
    }

    // Now select the next thread.
    //
    // Try to avoid rebalancing (from tree insertion and deletion) in the case
    // where the next thread is the current one.
    if (current_ && current_->IsActive(now) &&
        (!next || SchedulesBeforeIfActive(*current_, *next))) {
      next = current_;
    } else {
      if (next) {
        Dequeue(*next);
        next->set_state(ThreadState::kRunning);
      }
      if (current_) {
        Queue(*current_, now);
      }
      current_ = next;
    }

    Time preemption;
    if (next) {
      Time next_completion = std::min<Time>(now + TimesliceRemainingOn(*next), next->finish());
      ZX_DEBUG_ASSERT(TimesliceRemainingOn(*next) > 0);
      ZX_DEBUG_ASSERT(now < next->finish());

      // Check if there is a thread with an earlier finish that will become
      // eligible before `next` finishes.
      if (auto it = FindNextEligibleThread(next_completion); it && it->finish() < next->finish()) {
        preemption = std::min<Time>(next_completion, it->start());
      } else {
        preemption = next_completion;
      }
    } else {
      // If there is nothing currently eligible, we should preempt next when
      // there is.
      preemption = empty() ? Time::Max() : begin()->start();
    }

    // Also factor in when the next actively blocked thread - if any - will
    // finish its period so that its firm utilization can be dropped as early as
    // possible. At this point in the routine (courtesy of the popping at the
    // top) any threads in actively_blocked_ are ensured to be active.
    if (!actively_blocked_.is_empty()) {
      preemption = std::min(preemption, actively_blocked_.front().finish());
    }

    ZX_DEBUG_ASSERT((!next && preemption == Time::Max()) || preemption > now);
    return {next, preemption};
  }

 private:
  using mutable_iterator = typename ReadyTree::iterator;

  // Implements both the WAVLTree key and node traits for the tree of ready
  // threads.
  struct ReadyNodeTraits {
    // Start time, along with the address of the thread as a convenient
    // tie-breaker.
    using KeyType = std::pair<Time, uintptr_t>;

    static KeyType GetKey(const Thread& thread) {
      return std::make_pair(thread.start(), reinterpret_cast<uintptr_t>(&thread));
    }

    static bool LessThan(KeyType a, KeyType b) { return a < b; }

    static bool EqualTo(KeyType a, KeyType b) { return a == b; }

    static auto& node_state(Thread& thread) { return thread.run_queue_.ready; }
  };

  // Implements with traits for the WAVLTreeBestNodeObserver, which will
  // automatically manage a subtree's minimum finish time on insertion and
  // deletion.
  //
  // The value is used to perform a partition search in O(log n) time, to find
  // the thread with the earliest finish time that also has an eligible start
  // time. See FindNextEligibleThread().
  struct SubtreeMinFinishObserverTraits {
    static Time GetValue(const Thread& thread) { return thread.finish(); }

    static Time GetSubtreeBest(const Thread& thread) {
      return thread.run_queue_.subtree_min_finish;
    }

    static bool Compare(Time a, Time b) { return a < b; }

    static void AssignBest(Thread& thread, Time val) { thread.run_queue_.subtree_min_finish = val; }

    static void ResetBest(Thread& target) {}
  };

  // Implements both the WAVLTree key and node traits for the tree of 'currently
  // blocked' threads. See actively_blocked_ below.
  struct ActivelyBlockedNodeTraits {
    // Finish time, along with the address of the thread as a convenient
    // tie-breaker.
    using KeyType = std::pair<Time, uintptr_t>;

    static KeyType GetKey(const Thread& thread) {
      return std::make_pair(thread.finish(), reinterpret_cast<uintptr_t>(&thread));
    }

    static bool LessThan(KeyType a, KeyType b) { return a < b; }

    static bool EqualTo(KeyType a, KeyType b) { return a == b; }

    // We reuse the same node state as the ReadyTree, as a node cannot be in
    // both trees at once.
    static auto& node_state(Thread& thread) { return thread.run_queue_.actively_blocked; }
  };

  // See actively_blocked_ below.
  using ActivelyBlockedTree =
      fbl::WAVLTree<typename ActivelyBlockedNodeTraits::KeyType, Thread*, ActivelyBlockedNodeTraits,
                    fbl::DefaultObjectTag, fbl::SizeOrder::Constant, ActivelyBlockedNodeTraits>;

  // Provided both threads are active, this gives whether the first should be
  // scheduled before the second, which is a simple lexicographic order on
  // (finish, start, address). (The address is a guaranteed and convenient final
  // tiebreaker which should never amount to a consistent bias.)
  //
  // This comparison is only valid if the given threads are active. It is the
  // caller's responsibility to ensure that this is the case.
  static constexpr bool SchedulesBeforeIfActive(const Thread& a, const Thread& b) {
    return std::make_tuple(a.finish(), a.start(), &a) < std::make_tuple(b.finish(), b.start(), &b);
  }

  // Shorthand for convenience and readability.
  static constexpr Time SubtreeMinFinish(mutable_iterator it) {
    return it->run_queue_.subtree_min_finish;
  }

  // Whether a thread is currently in currently in actively_blocked_.
  static constexpr bool InActivelyBlockedTree(const Thread& thread) {
    return thread.run_queue_.actively_blocked.InContainer();
  }

  FlexibleWeight total_flexible_demand() const {
    return (current_ ? current_->flexible_weight() : FlexibleWeight{0}) + ready_flexible_demand_;
  }

  Utilization total_firm_utilization() const {
    Utilization utilization = (current_ ? current_->firm_utilization() : Utilization{0}) +
                              ready_and_actively_blocked_firm_utilization_;
    // Clamp to account for oversubscription.
    return std::min(Utilization{1}, utilization);
  }

  Utilization FlexibleUtilizationOf(const Thread& thread) const {
    FlexibleWeight demand = total_flexible_demand();
    return demand == 0 ? Utilization{0} : thread.flexible_weight() / demand;
  }

  // Returns the thread eligible to scheduled at a given time with the minimal
  // finish time.
  mutable_iterator FindNextEligibleThread(Time time) {
    if (ready_.is_empty() || ready_.front().start() > time) {
      return ready_.end();
    }

    // `node` will follow a search path that partitions the tree into eligible
    // tasks, iterating to the subtree of eligible start times and then cleaving
    // to the right. `path` will track the minimum finish time encountered along
    // the search path, while `subtree` will track the subtree with minimum
    // finish time off of the search path. After the search, we will be able to
    // check `path` against `subtree` to determine where the true minimal,
    // eligible finish lies.
    mutable_iterator node = ready_.root();
    mutable_iterator path = ready_.end();
    mutable_iterator subtree = ready_.end();
    while (node) {
      // Iterate to the subtree of eligible start times.
      if (node->start() > time) {
        node = node.left();
        continue;
      }

      // Earlier finish found on path: update `path`.
      if (!path || path->finish() > node->finish()) {
        path = node;
      }

      // Earlier finish found off path: update `subtree`.
      {
        auto left = node.left();
        if (!subtree || (left && SubtreeMinFinish(subtree) > SubtreeMinFinish(left))) {
          subtree = left;
        }
      }

      node = node.right();
    }

    // Check if the minimum eligible finish was found along the search path. If
    // there is an identical finish time in the subtree, respect the documented
    // tiebreaker policy and go with the subtree's thread.
    if (!subtree || SubtreeMinFinish(subtree) > path->finish()) {
      return path;
    }

    // Else, the minimum eligible finish must exist in `subtree`.
    for (node = subtree; node->finish() != SubtreeMinFinish(subtree);) {
      if (auto left = node.left(); left && SubtreeMinFinish(node) == SubtreeMinFinish(left)) {
        node = left;
      } else {
        node = node.right();
      }
    }
    return node;
  }

  void QueueActivelyBlocked(Thread& thread) {
    ZX_DEBUG_ASSERT(thread.state() == ThreadState::kBlocked);
    ZX_DEBUG_ASSERT(!InActivelyBlockedTree(thread));

    // No point in tracking the thread if it does not contribute firm demand.
    if (thread.firm_capacity() > 0) {
      actively_blocked_.insert(&thread);
      ready_and_actively_blocked_firm_utilization_ += thread.firm_utilization();
    }
  }

  void DequeueActivelyBlocked(Thread& thread) {
    ZX_DEBUG_ASSERT(InActivelyBlockedTree(thread));
    actively_blocked_.erase(thread);
    ready_and_actively_blocked_firm_utilization_ -= thread.firm_utilization();
  }

  // The thread currently selected to be run.
  Thread* current_ = nullptr;

  // The tree of threads ready to be run.
  ReadyTree ready_;

  // The tree of threads (with firm work to do) that were last seen to blocked
  // within their active periods, ordered by finish time.
  ActivelyBlockedTree actively_blocked_;

  // The aggregate flexible weight across all ready threads.
  FlexibleWeight ready_flexible_demand_{0};

  // The aggregate firm utilization across all ready and actively blocked
  // threads.
  Utilization ready_and_actively_blocked_firm_utilization_{0};
};

}  // namespace sched

#endif  // ZIRCON_KERNEL_LIB_SCHED_INCLUDE_LIB_SCHED_RUN_QUEUE_H_
