// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include <lib/fit/defer.h>

#include <kernel/owned_wait_queue.h>
#include <kernel/scheduler.h>
#include <kernel/scheduler_inline.h>
#include <kernel/scheduler_state.h>
#include <kernel/scheduler_tracing.h>
#include <ktl/algorithm.h>
#include <ktl/type_traits.h>

// Pi is an inner class of Scheduler that provides access to scheduler state
// while hiding the implementation details from the main scheduler header. Its
// primary jobs are:
//
// 1) Provide accessors abstract the distinction between a thread and an owned
//    wait queue when working with templated methods who operate on an upstream
//    type and a target type, where each type might be either a Thread or an
//    OwnedWaitQueue.
// 2) Implement the common PI handler responsible for obtaining the proper
//    locks, and removing/re-inserting a target from/to its container while
//    updating the targets dynamic scheduling parameters.
struct Scheduler::Pi {
  static void AssertEpDirtyState(const Thread& thread, SchedulerState::ProfileDirtyFlag expected)
      TA_REQ(thread.get_lock()) {
    thread.scheduler_state().effective_profile().AssertDirtyState(expected);
  }

  static SchedTime& GetStartTime(Thread& thread) TA_REQ(thread.get_lock()) {
    return thread.scheduler_state().start_time_;
  }

  static SchedTime& GetFinishTime(Thread& thread) TA_REQ(thread.get_lock()) {
    return thread.scheduler_state().finish_time_;
  }

  static SchedDuration& GetTimeSliceNs(Thread& thread) TA_REQ(thread.get_lock()) {
    return thread.scheduler_state().time_slice_ns_;
  }

  static SchedDuration& GetTimeSliceUsedNs(Thread& thread) TA_REQ(thread.get_lock()) {
    return thread.scheduler_state().time_slice_used_ns_;
  }

  static SchedTime GetStartTime(const Thread& thread) TA_REQ(thread.get_lock()) {
    return thread.scheduler_state().start_time_;
  }

  static SchedTime GetFinishTime(const Thread& thread) TA_REQ(thread.get_lock()) {
    return thread.scheduler_state().finish_time_;
  }

  static SchedDuration GetTimeSliceNs(const Thread& thread) TA_REQ(thread.get_lock()) {
    return thread.scheduler_state().time_slice_ns_;
  }

  static SchedDuration GetTimeSliceUsedNs(const Thread& thread) TA_REQ(thread.get_lock()) {
    return thread.scheduler_state().time_slice_used_ns_;
  }

  static SchedDuration GetRemainingTimeSliceNs(const Thread& thread) TA_REQ(thread.get_lock()) {
    return thread.scheduler_state().remaining_time_slice_ns();
  }

  // OwnedWaitQueues do not need to bother to track the dirty or clean state of
  // their implied effective profile.  They have no base profile (only inherited
  // values) which gets turned into an effective profile by the
  // EffectiveProfileHeper (see below) during a PI interaction.  We can get away
  // with this because OWQs:
  //
  // 1) Cannot exist in any collections where their position is determined by
  //    effective profile (otherwise we would need to remove and re-insert the
  //    node in the collection during an update).
  // 2) Cannot contribute to a scheduler's bookkeeping (because OWQs are not
  //    things which get scheduled).
  //
  static void AssertEpDirtyState(const OwnedWaitQueue& owq,
                                 SchedulerState::ProfileDirtyFlag expected) TA_REQ(owq.get_lock()) {
  }

  static SchedTime& GetStartTime(OwnedWaitQueue& owq) TA_REQ(owq.get_lock()) {
    return owq.inherited_scheduler_state_storage()->start_time;
  }

  static SchedTime& GetFinishTime(OwnedWaitQueue& owq) TA_REQ(owq.get_lock()) {
    return owq.inherited_scheduler_state_storage()->finish_time;
  }

  static SchedDuration& GetTimeSliceNs(OwnedWaitQueue& owq) TA_REQ(owq.get_lock()) {
    return owq.inherited_scheduler_state_storage()->time_slice_ns;
  }

  static SchedDuration& GetTimeSliceUsedNs(OwnedWaitQueue& owq) TA_REQ(owq.get_lock()) {
    return owq.inherited_scheduler_state_storage()->time_slice_used_ns;
  }

  static SchedTime GetStartTime(const OwnedWaitQueue& owq) TA_REQ(owq.get_lock()) {
    return owq.inherited_scheduler_state_storage()->start_time;
  }

  static SchedTime GetFinishTime(const OwnedWaitQueue& owq) TA_REQ(owq.get_lock()) {
    return owq.inherited_scheduler_state_storage()->finish_time;
  }

