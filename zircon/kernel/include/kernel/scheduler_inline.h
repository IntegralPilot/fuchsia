// Copyright 2022 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#ifndef ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_INLINE_H_
#define ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_INLINE_H_

// This file defines a number of inline methods out-of-line with the
// declarations in kernel/scheduler.h to reduce the clutter in that header,
// while guaranteeing that callers see the definitions so that inlining may
// occur.

#include <lib/kconcurrent/chainlock_transaction.h>

#include <arch/interrupt.h>
#include <arch/mp.h>
#include <ffl/fixed_format.h>
#include <kernel/scheduler.h>
#include <kernel/scheduler_tracing.h>
#include <ktl/algorithm.h>

// Scales the given value up by the reciprocal of the CPU performance scale.
template <typename T>
inline T Scheduler::ScaleUp(T value) const {
  return value * processing_rate_reciprocal();
}

// Scales the given value down by the CPU performance scale.
template <typename T>
inline T Scheduler::ScaleDown(T value) const {
  return value * processing_rate();
}

// Returns a new flow id when flow tracing is enabled, zero otherwise.
inline uint64_t Scheduler::NextFlowId() {
  if constexpr (LOCAL_KTRACE_LEVEL_ENABLED(FLOW)) {
    return next_flow_id_.fetch_add(1);
  }
  return 0;
}

// Updates the total estimated runtime estimator with the given delta. The
// exported value is scaled by the relative performance factor of the CPU to
// account for performance differences in the estimate.
inline void Scheduler::UpdateTotalExpectedRuntime(SchedDuration delta_ns) {
  total_expected_runtime_ns_ += delta_ns;
  DEBUG_ASSERT(total_expected_runtime_ns_ >= 0);
  const SchedDuration scaled_ns = ScaleUp(total_expected_runtime_ns_);
  exported_queue_time_ns_ = scaled_ns;
  LOCAL_KTRACE_COUNTER(COUNTER, "Estimated Runtime", this_cpu(), ("CPU", scaled_ns.raw_value()));

  if (power_level_control_.is_enabled()) {
    const SchedUtilization fair_demand = power_level_control_.fair_demand();
    exported_fair_utilization_ = fair_demand;
    exported_clamped_fair_utilization_ = power_level_control_.clamped_fair_demand();
    exported_clamped_total_utilization_ = power_level_control_.clamped_total_demand();

    auto latched_timestamp = KTrace::LatchedTimestamp();
    KTRACE_CPU_COUNTER_TIMESTAMP("kernel:power", "Demand:Variable", latched_timestamp(), this_cpu(),
                                 ("CPU", ffl::Round<uint64_t>(fair_demand * 1000)));
    KTRACE_CPU_COUNTER_TIMESTAMP(
        "kernel:power", "Demand:Total", latched_timestamp(), this_cpu(),
        ("CPU", ffl::Round<uint64_t>(power_level_control_.total_demand() * 1000)));
  }
}

// Updates the total deadline utilization estimator with the given delta.
inline void Scheduler::UpdateTotalDeadlineUtilization(SchedUtilization delta) {
  // Avoid unnecessary trace counter and atomic variable updates.
  if (delta != 0) {
    const SchedUtilization utilization = power_level_control_.UpdateNormalizedUtilization(delta);
    DEBUG_ASSERT_MSG(utilization >= 0, "utilization=%s delta=%s", Format(utilization).c_str(),
                     Format(delta).c_str());
    exported_deadline_utilization_ = utilization;
    exported_clamped_deadline_utilization_ = power_level_control_.clamped_deadline_utilization();
    exported_clamped_total_utilization_ = power_level_control_.clamped_total_demand();

    auto latched_timestamp = KTrace::LatchedTimestamp();
    KTRACE_CPU_COUNTER_TIMESTAMP("kernel:power", "Demand:Constant", latched_timestamp(), this_cpu(),
                                 ("CPU", ffl::Round<uint64_t>(utilization * 1000)));
    KTRACE_CPU_COUNTER_TIMESTAMP(
        "kernel:power", "Demand:Total", latched_timestamp(), this_cpu(),
        ("CPU", ffl::Round<uint64_t>(power_level_control_.total_demand() * 1000)));

    if (const ktl::optional<uint32_t> domain_id = power_level_control_.domain_id()) {
      const SchedUtilization domain_utilization =
          power_level_control_.domain_deadline_utilization();
      LOCAL_KTRACE_COUNTER_TIMESTAMP(BANDWIDTH, "Demand:Constant", latched_timestamp(),
                                     domain_id.value(),
                                     ("Domain", ffl::Round<uint64_t>(domain_utilization * 1000)));
    }
  }
}

