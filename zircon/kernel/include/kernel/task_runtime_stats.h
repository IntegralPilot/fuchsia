// Copyright 2020 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_TASK_RUNTIME_STATS_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_TASK_RUNTIME_STATS_H_

#include <lib/arch/intrin.h>
#include <lib/kconcurrent/copy.h>
#include <lib/kconcurrent/seqlock.h>
#include <lib/relaxed_atomic.h>
#include <zircon/syscalls/object.h>
#include <zircon/time.h>

#include <kernel/lockdep.h>
#include <kernel/scheduler_state.h>
#include <ktl/array.h>

//
// Types and utilities for efficiently accumulating and aggregating task runtime
// stats.
//
// Runtime stats are maintained at three levels: thread, process, and job.
// Threads maintain and update their runtime stats as threads change state.
// Terminating threads roll up to their owning process, and terminating
// processes roll up to their owning job, however queries of process and job
// stats require summing all of the currently running thread stats with the
// rolled up stats for terminated threads.
//
// Per-thread stats are maintained by ThreadRuntimeStats, which provides a
// sequence locked snapshot of the runtime stats with an affordance to
// compensate for unaccounted run-time/queue-time when a thread is in a runnable
// state (i.e. ready or running).
//

// Runtime stats of a thread, process, or job.
//
// Not safe for concurrent use by multiple threads.
struct TaskRuntimeStats {
  // The total duration (in ticks) spent running on a CPU.
  zx_duration_mono_ticks_t cpu_ticks = 0;

  // The total duration (in ticks) spent ready to start running.
  zx_duration_mono_ticks_t queue_ticks = 0;

  // The total duration (in ticks) spent handling page faults.
  zx_duration_mono_ticks_t page_fault_ticks = 0;

  // The total duration (in ticks) spent contented on kernel locks.
  zx_duration_mono_ticks_t lock_contention_ticks = 0;

  // Adds another TaskRuntimeStats to this one.
  constexpr TaskRuntimeStats& operator+=(const TaskRuntimeStats& other) {
    cpu_ticks = zx_ticks_add_ticks(cpu_ticks, other.cpu_ticks);
    queue_ticks = zx_ticks_add_ticks(queue_ticks, other.queue_ticks);
    page_fault_ticks = zx_ticks_add_ticks(page_fault_ticks, other.page_fault_ticks);
    lock_contention_ticks = zx_ticks_add_ticks(lock_contention_ticks, other.lock_contention_ticks);
    return *this;
  }

  // Conversion to zx_info_task_runtime_t.
  operator zx_info_task_runtime_t() const;
};

struct TaskRuntimeStatsTests;  // fwd decl so we can be friends with our tests.

namespace task_runtime_stats::internal {

// Manages sequence locked updates and access to per-thread runtime stats.
class ThreadRuntimeStats {
  static constexpr concurrent::SyncOpt kRuntimeStatsSyncType = concurrent::SyncOpt::Fence;

  template <typename>
  struct LockOption {};

 public:
  struct ThreadStats {
    // When the thread entered its current state.
    zx_duration_mono_ticks_t total_running_ticks = 0;
    zx_duration_mono_ticks_t total_ready_ticks = 0;
    zx_duration_mono_ticks_t state_change_ticks = 0;
    thread_state current_state{THREAD_INITIAL};
  };

  ThreadRuntimeStats() = default;

  // Update must be locked Exclusive with either IrqSave or NoIrqSave.
  static constexpr LockOption<ExclusiveIrqSave> IrqSave{};
  static constexpr LockOption<ExclusiveNoIrqSave> NoIrqSave{};

  // Updates the ThreadStats state with the given deltas and last thread state.
  template <typename ExclusiveOption>
  void Update(thread_state new_state, LockOption<ExclusiveOption>) TA_EXCL(seq_lock_) {
    // Enter our sequence lock and obtain a mutable reference to our payload so
    // we can update the payload in-place.  We are free to read the contents of
    // the payload without the use of any atomics (the SeqLock behaves like a
    // spinlock in this situation, ensures proper ordering and excluding other
    // writes), however we need to make sure to use relaxed atomic stores when
    // writing to the payload (as we may have concurrent reads taking place from
    // observers).
    Guard<SeqLock<kRuntimeStatsSyncType>, ExclusiveOption> thread_guard{&seq_lock_};
    ThreadStats& stats = published_stats_.BeginInPlaceUpdate();

    // Skip the update if we are already in the new state.
    //
    // This can happen when a thread unblocks, and Scheduler::Unblock changes
    // the state of the thread from BLOCKED to READY, and records the transition
    // in the runtime stats in the process. Eventually, we will reschedule on
    // this CPU, and we will record another state transition in the stats
    // (here), but we actually already recorded the stats previously.
    //
    // TODO(johngro): Look into changing this behavior.  It would be better if
    // we only ever called this method when we _knew_ for a fact that we were
    // changing states.
    if (stats.current_state == new_state) {
      return;
    }

    // Make sure that our sampling of the ticks counter takes place between the
    // two stores of the sequence number, and is not allowed to move outside of
    // the update transaction because of pipelined execution.
    const zx_instant_mono_ticks_t now =
        timer_current_mono_ticks_synchronized<GetTicksSyncFlag::kAfterPreviousStores |
                                              GetTicksSyncFlag::kBeforeSubsequentStores>();

    // Now go ahead an update our payload, making sure to use relaxed atomic
    // stores when writing to the contents.
    if (stats.current_state == THREAD_RUNNING) {
      const zx_duration_mono_ticks_t delta = zx_ticks_sub_ticks(now, stats.state_change_ticks);
      ktl::atomic_ref(stats.total_running_ticks)
          .store(zx_ticks_add_ticks(stats.total_running_ticks, delta), ktl::memory_order_relaxed);
    } else if (stats.current_state == THREAD_READY) {
      const zx_duration_mono_ticks_t delta = zx_ticks_sub_ticks(now, stats.state_change_ticks);
      ktl::atomic_ref(stats.total_ready_ticks)
          .store(zx_ticks_add_ticks(stats.total_ready_ticks, delta), ktl::memory_order_relaxed);
    }

    ktl::atomic_ref(stats.state_change_ticks).store(now, ktl::memory_order_relaxed);
    ktl::atomic_ref(stats.current_state).store(new_state, ktl::memory_order_relaxed);
  }