  static SchedDuration GetTimeSliceNs(const OwnedWaitQueue& owq) TA_REQ(owq.get_lock()) {
    return owq.inherited_scheduler_state_storage()->time_slice_ns;
  }

  static SchedDuration GetTimeSliceUsedNs(const OwnedWaitQueue& owq) TA_REQ(owq.get_lock()) {
    return owq.inherited_scheduler_state_storage()->time_slice_used_ns;
  }

  static SchedDuration GetRemainingTimeSliceNs(const OwnedWaitQueue& owq) TA_REQ(owq.get_lock()) {
    return owq.inherited_scheduler_state_storage()->time_slice_ns -
           owq.inherited_scheduler_state_storage()->time_slice_used_ns;
  }

  template <typename Upstream, typename UpdateDynamicParams>
  static inline void Common(Upstream& upstream, Thread& thread,
                            UpdateDynamicParams update_dynamic_params,
                            const SchedulerState::EffectiveProfile* old_target_ep = nullptr)
      TA_REQ(chainlock_transaction_token, upstream.get_lock(), thread.get_lock());

  template <typename Upstream, typename UpdateDynamicParams>
  static inline void Common(Upstream& upstream, OwnedWaitQueue& owq,
                            UpdateDynamicParams update_dynamic_params,
                            const SchedulerState::EffectiveProfile* old_target_ep = nullptr)
      TA_REQ(chainlock_transaction_token, upstream.get_lock(), owq.get_lock());
};

namespace {

// Threads contain internal storage which holds their "effective profile", the
// combination of their base profile and all of their inherited profile
// pressure, as well as set of dirty/clean flags.
//
// Owned wait queues don't have quite the same arrangement.  They themselves
// have no base profile, and their effective profile is really only the their
// inherited deadline profile (if any), or the total of their inherited fair
// weight (if there is no inherited deadline).  They do not explicitly maintain
// storage for their effective profile.
//
// When we get to this point in profile propagation, however, we need to be able
// to compute 3 things:
//
// 1) The effective profile of the target node before recomputing it because of
//    the change in profile pressure.
// 2) The effective profile of the target node after recomputing it because of
//    the change in profile pressure (note that this is the same for OWQs, but
//    not threads)
// 3) The effective profile of the upstream node which gave rise to the change
//    in target profile pressure.
//
// For threads, we can just access the reference to the current effective
// profile when we need to know.  The non-templated Thread version of
// HandlePiCommon can latch the old ep into a local variable before recomputing,
// and pass the reference to both the old and new profiles to the injected
// callback.  Likewise, if a thread is the upstream node and an operation needs
// to know the effective profile of the upstream node, a reference to the
// thread's internal storage is all which is needed.
//
// OWQs are a bit more problematic as they don't have internal storage to
// reference.  We actually need to compute what the effective profile is based
// on the current IPVs, and store that result somewhere.  We would rather not
// perform this calculation when we don't have to, and we would also rather not
// copy a thread's effective profile into local stack allocated storage if we
// don't have to.
//
// This starts to become an issue in the operations themselves, whose node types
// are templated to keep the logic consistent even when the nodes involved are
// different combinations of Thread and OWQ.  In particular, when an operation
// captures the `upstream` member in a lambda callback, we cannot simply call
// `upstream.effective_profile()` to fetch a reference to internal storage (OWQs
// don't have any), nor do we want to copy the thread's internal storage to a
// local EP instance when we could have used a const reference to the thread's
// internal storage instead.
//
// The GetEffectiveProfile functions provide a uniform way to compute an OWQ's
// effective profile, based on its IPVs, or to retrieve a thread's existing
// effective profile. Callsites can bind the result of GetEffectiveProfile to a
// const reference to make the code uniform, taking advantage of const
// reference lifetime extension for the temporary returned in the OWQ case.
SchedulerState::EffectiveProfile GetEffectiveProfile(const OwnedWaitQueue& owq)
    TA_REQ(owq.get_lock()) {
  DEBUG_ASSERT(owq.inherited_scheduler_state_storage() != nullptr);
  return owq.GetEffectiveProfile();
}

const SchedulerState::EffectiveProfile& GetEffectiveProfile(const Thread& thread)
    TA_REQ(thread.get_lock()) {
  return thread.scheduler_state().effective_profile();
}

}  // anonymous namespace