inline bool Scheduler::UpdateProcessingRate(zx_instant_boot_ticks_t boot_ticks) {
  if (power_level_control_.is_processing_rate_update_pending()) {
    const SchedProcessingRate processing_rate = power_level_control_.UpdateProcessingRate();
    exported_processing_rate_ = processing_rate;
    if (!power_level_control_.domain()) {
      exported_max_processing_rate_ = power_level_control_.clamped_max_processing_rate();
    }
    KTRACE_CPU_COUNTER_TIMESTAMP("kernel:power", "Rate", boot_ticks, this_cpu(),
                                 ("CPU", ffl::Round<uint64_t>(processing_rate * 1000)));
    return true;
  }
  return false;
}

inline void Scheduler::TraceTotalRunnableThreads() const {
  LOCAL_KTRACE_COUNTER(COUNTER, "Queue Length", this_cpu(), ("CPU", runnable_task_count()));
}

inline void Scheduler::RescheduleMask(cpu_mask_t cpus_to_reschedule_mask) {
  // Does the local CPU need to be preempted?
  const cpu_mask_t local_mask = cpu_num_to_mask(arch_curr_cpu_num());
  const cpu_mask_t local_cpu = cpus_to_reschedule_mask & local_mask;

  PreemptionState& preemption_state = Thread::Current::Get()->preemption_state();

  // First deal with the remote CPUs.
  //
  // Can we reschedule them?
  if (preemption_state.EagerReschedDisableCount() > 0) {
    // EagerReschedDisabled implies that local preemption is also disabled.
    DEBUG_ASSERT(!preemption_state.PreemptIsEnabled());
    // Nope, save them for later.
    preemption_state.preempts_pending_add(cpus_to_reschedule_mask);
    if (local_cpu != 0) {
      preemption_state.EvaluateTimesliceExtension();
    }
    return;
  }

  // |mp_reschedule| will remove the local cpu from the mask for us.
  mp_reschedule(cpus_to_reschedule_mask, 0);

  // Now deal with the local CPU.
  if (local_cpu == 0) {
    // Notihng to do.
    return;
  }
  const bool preempt_enabled = preemption_state.EvaluateTimesliceExtension();

  // Can we do it here and now?
  if (!preempt_enabled) {
    // Nope, can't do it now.  Make a note for later.
    preemption_state.preempts_pending_add(local_cpu);
    return;
  }

  // From a chain-lock requirement perspective, there are a few different
  // cases to consider.
  //
  // 1) We currently have no transaction in progress and hold no locks.
  //    In this case, we can simply call Preempt, which will start a
  //    transaction, lock our current thread, and take care of preemption.
  // 2) We do currently have a transaction in progress, but we hold no
  //    locks.  This can happen in the cases such as Unblock(thread) or
  //    Unblock(list), where all of the unblocking thread's locks were
  //    dropped after adding them to the proper scheduler queue.  We need
  //    to re-use our existing transaction by first restarting it, then
  //    obtaining our current thread's lock, and finally calling
  //    PreemptLocked.
  // 3) We do currently have a transaction in progress, and we are holding
  //    exactly one lock which is the current thread's lock.  In this case,
  //    we call simply PreemptLocked directly.
  //
  // TODO(johngro): Determine if #1 and #3 are actual possibilities by the
  // time that we hit this stage.  #2 may be the only legit case, but I'm
  // not quite sure yet.
  ChainLockTransaction* const active_clt = ChainLockTransaction::Active();
  if (active_clt == nullptr) {
    Preempt(PreemptType::Reschedule);
    return;
  }

  ChainLockTransaction::MarkActive();
  Thread* const current_thread = Thread::Current::Get();
  if (current_thread->get_lock().is_held() == false) {
    active_clt->Restart(CLT_TAG("Scheduler::RescheduleMask"));
    active_clt->AssertNumLocksHeld(0);
    ChainLockGuard guard{current_thread->get_lock()};
    active_clt->Finalize();
    PreemptLocked(current_thread, PreemptType::Reschedule);
    return;
  }

  active_clt->AssertNumLocksHeld(1);
  current_thread->get_lock().AssertHeld();
  PreemptLocked(current_thread, PreemptType::Reschedule);
}

