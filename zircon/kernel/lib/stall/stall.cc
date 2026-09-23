// Copyright 2024 The Fuchsia Authors
//
// Use of this source code is governed by a MIT-style
// license that can be found in the LICENSE file or at
// https://opensource.org/licenses/MIT

#include "lib/stall.h"

#include <arch/interrupt.h>
#include <kernel/percpu.h>
#include <lk/init.h>

StallAggregator StallAggregator::singleton_;

void StallAccumulator::UpdateWithIrqDisabled(int op_contributors_progressing,
                                             int op_contributors_stalling) {
  // Check argument range.
  ZX_DEBUG_ASSERT(-1 <= op_contributors_progressing && op_contributors_progressing <= +1);
  ZX_DEBUG_ASSERT(-1 <= op_contributors_stalling && op_contributors_stalling <= +1);

  Guard<SpinLock, NoIrqSave> guard{&lock_};
  Consolidate();

  // Apply variations.
  num_contributors_progressing_ += op_contributors_progressing;
  num_contributors_stalling_ += op_contributors_stalling;

  // Check that we are counting correctly and we never decrement below zero.
  ZX_DEBUG_ASSERT(num_contributors_progressing_ != SIZE_MAX);
  ZX_DEBUG_ASSERT(num_contributors_stalling_ != SIZE_MAX);
}

void StallAccumulator::Update(int op_contributors_progressing, int op_contributors_stalling) {
  InterruptDisableGuard guard;
  UpdateWithIrqDisabled(op_contributors_progressing, op_contributors_stalling);
}

StallAccumulator::Stats StallAccumulator::Flush() {
  Guard<SpinLock, IrqSave> guard{&lock_};
  Consolidate();

  Stats result = accumulated_stats_;
  accumulated_stats_ = {};
  return result;
}

void StallAccumulator::Consolidate() {
  zx_instant_mono_t now = current_mono_time();
  zx_duration_mono_t time_delta = now - last_consolidate_time_;

  if (num_contributors_stalling_ > 0) {
    accumulated_stats_.total_time_stall_some += time_delta;
  }

  if (num_contributors_stalling_ > 0 && num_contributors_progressing_ == 0) {
    accumulated_stats_.total_time_stall_full += time_delta;
  }

  if (num_contributors_progressing_ > 0 || num_contributors_stalling_ > 0) {
    accumulated_stats_.total_time_active += time_delta;
  }

  last_consolidate_time_ = now;
}

void StallAccumulator::ApplyContextSwitch(Thread *current_thread, Thread *next_thread) {
  // Helper functions to get/set the given thread's memory_stall_state value,
  // which is guarded by preempt_disabled_token. This is necessary because
  // preemption is not formally disabled when we're called (i.e. within the
  // scheduler), but in fact it is, because the scheduler does not preempt
  // itself.
  auto GetMemoryStallState = [](const Thread *thread) TA_NO_THREAD_SAFETY_ANALYSIS {
    return thread->memory_stall_state();
  };
  auto SetMemoryStallState =
      [](Thread *thread, ThreadStallState value)
          TA_NO_THREAD_SAFETY_ANALYSIS { thread->set_memory_stall_state(value); };

  cpu_num_t current_cpu = arch_curr_cpu_num();

  const SchedulerState *next_state = &next_thread->scheduler_state();
  ZX_DEBUG_ASSERT(next_state->curr_cpu() == current_cpu);

  StallAccumulator &local_accumulator = percpu::GetCurrent().memory_stall_accumulator;
  int local_op_progressing = 0;
  int local_op_stalling = 0;

  // If the current thread is not stalling, remove it from the number of progressing threads.
  if (GetMemoryStallState(current_thread) == ThreadStallState::Progressing) {
    SetMemoryStallState(current_thread, ThreadStallState::Inactive);
    local_op_progressing -= 1;
  }

  // If the next thread has been migrated while stalling, move its stall to the current CPU.
  if (GetMemoryStallState(next_thread) == ThreadStallState::Stalling &&
      next_state->last_cpu() != current_cpu) {
    // Unmark the thread as stalling on its last CPU.
    ZX_DEBUG_ASSERT_MSG(next_state->last_cpu() != INVALID_CPU,
                        "Stalling threads must have run at least once");
    StallAccumulator &last_accumulator =
        percpu::Get(next_state->last_cpu()).memory_stall_accumulator;
    last_accumulator.UpdateWithIrqDisabled(0, -1);

    // We now have new a stalling thread tied to the current_cpu.
    local_op_stalling += 1;
  }

  // If the next thread is not stalling, add it to the local counter.
  if (GetMemoryStallState(next_thread) == ThreadStallState::Inactive) {
    SetMemoryStallState(next_thread, ThreadStallState::Progressing);
    local_op_progressing += 1;
  }

  // Propagate changes (we can skip this to be faster if there are no changes).
  if (local_op_progressing != 0 || local_op_stalling != 0) {
    local_accumulator.UpdateWithIrqDisabled(local_op_progressing, local_op_stalling);
  }
}