// Handle all of the common tasks associated with each of the possible PI
// interactions.  The outline of this is:
//
// 1) If the target is an active thread (meaning either running or runnable),
//    we need to:
// 1.1) Enter the scheduler's queue lock.
// 1.2) If the thread is active, but not actually running, remove the target
//      thread from its scheduler's run queue if it is in the queue.
// 1.3) Now update the thread's effective profile.
// 1.4) Apply any changes in the thread's effective profile to its scheduler's
//      bookkeeping.
// 1.5) Update the dynamic parameters of the thread.
// 1.6) Either re-insert the thread into its scheduler's run queue (if it was
//      READY AND in the queue) or adjust its schedulers preemption time (if it
//      was RUNNING).
// 1.7) Trigger a reschedule of the the thread's CPU.
// 2) If the target is either an OwnedWaitQueue, or a thread which is not
//    active:
// 2.1) Recompute the target's effective profile, adjust the target's position
//      in it's wait queue if the target is a thread which is currently
//      blocked in a wait queue.
// 2.2) Recompute the target's dynamic scheduler parameters.

// PI common path for threads.
template <typename Upstream, typename UpdateDynamicParams>
inline void Scheduler::Pi::Common(Upstream& upstream, Thread& thread,
                                  UpdateDynamicParams update_dynamic_params,
                                  const SchedulerState::EffectiveProfile* old_target_ep) {
  ktrace::Scope trace = LOCAL_KTRACE_BEGIN_SCOPE(COMMON, "Pi::Common(Thread)");

  SchedulerState& state = thread.scheduler_state();

  if (const cpu_num_t curr_cpu = state.curr_cpu_; curr_cpu != INVALID_CPU) {
    DEBUG_ASSERT_MSG((thread.state() == THREAD_RUNNING) || (thread.state() == THREAD_READY),
                     "Unexpected target_ state %u for tid %" PRIu64 "\n", thread.state(),
                     thread.tid());

    Scheduler& scheduler = *Get(curr_cpu);
    Guard<MonitoredSpinLock, NoIrqSave> queue_guard{&scheduler.queue_lock_, SOURCE_TAG};
    scheduler.ValidateInvariants();
    scheduler.AssertInScheduler(thread);

    // Sample the current time after acquiring the queue lock to avoid large
    // skews under contention.
    const SchedMonoTimeAndBootTicks now = CurrentMonoTimeAndBootTicks();

    // Keep track of the original disposition. See SchedulerQueueState for
    // more details on how the disposition and thread state indicate which
    // operations are permitted on the thread and its associated scheduler
    // bookkeeping.
    const Disposition disposition = thread.disposition();
    DEBUG_ASSERT_MSG(disposition != Disposition::Unassociated,
                     "Found unassociated thread %s with tid %lu in state %d, curr_cpu is: %u\n",
                     thread.name(), thread.tid(), thread.state(),
                     thread.scheduler_state().curr_cpu());

    if (thread.state() == THREAD_READY) {
      if (disposition == Disposition::Enqueued) {
        scheduler.EraseFromQueue(&thread);
      } else {
        CountUpdateInTransition();
      }
    } else {
      DEBUG_ASSERT(disposition == Disposition::Associated);
      ktrace::Scope trace_update = LOCAL_KTRACE_BEGIN_SCOPE(COMMON, "Update Timeslice",
                                                            ("target cpu", scheduler.this_cpu()));

      // Update the time slice before updating other bookkeeping.
      const SchedDuration actual_runtime_ns =
          ktl::max<SchedDuration>(now.mono_time - state.last_started_running_, SchedDuration{0});
      const SchedDuration scaled_actual_runtime_ns = state.effective_profile().IsDeadline()
                                                         ? scheduler.ScaleDown(actual_runtime_ns)
                                                         : actual_runtime_ns;

      state.runtime_ns_ += actual_runtime_ns;
      state.time_slice_used_ns_ += scaled_actual_runtime_ns;
      state.last_started_running_ = now.mono_time;
      scheduler.start_of_current_time_slice_ns_ = now.mono_time;
      scheduler.UpdateEstimatedEnergyConsumption(&thread, now, actual_runtime_ns);

      trace_update = KTRACE_END_SCOPE(("mono_now", now.mono_time), ("boot_ticks", now.boot_ticks),
                                      ("actual_runtime_ns", actual_runtime_ns));
    }

    // Copy the original effective profile before updating it to compute the
    // changes to the scheduler bookkeeping.
    const EffectiveProfile old_ep = state.effective_profile();
    thread.RecomputeEffectiveProfile();
    const EffectiveProfile& new_ep = state.effective_profile();

    // Update the scheduler bookkeeping, if necessary.
    bool bandwidth_demand_changed = false;
    if (disposition == Disposition::Associated || disposition == Disposition::Enqueued) {
      SchedWeight weight_delta{0};
      SchedUtilization utilization_delta{0};
      SchedUtilization critical_utilization_delta{0};

      if (old_ep.IsFair()) {
        weight_delta -= old_ep.weight();
      } else {
        utilization_delta -= old_ep.deadline().utilization;
        if (old_ep.is_critical()) {
          critical_utilization_delta -= old_ep.deadline().utilization;
        }
      }
      if (new_ep.IsFair()) {
        weight_delta += new_ep.weight();
      } else {
        utilization_delta += new_ep.deadline().utilization;
        if (new_ep.is_critical()) {
          critical_utilization_delta += new_ep.deadline().utilization;
        }
      }

      if (weight_delta != 0) {
        bandwidth_demand_changed = true;
        scheduler.weight_total_ += weight_delta;
        scheduler.UpdateFairBandwidthPeriod(now.mono_time);
      }
      if (utilization_delta != 0) {
        bandwidth_demand_changed = true;
        scheduler.UpdateTotalDeadlineUtilization(utilization_delta);
      }
      scheduler.critical_deadline_utilization_ += critical_utilization_delta;
    }

    DEBUG_ASSERT(scheduler.weight_total_ >= SchedWeight{0});
    DEBUG_ASSERT(scheduler.critical_deadline_utilization_ >= SchedUtilization{0});
    DEBUG_ASSERT(scheduler.power_level_control_.normalized_deadline_utilization() >=
                 SchedUtilization{0});

    update_dynamic_params(upstream, thread, old_ep, new_ep, now.mono_time);

    if (thread.state() == THREAD_READY) {
      if (disposition == Disposition::Enqueued) {
        scheduler.QueueThread(&thread, Placement::Adjustment);
      }
    } else {
      DEBUG_ASSERT(thread.state() == THREAD_RUNNING);
      if (new_ep.IsFair()) {
        scheduler.target_preemption_time_ns_ =
            scheduler.start_of_current_time_slice_ns_ + state.remaining_time_slice_ns();
      } else {
        const SchedDuration scaled_remaining_time_slice_ns =
            scheduler.ScaleUp(state.remaining_time_slice_ns());
        scheduler.target_preemption_time_ns_ = ktl::min<SchedTime>(
            scheduler.start_of_current_time_slice_ns_ + scaled_remaining_time_slice_ns,
            state.finish_time_);
      }

      // Emit a context switch event to and from the same thread to update the
      // visualized schedling parameters if the bandwidth demand actually
      // changed.
      if (bandwidth_demand_changed) {
        SchedTraceContextSwitch(&thread, &thread, curr_cpu);
      }
    }

    // Check that target is left in the same disposition.
    DEBUG_ASSERT(disposition == thread.disposition());

    // Reschedule to reflect the updated state.
    RescheduleMask(cpu_num_to_mask(state.curr_cpu_));
    scheduler.ValidateInvariants();
  } else {
    // The target thread is not runnable; update its effective profile and the
    // scheduler bookkeeping.
    SchedulerState::EffectiveProfile old_ep = state.effective_profile_;
    if (WaitQueueBase* wq = thread.wait_queue_state().blocking_wait_queue_; wq != nullptr) {
      wq->get_lock().AssertHeld();
      wq->UpdateBlockedThreadEffectiveProfile(thread);
    } else {
      thread.RecomputeEffectiveProfile();
    }
    update_dynamic_params(upstream, thread, old_ep, state.effective_profile_, CurrentTime());
  }

  DEBUG_ASSERT_MSG(SchedTime finish_time = GetFinishTime(thread);
                   finish_time >= 0, "finish_time %ld\n", finish_time.raw_value());
}