inline void Scheduler::RescheduleCpus(cpu_mask_t cpu_mask) {
  InterruptDisableGuard interrupt_disable;
  // TODO(eieio): See if IPIs to idle CPUs can be elided. This may require
  // refactoring some code that updates scheduler bookkeeping and trace
  // counters, such that the modifying CPU does all of the work so the target
  // CPU can remain idle.
  RescheduleMask(cpu_mask);
}

inline zx_thread_state_t SchedUserThreadState(const Thread* thread)
    TA_REQ_SHARED(thread->get_lock()) {
  switch (thread->state()) {
    case THREAD_INITIAL:
    case THREAD_READY:
      return ZX_THREAD_STATE_NEW;
    case THREAD_RUNNING:
      return ZX_THREAD_STATE_RUNNING;
    case THREAD_BLOCKED:
    case THREAD_BLOCKED_READ_LOCK:
    case THREAD_SLEEPING:
      return ZX_THREAD_STATE_BLOCKED;
    case THREAD_SUSPENDED:
      return ZX_THREAD_STATE_SUSPENDED;
    case THREAD_DEATH:
      return ZX_THREAD_STATE_DEAD;
    default:
      return UINT32_MAX;
  }
}

constexpr int32_t kIdleWeight = ktl::numeric_limits<int32_t>::min();

// Writes a context switch record to the ktrace buffer. This is always enabled
// so that user mode tracing can track which threads are running.
inline void SchedTraceContextSwitch(Thread* current_thread, Thread* next_thread,
                                    cpu_num_t current_cpu)
    TA_REQ(current_thread->get_lock(), next_thread->get_lock()) {
  SchedulerState& current_state = current_thread->scheduler_state();
  SchedulerState& next_state = next_thread->scheduler_state();
  KTRACE_CONTEXT_SWITCH(
      "kernel:sched", current_cpu, SchedUserThreadState(current_thread), current_thread->fxt_ref(),
      next_thread->fxt_ref(),
      ("outgoing_weight",
       current_thread->IsIdle() ? kIdleWeight : current_state.GetWeightOrPackedDeadlineParams()),
      ("incoming_weight",
       next_thread->IsIdle() ? kIdleWeight : next_state.GetWeightOrPackedDeadlineParams()));
}

// Writes a thread wakeup record to the ktrace buffer. This is always enabled
// so that user mode tracing can track which threads are waking.
inline void SchedTraceWakeup(Thread* thread, cpu_num_t target_cpu) TA_REQ(thread->get_lock()) {
  SchedulerState& state = thread->scheduler_state();
  const Thread* current_thread = Thread::Current::Get();
  if (!current_thread->IsIdle()) {
    KTRACE_THREAD_WAKEUP(
        "kernel:sched", target_cpu, thread->fxt_ref(),
        ("weight", thread->IsIdle() ? kIdleWeight : state.GetWeightOrPackedDeadlineParams()),
        ("waker", ktrace::Koid{current_thread->tid()}));
  } else {
    KTRACE_THREAD_WAKEUP(
        "kernel:sched", target_cpu, thread->fxt_ref(),
        ("weight", thread->IsIdle() ? kIdleWeight : state.GetWeightOrPackedDeadlineParams()));
  }
}

#endif  // ZIRCON_KERNEL_INCLUDE_KERNEL_SCHEDULER_INLINE_H_