  // Updates the page fault / lock contention ticks with the given deltas. These values do not
  // require relative coherence with other state.
  void AddPageFaultTicks(zx_duration_mono_ticks_t delta) { page_fault_ticks_.fetch_add(delta); }
  void AddLockContentionTicks(zx_duration_mono_ticks_t delta) {
    lock_contention_ticks_.fetch_add(delta);
  }

  // Returns the instantaneous runtime stats for the thread, including the time
  // the thread has spent in its current state (if that state is either READY or
  // RUNNING).
  TaskRuntimeStats GetCompensatedTaskRuntimeStats() const TA_EXCL(seq_lock_) {
    ReadResult res = Read();

    // Adjust for the current time if the thread was in a state that we track
    // when we queried its stats.
    if ((res.stats.current_state == THREAD_RUNNING) || (res.stats.current_state == THREAD_READY)) {
      const zx_duration_mono_ticks_t delta =
          zx_ticks_sub_ticks(res.now, res.stats.state_change_ticks);
      zx_duration_mono_ticks_t& counter = (res.stats.current_state == THREAD_RUNNING)
                                              ? res.stats.total_running_ticks
                                              : res.stats.total_ready_ticks;
      counter = zx_ticks_add_ticks(counter, delta);
    }

    return TaskRuntimeStats{.cpu_ticks = res.stats.total_running_ticks,
                            .queue_ticks = res.stats.total_ready_ticks,
                            .page_fault_ticks = page_fault_ticks_,
                            .lock_contention_ticks = lock_contention_ticks_};
  }

 private:
  friend struct ::TaskRuntimeStatsTests;

  struct ReadResult {
    ThreadStats stats;
    zx_instant_mono_ticks_t now{};
  };

  // Returns a coherent snapshot of the ThreadStats state.
  ReadResult Read() const TA_EXCL(seq_lock_) {
    ReadResult ret;
    for (bool success = false; !success; arch::Yield()) {
      // TODO(johngro): Look into doing this without disabling ints.  Right
      // now, the following is possible:
      //
      // 1) Code enters the lock-guard, "holding" the SeqLock for read.
      // 2) Before exiting, the preemption timer fires.
      // 3) The scheduler selects a new thread during preemption, and needs to
      //    update stats for the old thread/process.
      // 4) It calls into Update which attempts to hold the SeqLock exclusively.
      // 5) Lockdep asserts, because it looks like we are attempting to enter
      //    the same lock class multiple times.
      //
      // Typically, this would be an error, but in the case of a seqlock, it
      // actually isn't.  The read operation (holding the lock with shared
      // access) cannot block the write operation (obtaining the lock
      // exclusively).  The write during the preempt would simply cause the
      // initial read transaction to fail and try again.  Lockdep does not
      // know this however, so it complains.
      //
      // In the short term, we just turn off interrupts during the read.
      // Moving forward, it would be better to teach lockdep about the proper
      // SeqLock semantics, and only have it complain if code attempts to
      // enter the lock exclusively multiple times.
      //
      Guard<SeqLock<kRuntimeStatsSyncType>, SharedIrqSave> guard{&seq_lock_, success};
      // Read the payload.
      published_stats_.Read(ret.stats);

      // Make sure that our sampling of the ticks counter takes place between
      // the two reads of the sequence number, and is not allowed to move
      // outside of the region because of pipelined execution.
      ret.now = timer_current_mono_ticks_synchronized<GetTicksSyncFlag::kAfterPreviousLoads |
                                                      GetTicksSyncFlag::kBeforeSubsequentLoads>();
    }

    return ret;
  }

  // This lock is acquired in the scheduler. Disable reporting to prevent
  // interacting with the debug log and interfering with the last result, which
  // can cause lockdep failures.
  mutable DECLARE_SEQLOCK_EXPLICIT_SYNC(ThreadRuntimeStats, kRuntimeStatsSyncType,
                                        LockFlagsReportingDisabled) seq_lock_;

  SeqLockPayload<ThreadStats, decltype(seq_lock_)> published_stats_ TA_GUARDED(seq_lock_){};
  RelaxedAtomic<zx_duration_mono_ticks_t> page_fault_ticks_{0};
  RelaxedAtomic<zx_duration_mono_ticks_t> lock_contention_ticks_{0};
};

}  // namespace task_runtime_stats::internal

using ThreadRuntimeStats = task_runtime_stats::internal::ThreadRuntimeStats;

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_TASK_RUNTIME_STATS_H_