// PI common path for owned wait queues.
template <typename Upstream, typename UpdateDynamicParams>
inline void Scheduler::Pi::Common(Upstream& upstream, OwnedWaitQueue& owq,
                                  UpdateDynamicParams update_dynamic_params,
                                  const SchedulerState::EffectiveProfile* old_target_ep) {
  ktrace::Scope trace = LOCAL_KTRACE_BEGIN_SCOPE(COMMON, "Pi::Common(OwnedWaitQueue)");

  const SchedulerState::EffectiveProfile& old_ep =
      old_target_ep ? *old_target_ep : GetEffectiveProfile(owq);
  SchedulerState::EffectiveProfile new_ep = GetEffectiveProfile(owq);
  update_dynamic_params(upstream, owq, old_ep, new_ep, CurrentTime());

  DEBUG_ASSERT_MSG(SchedTime finish_time = GetFinishTime(owq);
                   finish_time >= 0, "finish_time %ld\n", finish_time.raw_value());
}

// Updates a thread's effective profile and position in its container at the
// start of a base profile update operation, regardless of whether or not the
// target thread is currently blocked or currently assigned to a scheduler.
//
// Later on, if the thread happens to be an upstream member of a PI graph whose
// target is either an OwnedWaitQueue or another thread, the
// UpstreamThreadBaseProfileChanged operation will be triggered to handle the
// downstream target of the graph.
void Scheduler::ThreadBaseProfileChanged(Thread& thread) {
  // The base profile of this thread has changed.  While there may or may not be
  // something downstream of this thread, we need to start by dealing with
  // updating this threads static and dynamic scheduling parameters first.
  Pi::AssertEpDirtyState(thread, SchedulerState::ProfileDirtyFlag::BaseDirty);

  const auto update_dynamic_params =
      +[](const Thread&, Thread& target, const SchedulerState::EffectiveProfile& target_old_ep,
          const SchedulerState::EffectiveProfile& target_new_ep, SchedTime mono_now)
           TA_REQ(chainlock_transaction_token, target.get_lock()) {
             // Make sure the start and finish times are consistent with the bandwidth
             // parameters of the deadline profile. Consistency in fair profiles is
             // handled by Scheduler::AdjustFairBandwidth.
             if (target_new_ep.IsDeadline()) {
               Pi::GetStartTime(target) =
                   Pi::GetFinishTime(target) - target_new_ep.deadline().deadline_ns;
             }
           };

  Pi::Common(thread, thread, update_dynamic_params);
}