zx::result<ktl::unique_ptr<StallObserver>> StallObserver::Create(zx_duration_mono_t threshold,
                                                                 zx_duration_mono_t window,
                                                                 EventReceiver *event_receiver) {
  if (window <= 0 || threshold <= 0 || threshold > window) {
    return zx::error(ZX_ERR_INVALID_ARGS);
  }

  size_t samples_size = (window + kStallSampleInterval - 1) / kStallSampleInterval;

  fbl::AllocChecker ac;
  fbl::Array<zx_duration_mono_t> samples = fbl::MakeArray<zx_duration_mono_t>(&ac, samples_size);
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  ktl::unique_ptr<StallObserver> result{
      new (&ac) StallObserver(threshold, event_receiver, ktl::move(samples))};
  if (!ac.check()) {
    return zx::error(ZX_ERR_NO_MEMORY);
  }

  return zx::ok(ktl::move(result));
}

StallObserver::StallObserver(zx_duration_mono_t threshold, EventReceiver *event_receiver,
                             fbl::Array<zx_duration_mono_t> samples)
    : threshold_(threshold), event_receiver_(event_receiver), samples_(ktl::move(samples)) {}

void StallObserver::PushSample(zx_duration_mono_t sample) {
  samples_sum_ += sample - samples_[samples_pos_];
  samples_[samples_pos_++] = sample;
  if (samples_pos_ == samples_.size()) {
    samples_pos_ = 0;
  }

  if (samples_sum_ >= threshold_) {
    event_receiver_->OnAboveThreshold();
  } else {
    event_receiver_->OnBelowThreshold();
  }
}

StallAggregator::Stats StallAggregator::ReadStats() const {
  Guard<CriticalMutex> guard{&stats_lock_};
  return stats_;
}

void StallAggregator::AddObserverSome(StallObserver *observer) {
  Guard<CriticalMutex> guard{&observers_lock_};
  observers_some_.push_back(observer);
}

void StallAggregator::RemoveObserverSome(StallObserver *observer) {
  Guard<CriticalMutex> guard{&observers_lock_};
  observers_some_.erase(*observer);
}

void StallAggregator::AddObserverFull(StallObserver *observer) {
  Guard<CriticalMutex> guard{&observers_lock_};
  observers_full_.push_back(observer);
}

void StallAggregator::RemoveObserverFull(StallObserver *observer) {
  Guard<CriticalMutex> guard{&observers_lock_};
  observers_full_.erase(*observer);
}

void StallAggregator::SampleOnce(
    fit::inline_function<void(PerCpuStatsCallback)> iterate_per_cpu_stats) {
  // Aggregate stats from all CPUs.
  struct {
    zx_duration_mono_t weighted_some = 0;
    zx_duration_mono_t weighted_full = 0;
    zx_duration_mono_t total_weight = 0;
  } totals;
  iterate_per_cpu_stats([&totals](const StallAccumulator::Stats &stats) {
    totals.weighted_some += stats.total_time_stall_some * stats.total_time_active;
    totals.weighted_full += stats.total_time_stall_full * stats.total_time_active;
    totals.total_weight += stats.total_time_active;
  });

  // Compute weighted average.
  zx_duration_mono_t delta_some, delta_full;
  if (totals.total_weight != 0) {
    delta_some = totals.weighted_some / totals.total_weight;
    delta_full = totals.weighted_full / totals.total_weight;
  } else {
    delta_some = 0;
    delta_full = 0;
  }

  // Update stored stats.
  {
    Guard<CriticalMutex> guard{&stats_lock_};
    stats_.stalled_time_some += delta_some;
    stats_.stalled_time_full += delta_full;
  }

  // Notify observers.
  {
    Guard<CriticalMutex> guard{&observers_lock_};
    for (StallObserver &observer : observers_some_) {
      observer.PushSample(delta_some);
    }
    for (StallObserver &observer : observers_full_) {
      observer.PushSample(delta_full);
    }
  }
}

void StallAggregator::IteratePerCpuStats(PerCpuStatsCallback callback) {
  percpu::ForEach([callback = std::move(callback)](cpu_num_t cpu_num, percpu *cpu_data) {
    StallAccumulator &cpu_accum = cpu_data->memory_stall_accumulator;
    StallAccumulator::Stats stats = cpu_accum.Flush();
    callback(stats);
  });
}

void StallAggregator::StartSamplingThread(uint level) {
  auto worker_thread = [](void *) {
    StallAggregator *aggregator = GetStallAggregator();
    zx_instant_mono_t deadline = current_mono_time();

    for (;;) {
      aggregator->SampleOnce();

      deadline += kStallSampleInterval;
      Thread::Current::Sleep(deadline);
    }

    return 0;
  };

  Thread *t = Thread::Create("stall-aggregator", worker_thread, nullptr, LOW_PRIORITY);
  ZX_ASSERT(t != nullptr);
  t->DetachAndResume();
}

LK_INIT_HOOK(stall, StallAggregator::StartSamplingThread, LK_INIT_LEVEL_USER)