// Called when a thread in a graph whose target is either an OwnedWaitQueue or
// a different Thread changes its base profile in order update the target's new
// effective profile, position in container, and dynamic scheduling parameters.
template <typename TargetType>
void Scheduler::UpstreamThreadBaseProfileChanged(const Thread& upstream, TargetType& target) {
  ktrace::Scope trace = LOCAL_KTRACE_BEGIN_SCOPE(COMMON, "sched_pi: base profile changed");
  // The base profile of a thread upstream of this target node has changed.  We need to
  // do the following:
  //
  // 1) Recompute the target's effective profile.
  // 2) Handle any bookkeeping updates for the scheduler's state, if the target
  //    is a thread which is either RUNNING or READY, and therefore has a
  //    scheduler assigned to it.
  // 3) Handle any updates to the target's dynamic scheduling parameters (eg,
  //    start time, finish time, time slice remaining)
  if constexpr (ktl::is_same_v<Thread, TargetType>) {
    DEBUG_ASSERT(&upstream != &target);
  }
  Pi::AssertEpDirtyState(target, SchedulerState::ProfileDirtyFlag::InheritedDirty);
  Pi::AssertEpDirtyState(upstream, SchedulerState::ProfileDirtyFlag::Clean);

  const auto update_dynamic_params = +[](const Thread& upstream, TargetType& target,
                                         const SchedulerState::EffectiveProfile& target_old_ep,
                                         const SchedulerState::EffectiveProfile& target_new_ep,
                                         SchedTime mono_now) TA_REQ(upstream.get_lock(),
                                                                    target.get_lock()) {
    // Make sure the start and finish times are consistent with the bandwidth
    // parameters of the deadline profile. Consistency in fair profiles is
    // handled by Scheduler::AdjustFairBandwidth.
    if (target_new_ep.IsDeadline()) {
      Pi::GetStartTime(target) = Pi::GetFinishTime(target) - target_new_ep.deadline().deadline_ns;
    }
  };

  Pi::Common(upstream, target, update_dynamic_params);
}

// Called when a new edge is added connecting the target of one PI graph (the
// upstream node) to a different PI graph.
template <typename UpstreamType, typename TargetType>
void Scheduler::JoinNodeToPiGraph(const UpstreamType& upstream, TargetType& target,
                                  ForceInheritance force_inheritance) {
  ktrace::Scope trace = LOCAL_KTRACE_BEGIN_SCOPE(COMMON, "sched_pi: join");

  if constexpr (ktl::is_same_v<UpstreamType, TargetType>) {
    DEBUG_ASSERT(&upstream != &target);
  }
  Pi::AssertEpDirtyState(target, SchedulerState::ProfileDirtyFlag::InheritedDirty);
  Pi::AssertEpDirtyState(upstream, SchedulerState::ProfileDirtyFlag::Clean);

  const auto update_dynamic_params = +[](const UpstreamType& upstream, TargetType& target,
                                         const SchedulerState::EffectiveProfile& target_old_ep,
                                         const SchedulerState::EffectiveProfile& target_new_ep,
                                         SchedTime mono_now) TA_REQ(chainlock_transaction_token,
                                                                    upstream.get_lock(),
                                                                    target.get_lock()) {
    const SchedulerState::EffectiveProfile& upstream_ep = GetEffectiveProfile(upstream);

    // If our upstream node is fair, then we have nothing more to do in the
    // common path. Our target's effective profile has already been updated
    // appropriately, and no changes to the target's dynamic deadline scheduling
    // parameters needs to be done (since new pressure from a fair thread
    // currently has no effect on deadline utilization). Any scheduler specific
    // side effects will be handled by the active thread path (below) if the
    // target is an active thread.
    // TODO(https://fxbug.dev/42182908): Implement fair-to-fair priority
    // inheritance.
    if (upstream_ep.IsFair()) {
      return;
    }

    // Our upstream node is not a fair node, therefore it must be a deadline
    // node. In addition, no matter what it was before, our target node must now
    // be a deadline node.
    DEBUG_ASSERT(upstream_ep.IsDeadline());
    DEBUG_ASSERT(target_new_ep.IsDeadline());

    // Verify the start/finish times of the upstream node have the required
    // relationship.
    // TODO(https://fxbug.dev/448121736): Start time == finish time.
    DEBUG_ASSERT_MSG(
        Pi::GetStartTime(upstream) < Pi::GetFinishTime(upstream),
        "upstream_ep: start_time=%" PRId64 " finish_time=%" PRId64 " deadline_ns=%" PRId64,
        Pi::GetStartTime(upstream).raw_value(), Pi::GetFinishTime(upstream).raw_value(),
        upstream_ep.deadline().deadline_ns.raw_value());

    if (target_old_ep.IsFair()) {
      // If target has just now become deadline, we can simply transfer the
      // dynamic deadline parameters from upstream to the target.
      Pi::GetStartTime(target) = Pi::GetStartTime(upstream);
      Pi::GetFinishTime(target) = Pi::GetFinishTime(upstream);
      Pi::GetTimeSliceNs(target) = Pi::GetTimeSliceNs(upstream);
      Pi::GetTimeSliceUsedNs(target) = Pi::GetTimeSliceUsedNs(upstream);
    } else {
      // The target was already a deadline thread, then we need to recompute the
      // target's dynamic deadline parameters using the lag equation. Compute
      // the remaining periods of the target and upstream threads.
      const SchedDuration target_remaining_period =
          ktl::max<SchedDuration>(Pi::GetFinishTime(target) - mono_now, SchedDuration{0});
      const SchedDuration upstream_remaining_period =
          ktl::max<SchedDuration>(Pi::GetFinishTime(upstream) - mono_now, SchedDuration{0});
      const SchedDuration min_remaining_period =
          ktl::min(target_remaining_period, upstream_remaining_period);

      Pi::GetFinishTime(target) = ktl::min(Pi::GetFinishTime(target), Pi::GetFinishTime(upstream));
      Pi::GetStartTime(target) = Pi::GetFinishTime(target) - target_new_ep.deadline().deadline_ns;

      // Verify the start/finish times of the target node have the required
      // relationship.
      // TODO(https://fxbug.dev/448121736): Start time == finish time.
      DEBUG_ASSERT_MSG(Pi::GetStartTime(target) < Pi::GetFinishTime(target),
                       "target_new_ep: start_time=%" PRId64 " finish_time=%" PRId64
                       " deadline_ns=%" PRId64,
                       Pi::GetStartTime(target).raw_value(), Pi::GetFinishTime(target).raw_value(),
                       target_new_ep.deadline().deadline_ns.raw_value());

      // TODO(eieio): If a period is expired, the full bandwidth contribution of
      // the respective task is available to the target and downstream, if any.
      const SchedDuration new_remaining_time_slice =
          Pi::GetRemainingTimeSliceNs(target) + Pi::GetRemainingTimeSliceNs(upstream) +
          (target_old_ep.deadline().utilization *
           (min_remaining_period - target_remaining_period)) +
          (upstream_ep.deadline().utilization * (min_remaining_period - upstream_remaining_period));

      // Limit the TSR.  It cannot be less than zero nor can it be more than the
      // time until the absolute deadline of the new combined thread.
      //
      // TODO(johngro): If we did have to clamp the TSR, the amount we clamp by
      // needs to turn into carried lag.
      const SchedDuration clamped_remaining_time_slice = ktl::clamp<SchedDuration>(
          new_remaining_time_slice, SchedDuration{0}, min_remaining_period);
      Pi::GetTimeSliceNs(target) = target_new_ep.deadline().capacity_ns;
      Pi::GetTimeSliceUsedNs(target) =
          target_new_ep.deadline().capacity_ns - clamped_remaining_time_slice;
      DEBUG_ASSERT_MSG(Pi::GetRemainingTimeSliceNs(target) >= 0,
                       "capacity=%" PRId64 " remaining_time_slice=%" PRId64
                       " remaining_period=%" PRId64,
                       target_new_ep.deadline().capacity_ns.raw_value(),
                       new_remaining_time_slice.raw_value(), min_remaining_period.raw_value());
    }
  };

  Pi::Common(upstream, target, update_dynamic_params);
}

// Called when an upstream node has its downstream edge removed, splitting it
// from the PI graph it was a member of and becoming the target of a new graph
// in the process.
template <typename UpstreamType, typename TargetType>
void Scheduler::SplitNodeFromPiGraph(UpstreamType& upstream, TargetType& target,
                                     const SchedulerState::EffectiveProfile* old_target_ep) {
  ktrace::Scope trace = LOCAL_KTRACE_BEGIN_SCOPE(COMMON, "sched_pi: split");

  if constexpr (ktl::is_same_v<UpstreamType, TargetType>) {
    DEBUG_ASSERT(&upstream != &target);
  }
  Pi::AssertEpDirtyState(target, SchedulerState::ProfileDirtyFlag::InheritedDirty);
  Pi::AssertEpDirtyState(upstream, SchedulerState::ProfileDirtyFlag::Clean);

  const auto update_dynamic_params = +[](UpstreamType& upstream, TargetType& target,
                                         const SchedulerState::EffectiveProfile& target_old_ep,
                                         const SchedulerState::EffectiveProfile& target_new_ep,
                                         SchedTime mono_now) TA_REQ(chainlock_transaction_token,
                                                                    upstream.get_lock(),
                                                                    target.get_lock()) {
    const SchedulerState::EffectiveProfile& upstream_ep = GetEffectiveProfile(upstream);

    // Was the target node a fair node? If so, there is really nothing for us to
    // do here.
    if (target_old_ep.IsFair()) {
      return;
    }

    DEBUG_ASSERT(target_old_ep.IsDeadline());
    if (target_new_ep.IsFair()) {
      // If target node is now a fair node, then the upstream node must have
      // been a deadline node. This split operation is what caused the target
      // node to change from deadline to fair, all of the deadline pressure must
      // have been coming from the upstream node. Assert all of this.
      DEBUG_ASSERT(upstream_ep.IsDeadline());
      DEBUG_ASSERT_MSG(target_old_ep.deadline().capacity_ns == upstream_ep.deadline().capacity_ns,
                       "toep.deadline.capacity=%" PRId64 " uep.deadline.capacity=%" PRId64,
                       target_old_ep.deadline().capacity_ns.raw_value(),
                       upstream_ep.deadline().capacity_ns.raw_value());
      DEBUG_ASSERT_MSG(target_old_ep.deadline().deadline_ns == upstream_ep.deadline().deadline_ns,
                       "toep.deadline.deadline=%" PRId64 " uep.deadline.deadline=%" PRId64,
                       target_old_ep.deadline().deadline_ns.raw_value(),
                       upstream_ep.deadline().deadline_ns.raw_value());

      // Verify the start/finish times of the target node have the required
      // relationship.
      // TODO(https://fxbug.dev/448121736): Start time == finish time.
      DEBUG_ASSERT_MSG(Pi::GetStartTime(target) < Pi::GetFinishTime(target),
                       "target_old_ep: start_time=%" PRId64 " finish_time=%" PRId64
                       " deadline_ns=%" PRId64,
                       Pi::GetStartTime(target).raw_value(), Pi::GetFinishTime(target).raw_value(),
                       target_old_ep.deadline().deadline_ns.raw_value());

      // Give the dynamic deadline parameters over to the upstream node.
      Pi::GetStartTime(upstream) = Pi::GetStartTime(target);
      Pi::GetFinishTime(upstream) = Pi::GetFinishTime(target);
      Pi::GetTimeSliceNs(upstream) = Pi::GetTimeSliceNs(target);
      Pi::GetTimeSliceUsedNs(upstream) = Pi::GetTimeSliceUsedNs(target);

      // TODO(eieio): Just expire the time slice for now. This should actually
      // be split out the same way as for deadline threads.
      Pi::GetTimeSliceUsedNs(target) = Pi::GetTimeSliceNs(target);
    } else {
      // OK, the target node is still a deadline node. If the upstream node is a
      // fair node, we don't have to do anything at all. A fair node splitting
      // off from a deadline node should not change the deadline node's dynamic
      // parameters. If the upstream fair node is a thread, it is going to
      // arrive in a new scheduler queue Real Soon Now, and have new dynamic
      // parameters computed for it.
      //
      // If both nodes are deadline nodes, then we need to invoke the lag
      // equation in order to figure out what the new time slice remaining and
      // absolute deadlines are.
      if (upstream_ep.IsDeadline()) {
        // Compute the time until absolute deadline of the target and upstream.
        const SchedDuration target_remaining_period =
            ktl::max<SchedDuration>(Pi::GetFinishTime(target) - mono_now, SchedDuration{0});
        const SchedDuration upstream_remaining_period =
            ktl::max<SchedDuration>(Pi::GetFinishTime(upstream) - mono_now, SchedDuration{0});

        // Figure out what the uncapped utilization of the combined thread
        // would have been based on the utilizations of the target and
        // upstream nodes after the split. It is important when scaling
        // timeslices to be sure that we divide by a utilization value which
        // is the sum of the two (now separated) utilization values.
        const SchedUtilization combined_uncapped_utilization =
            target_new_ep.deadline().utilization + upstream_ep.deadline().utilization;
        const SchedUtilization upstream_utilization_ratio =
            upstream_ep.deadline().utilization / combined_uncapped_utilization;
        const SchedDuration new_upstream_remaining_time_slice =
            (upstream_utilization_ratio * Pi::GetRemainingTimeSliceNs(target)) +
            (upstream_ep.deadline().utilization *
             (upstream_remaining_period - target_remaining_period));

        // TODO(johngro): This also changes when carried lag comes into play.
        Pi::GetTimeSliceNs(upstream) = upstream_ep.deadline().capacity_ns;
        Pi::GetTimeSliceUsedNs(upstream) =
            upstream_ep.deadline().capacity_ns -
            ktl::max(new_upstream_remaining_time_slice, SchedDuration{0});

        // TODO(johngro): Fix this. Logically, it is not correct to preserve the
        // abs deadline of the target after the split. The target's bookkeeping
        // should be equivalent to the values which would be obtained by joining
        // all of the threads which exist upstream of this node together.
        // Because of this, our new target finish time should be equal to the
        // min across all finish times immediately upstream of this node.
        //
        // Now handle the target node. We preserve the absolute deadline of the
        // target node before and after the split, so we need to recompute its
        // start time so that the distance between the absolute deadline and the
        // start time is equal to the new relative deadline of the target node.
        Pi::GetStartTime(target) = Pi::GetFinishTime(target) - target_new_ep.deadline().deadline_ns;

        // Verify the start/finish times of the target node have the required
        // relationship.
        // TODO(https://fxbug.dev/448121736): Start time == finish time.
        DEBUG_ASSERT_MSG(
            Pi::GetStartTime(target) < Pi::GetFinishTime(target),
            "target_new_ep: start_time=%" PRId64 " finish_time=%" PRId64 " deadline_ns=%" PRId64,
            Pi::GetStartTime(target).raw_value(), Pi::GetFinishTime(target).raw_value(),
            target_new_ep.deadline().deadline_ns.raw_value());

        // The time till absolute deadline of the pre and post split target
        // remains the same, so the ttad contributions to the timeslice
        // remaining simply drop out of the lag equation.
        //
        // Note that fixed point division takes the precision of the assignee
        // into account to provide headroom in certain situations. Use an
        // intermediate with the same fractional precision as the utilization
        // operands before scaling the non-fractional timeslice.
        const SchedUtilization target_utilization_ratio =
            target_new_ep.deadline().utilization / combined_uncapped_utilization;
        const SchedDuration new_target_remaining_time_slice =
            Pi::GetRemainingTimeSliceNs(target) * target_utilization_ratio;

        Pi::GetTimeSliceNs(target) = target_new_ep.deadline().capacity_ns;
        Pi::GetTimeSliceUsedNs(target) =
            target_new_ep.deadline().capacity_ns -
            ktl::max(new_target_remaining_time_slice, SchedDuration{0});
      }
    }
  };

  Pi::Common(upstream, target, update_dynamic_params, old_target_ep);
}

template void Scheduler::UpstreamThreadBaseProfileChanged(const Thread&, Thread&);
template void Scheduler::UpstreamThreadBaseProfileChanged(const Thread&, OwnedWaitQueue&);

template void Scheduler::JoinNodeToPiGraph(const Thread&, Thread&, ForceInheritance);
template void Scheduler::JoinNodeToPiGraph(const Thread&, OwnedWaitQueue&, ForceInheritance);
template void Scheduler::JoinNodeToPiGraph(const OwnedWaitQueue&, Thread&, ForceInheritance);
template void Scheduler::JoinNodeToPiGraph(const OwnedWaitQueue&, OwnedWaitQueue&,
                                           ForceInheritance);

template void Scheduler::SplitNodeFromPiGraph(Thread&, Thread&,
                                              const SchedulerState::EffectiveProfile*);
template void Scheduler::SplitNodeFromPiGraph(Thread&, OwnedWaitQueue&,
                                              const SchedulerState::EffectiveProfile*);
template void Scheduler::SplitNodeFromPiGraph(OwnedWaitQueue&, Thread&,
                                              const SchedulerState::EffectiveProfile*);
template void Scheduler::SplitNodeFromPiGraph(OwnedWaitQueue&, OwnedWaitQueue&,
                                              const SchedulerState::EffectiveProfile*);
