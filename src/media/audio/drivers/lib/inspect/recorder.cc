// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/lib/inspect/recorder.h"

#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

namespace audio {

PowerTransition::PowerTransition(inspect::Node node, bool state, const zx::time& called_at,
                                 const zx::time& completed_at)
    : node_(std::move(node)) {
  state_ = node_.CreateBool(kPowerState, state);
  called_at_ = node_.CreateInt(kCalledAt, called_at.get());
  completed_at_ = node_.CreateInt(kEffectiveAt, completed_at.get());
}

DaiEntry::DaiEntry(inspect::Node node, uint64_t element_id) : node_(std::move(node)) {
  element_id_ = node_.CreateUint(kElementId, element_id);
}

ActiveChannelsCall::ActiveChannelsCall(inspect::Node node, uint64_t channel_mask,
                                       const zx::time& called_at, const zx::time& completed_at)
    : node_(std::move(node)) {
  channel_mask_ = node_.CreateUint(kChannelBitmask, channel_mask);
  called_at_ = node_.CreateInt(kCalledAt, called_at.get());
  completed_at_ = node_.CreateInt(kEffectiveAt, completed_at.get());
}

void TaskRecords::UpdateInt(inspect::IntProperty* property, inspect::Node* node,
                            std::string_view name, const int64_t value) {
  ZX_ASSERT_MSG(property != nullptr, "Invalid task node");
  if (*property) {
    property->Set(value);
  } else {
    ZX_ASSERT_MSG(node != nullptr, "Invalid task node");
    *property = node->CreateInt(name, value);
  }
}

void TaskRecords::RecordTaskMetrics(const Subtask::Metrics& metrics,
                                    std::optional<InterTaskDurations> inter_task_durations,
                                    std::optional<zx::duration> scheduling_delay) {
  if (task_times_entries_.size() == 0) {
    return;
  }
  auto& task_times = task_times_entries_[next_entry_index_];
  next_entry_index_ = (next_entry_index_ + 1) % max_entry_count_;

  if (!task_times.node || metrics.name != std::string_view(task_times.name)) {
    task_times = {};  // reset all inspect objects.
    task_times.node = node_.CreateChild(metrics.name);
    task_times.name = metrics.name;
  }

  if (inter_task_durations.has_value()) {
    UpdateInt(&task_times.start_to_start_us, &task_times.node, kStartToStartIntervalUsec,
              inter_task_durations->start_to_start.to_usecs());
    UpdateInt(&task_times.end_to_end_us, &task_times.node, kEndToEndIntervalUsec,
              inter_task_durations->end_to_end.to_usecs());
  }
  if (scheduling_delay.has_value() && scheduling_delay->to_usecs() > 0) {
    UpdateInt(&task_times.scheduling_delay_us, &task_times.node, kSchedulingDelayUsec,
              scheduling_delay->to_usecs());
  }
  UpdateInt(&task_times.wall_time_us, &task_times.node, kWallTimeUsec,
            metrics.wall_time.to_usecs());

  if (metrics.got_detailed_thread_metrics) {
    UpdateInt(&task_times.cpu_time_us, &task_times.node, kCpuTimeUsec, metrics.cpu_time.to_usecs());
    UpdateInt(&task_times.queue_time_us, &task_times.node, kQueueTimeUsec,
              metrics.queue_time.to_usecs());
    UpdateInt(&task_times.page_fault_time_us, &task_times.node, kPageFaultTimeUsec,
              metrics.page_fault_time.to_usecs());
    // Don't bother to publish kernel_lock_contention_time to Inspect if it is zero.
    if (metrics.kernel_lock_contention_time.to_nsecs() > 0) {
      UpdateInt(&task_times.kernel_lock_contention_time_us, &task_times.node,
                kKernelLockContentionTimeUsec, metrics.kernel_lock_contention_time.to_usecs());
    }
  }
}

AggregateRecords::AggregateRecords(inspect::Node& node, std::string_view name)
    : diagnostics_node_(node.CreateChild(name)),
      task_records_node_(diagnostics_node_.CreateChild(kTaskRecords)),
      sum_node_(task_records_node_.CreateChild(kSum)),
      min_task_records_(task_records_node_.CreateChild(kMin), 1),
      max_task_records_(task_records_node_.CreateChild(kMax), 1),
      avg_task_records_(task_records_node_.CreateChild(kAvg), 1) {
  task_count_prop_ = diagnostics_node_.CreateUint(kCountTasks, 0);
  task_underrun_count_prop_ = diagnostics_node_.CreateUint(kCountUnderruns, 0);
  task_overrun_count_prop_ = diagnostics_node_.CreateUint(kCountOverruns, 0);
  dropped_transfer_count_prop_ = diagnostics_node_.CreateUint(kCountDroppedTransfers, 0);

  // TODO(b/458465136): Eliminate the unnecessary Node indirection (task_records/metrics)
  min_metrics_ = Subtask::Metrics{kMin};
  min_metrics_.wall_time = zx::duration::infinite();
  min_metrics_.cpu_time = zx::duration::infinite();
  min_metrics_.queue_time = zx::duration::infinite();
  min_metrics_.page_fault_time = zx::duration::infinite();
  min_metrics_.kernel_lock_contention_time = zx::duration::infinite();
  max_metrics_ = Subtask::Metrics{kMax};
  sum_metrics_ = Subtask::Metrics{kSum};
  avg_metrics_ = Subtask::Metrics{kAvg};
}

void AggregateRecords::RecordTaskMetrics(const Subtask::Metrics& metrics,
                                         std::optional<InterTaskDurations> inter_task_durations) {
  total_task_count_++;
  task_count_prop_.Set(total_task_count_);

  // Get the min and max values for every field, including start-to-start and end-to-end deltas.
  // If `metrics.got_detailed_thread_metrics` is false, we only get basic wall-clock durations.
  min_metrics_.wall_time = std::min(min_metrics_.wall_time, metrics.wall_time);
  max_metrics_.wall_time = std::max(max_metrics_.wall_time, metrics.wall_time);
  sum_metrics_.wall_time += metrics.wall_time;

  if (metrics.got_detailed_thread_metrics) {
    min_metrics_.cpu_time = std::min(min_metrics_.cpu_time, metrics.cpu_time);
    min_metrics_.queue_time = std::min(min_metrics_.queue_time, metrics.queue_time);
    min_metrics_.page_fault_time = std::min(min_metrics_.page_fault_time, metrics.page_fault_time);
    min_metrics_.kernel_lock_contention_time =
        std::min(min_metrics_.kernel_lock_contention_time, metrics.kernel_lock_contention_time);

    max_metrics_.cpu_time = std::max(max_metrics_.cpu_time, metrics.cpu_time);
    max_metrics_.queue_time = std::max(max_metrics_.queue_time, metrics.queue_time);
    max_metrics_.page_fault_time = std::max(max_metrics_.page_fault_time, metrics.page_fault_time);
    max_metrics_.kernel_lock_contention_time =
        std::max(max_metrics_.kernel_lock_contention_time, metrics.kernel_lock_contention_time);

    // Without this, TaskRecords::RecordTaskMetrics won't cache min/max cpu/queue/page/klock.
    min_metrics_.got_detailed_thread_metrics = true;
    max_metrics_.got_detailed_thread_metrics = true;
    // sum_metrics_.got_detailed_thread_metrics is set implicitly via type conversion
    sum_metrics_ += metrics;
  }

  InterTaskDurations avg_inter_task_durations;
  zx::duration avg_scheduling_delay = zx::duration(0);
  bool has_scheduling_delay = false;
  if (inter_task_durations.has_value()) {
    if (task_schedule_interval_.has_value()) {
      zx::duration schedule_delay =
          inter_task_durations->start_to_start > task_schedule_interval_.value()
              ? inter_task_durations->start_to_start - task_schedule_interval_.value()
              : zx::duration(0);
      min_schedule_delay_ = std::min(min_schedule_delay_, schedule_delay);
      max_schedule_delay_ = std::max(max_schedule_delay_, schedule_delay);
      sum_schedule_delay_ += schedule_delay;
      total_scheduling_delay_count_++;
      avg_scheduling_delay =
          sum_schedule_delay_ / static_cast<int64_t>(total_scheduling_delay_count_);
      has_scheduling_delay = true;
    }

    min_inter_task_durations_.start_to_start =
        std::min(min_inter_task_durations_.start_to_start, inter_task_durations->start_to_start);
    min_inter_task_durations_.end_to_end =
        std::min(min_inter_task_durations_.end_to_end, inter_task_durations->end_to_end);

    max_inter_task_durations_.start_to_start =
        std::max(max_inter_task_durations_.start_to_start, inter_task_durations->start_to_start);
    max_inter_task_durations_.end_to_end =
        std::max(max_inter_task_durations_.end_to_end, inter_task_durations->end_to_end);

    sum_inter_task_durations_.start_to_start += inter_task_durations->end_to_end;
    sum_inter_task_durations_.end_to_end += inter_task_durations->end_to_end;

    total_inter_task_durations_count_++;
    avg_inter_task_durations.start_to_start =
        sum_inter_task_durations_.start_to_start / total_inter_task_durations_count_;
    avg_inter_task_durations.end_to_end =
        sum_inter_task_durations_.end_to_end / total_inter_task_durations_count_;
  }

  avg_metrics_.wall_time = sum_metrics_.wall_time / static_cast<int64_t>(total_task_count_);
  if (sum_metrics_.got_detailed_thread_metrics) {
    total_thread_metrics_count_++;
    avg_metrics_.cpu_time = sum_metrics_.cpu_time / total_thread_metrics_count_;
    avg_metrics_.queue_time = sum_metrics_.queue_time / total_thread_metrics_count_;
    avg_metrics_.page_fault_time = sum_metrics_.page_fault_time / total_thread_metrics_count_;
    // Don't maintain average kernel_lock_contention_time (it will be 0), but record max.
    avg_metrics_.got_detailed_thread_metrics = true;
  }
  sum_kernel_lock_contention_time_ += metrics.kernel_lock_contention_time;

  // Just record min/wall_time, min_start_to_start_, min_end_to_end_
  min_task_records_.RecordTaskMetrics(
      min_metrics_, min_inter_task_durations_,
      has_scheduling_delay ? std::make_optional(min_schedule_delay_) : std::nullopt);

  max_task_records_.RecordTaskMetrics(
      max_metrics_, max_inter_task_durations_,
      has_scheduling_delay ? std::make_optional(max_schedule_delay_) : std::nullopt);

  TaskRecords::UpdateInt(&sum_kernel_lock_time_prop_, &sum_node_, kKernelLockContentionTimeUsec,
                         sum_kernel_lock_contention_time_.to_usecs());

  avg_task_records_.RecordTaskMetrics(
      avg_metrics_, avg_inter_task_durations,
      has_scheduling_delay ? std::make_optional(avg_scheduling_delay) : std::nullopt);
}

void AggregateRecords::SetupBufferTracker(const std::string& name,
                                          std::optional<uint32_t> max_buffer_count,
                                          std::optional<zx::duration> per_buffer_duration) {
  buffer_tracker_ = std::make_unique<BufferTracker>(diagnostics_node_.CreateChild(name),
                                                    max_buffer_count, per_buffer_duration);
}

void AggregateRecords::RecordBufferSubmission() {
  if (buffer_tracker_) {
    buffer_tracker_->RecordSubmission();
  }
}
void AggregateRecords::RecordBufferCompletion() {
  if (buffer_tracker_) {
    buffer_tracker_->RecordCompletion();
  }
}
void AggregateRecords::StartMonitoringOutstandingBufferCount() {
  if (buffer_tracker_) {
    buffer_tracker_->StartMonitoringOutstandingBufferCount();
  }
}
void AggregateRecords::StopMonitoringOutstandingBufferCount() {
  if (buffer_tracker_) {
    buffer_tracker_->StopMonitoringOutstandingBufferCount();
  }
}

void AggregateRecords::SetTaskScheduleInterval(zx::duration interval) {
  task_schedule_interval_ = interval;
}

RunningInterval::RunningInterval(inspect::Node node, const zx::time& started_at,
                                 size_t startup_task_count, size_t final_task_count)
    : node_(std::move(node)),
      startup_tasks_to_save_(startup_task_count),
      final_tasks_to_save_(final_task_count),
      aggregate_records_(node_, kDiagnostics) {
  started_at_ = started_at;
  node_.RecordInt(kStartedAtUsec, started_at.get() / 1000);

  if (startup_tasks_to_save_) {
    startup_task_records_ = TaskRecords(node_.CreateChild(kStartupTaskRecords), startup_task_count);
  }
  if (final_tasks_to_save_) {
    final_task_records_ = TaskRecords(node_.CreateChild(kFinalTaskRecords), final_task_count);
  }
}

void RunningInterval::RecordStopTime(const zx::time& stopped_at) {
  stopped_at_ = stopped_at;
  node_.RecordInt(kStoppedAtUsec, stopped_at.get() / 1000);
  zx::duration audio_duration = stopped_at_ - started_at_;
  node_.RecordInt(kAudioDuration, audio_duration.to_usecs());
}

void RunningInterval::RecordTaskMetrics(const Subtask::Metrics& metrics,
                                        std::optional<InterTaskDurations> inter_task_durations) {
  ++record_count_;
  if (record_count_ <= startup_tasks_to_save_ || final_tasks_to_save_ > 0) {
    std::optional<TaskRecords>& task_records =
        record_count_ <= startup_tasks_to_save_ ? startup_task_records_ : final_task_records_;
    if (task_records.has_value()) {
      task_records->RecordTaskMetrics(metrics, inter_task_durations);
    }
  }
  aggregate_records_.RecordTaskMetrics(metrics, inter_task_durations);
}

RingBufferRecorder::RingBufferRecorder(RingBufferSpecification* ring_buffer_spec,
                                       inspect::Node node, const zx::time& created_at)
    : ring_buffer_spec_(ring_buffer_spec), instance_node_(std::move(node)) {
  created_at_ = instance_node_.CreateInt(kCtorTime, created_at.get());
  running_intervals_root_ = instance_node_.CreateChild(kRunningIntervals);
}

void RingBufferRecorder::RecordDestructionTime(const zx::time& destroyed_at) {
  destroyed_at_ = instance_node_.CreateInt(kDtorTime, destroyed_at.get());
}

// This captures the current startup_task_save_count_ and final_task_save_count_ for this interval.
void RingBufferRecorder::RecordStartTime(const zx::time& started_at) {
  auto running_interval = std::make_unique<RunningInterval>(
      running_intervals_root_.CreateChild(std::to_string(running_intervals_.size())), started_at,
      startup_task_save_count_, final_task_save_count_);
  if (buffer_tracker_name_.has_value()) {
    running_interval->diagnostics().SetupBufferTracker(
        *buffer_tracker_name_, buffer_tracker_max_count_, buffer_tracker_per_buffer_duration_);
  }
  if (task_schedule_interval_.has_value()) {
    running_interval->diagnostics().SetTaskScheduleInterval(task_schedule_interval_.value());
  }
  running_intervals_.emplace_back(std::move(running_interval));
  prev_start_time_ = std::nullopt;
  prev_wall_time_ = std::nullopt;
}

void RingBufferRecorder::RecordStopTime(const zx::time& stopped_at) {
  // It's pointless for clients to call Stop before Start, but we shouldn't crash if they do.
  if (!running_intervals_.empty()) {
    running_intervals_.back()->RecordStopTime(stopped_at);
  }
}

// Set the values that are captured into a RunningInterval at creation (upon RecordStartTime call).
// Can be called multiple times; does not affect a currently-active RunningInterval.
void RingBufferRecorder::SaveStartupAndFinalTaskMetrics(size_t startup_task_save_count,
                                                        size_t final_task_save_count) {
  startup_task_save_count_ = std::min(startup_task_save_count, kMaxStartupTaskRecords);
  final_task_save_count_ = std::min(final_task_save_count, kMaxFinalTaskRecords);
}

void RingBufferRecorder::RecordTaskMetrics(const Subtask::Metrics& metrics) {
  std::optional<InterTaskDurations> it_durations = std::nullopt;
  if (prev_start_time_.has_value() && prev_wall_time_.has_value()) {
    zx::duration start_to_start = metrics.start_time - prev_start_time_.value();
    zx::duration end_to_end = start_to_start + metrics.wall_time - prev_wall_time_.value();
    it_durations = InterTaskDurations{
        .start_to_start = start_to_start,
        .end_to_end = end_to_end,
    };
  }

  ring_buffer_spec_->aggregate_records().RecordTaskMetrics(metrics, it_durations);
  if (!running_intervals_.empty()) {
    running_intervals_.back()->RecordTaskMetrics(metrics, it_durations);
  }

  prev_start_time_ = metrics.start_time;
  prev_wall_time_ = metrics.wall_time;
}

void RingBufferRecorder::RecordTaskUnderrun(int64_t underrun_frames) {
  ring_buffer_spec_->aggregate_records().RecordTaskUnderrun(underrun_frames);
  if (!running_intervals_.empty()) {
    running_intervals_.back()->diagnostics().RecordTaskUnderrun(underrun_frames);
  }
}

void RingBufferRecorder::RecordTaskOverrun(int64_t overrun_frames) {
  ring_buffer_spec_->aggregate_records().RecordTaskOverrun(overrun_frames);
  if (!running_intervals_.empty()) {
    running_intervals_.back()->diagnostics().RecordTaskOverrun(overrun_frames);
  }
}

void RingBufferRecorder::RecordDroppedTransfer() {
  ring_buffer_spec_->aggregate_records().RecordDroppedTransfer();
  if (!running_intervals_.empty()) {
    running_intervals_.back()->diagnostics().RecordDroppedTransfer();
  }
}

void RingBufferRecorder::RecordActiveChannelsCall(uint64_t active_channels_bitmask,
                                                  const zx::time& set_active_channels_called_at,
                                                  const zx::time& active_channels_completed_at) {
  if (!active_channels_calls_node_) {
    active_channels_calls_node_ = instance_node_.CreateChild(kSetActiveChannelsCalls);
  }

  ActiveChannelsCall active_channels_call{
      active_channels_calls_node_.CreateChild(std::to_string(active_channels_calls_.size())),
      active_channels_bitmask, set_active_channels_called_at, active_channels_completed_at};
  active_channels_calls_.emplace_back(std::move(active_channels_call));
}

void RingBufferRecorder::SetupBufferTracker(const std::string& name,
                                            std::optional<uint32_t> max_buffer_count,
                                            std::optional<zx::duration> per_buffer_duration) {
  buffer_tracker_name_ = name;
  buffer_tracker_max_count_ = max_buffer_count;
  buffer_tracker_per_buffer_duration_ = per_buffer_duration;

  ring_buffer_spec_->aggregate_records().SetupBufferTracker(name, max_buffer_count,
                                                            per_buffer_duration);
}

void RingBufferRecorder::RecordBufferSubmission() {
  ring_buffer_spec_->aggregate_records().RecordBufferSubmission();
  if (!running_intervals_.empty()) {
    running_intervals_.back()->diagnostics().RecordBufferSubmission();
  }
}
void RingBufferRecorder::RecordBufferCompletion() {
  ring_buffer_spec_->aggregate_records().RecordBufferCompletion();
  if (!running_intervals_.empty()) {
    running_intervals_.back()->diagnostics().RecordBufferCompletion();
  }
}
void RingBufferRecorder::StartMonitoringOutstandingBufferCount() {
  ring_buffer_spec_->aggregate_records().StartMonitoringOutstandingBufferCount();
  if (!running_intervals_.empty()) {
    running_intervals_.back()->diagnostics().StartMonitoringOutstandingBufferCount();
  }
}
void RingBufferRecorder::StopMonitoringOutstandingBufferCount() {
  ring_buffer_spec_->aggregate_records().StopMonitoringOutstandingBufferCount();
  if (!running_intervals_.empty()) {
    running_intervals_.back()->diagnostics().StopMonitoringOutstandingBufferCount();
  }
}

void RingBufferRecorder::SetTaskScheduleInterval(zx::duration interval) {
  task_schedule_interval_ = interval;
  ring_buffer_spec_->aggregate_records().SetTaskScheduleInterval(interval);
}

RingBufferSpecification::RingBufferSpecification(inspect::Node node, uint64_t element_id,
                                                 bool supports_active_channels, bool outgoing)
    : node_(std::move(node)), aggregate_records_(node_, kDiagnosticsSummary) {
  element_id_ = node_.CreateUint(kElementId, element_id);
  supports_active_channels_ = node_.CreateBool(kSupportsActiveChannels, supports_active_channels);
  outgoing_ = node_.CreateBool(kIsOutgoingStream, outgoing);

  ring_buffer_inspect_instances_.reserve(kMaxRingBufferInspectInstances);
}

Recorder::Recorder(inspect::Node& inspect_root) : inspect_root_(inspect_root) {
  ring_buffers_root_node_ = inspect_root_.CreateChild(kRingBuffers);
  dai_root_node_ = inspect_root_.CreateChild(kDAIs);
}

void Recorder::PopulatePowerNodes() {
  current_power_state_ = inspect_root_.CreateBool(kCurrentPowerState, true);
  power_transitions_node_ = inspect_root_.CreateChild(kPowerTransitions);
}

void Recorder::PopulateRingBuffer(const std::string& name, uint64_t element_id,
                                  bool supports_active_channels, bool outgoing) {
  auto ring_buffer_spec_node = ring_buffers_root_node_.CreateChild(name);
  RingBufferSpecification ring_buffer_spec{std::move(ring_buffer_spec_node), element_id,
                                           supports_active_channels, outgoing};
  ring_buffer_specs_.emplace(element_id, std::move(ring_buffer_spec));
}

void Recorder::PopulateDai(const std::string& name, uint64_t element_id) {
  auto dai_node = dai_root_node_.CreateChild(name);
  DaiEntry dai_entry{std::move(dai_node), element_id};
  dai_entries_.emplace_back(std::move(dai_entry));
}

void Recorder::RecordSocPowerUp(const zx::time& called_at, const zx::time& completed_at) {
  if (!current_power_state_) {
    PopulatePowerNodes();
  }

  current_power_state_.Set(true);
  power_transitions_.emplace_back(
      power_transitions_node_.CreateChild(std::to_string(power_transitions_.size())), true,
      called_at, completed_at);
}

void Recorder::RecordSocPowerDown(const zx::time& called_at, const zx::time& completed_at) {
  if (!current_power_state_) {
    PopulatePowerNodes();
  }

  current_power_state_.Set(false);
  power_transitions_.emplace_back(
      power_transitions_node_.CreateChild(std::to_string(power_transitions_.size())), false,
      called_at, completed_at);
}

}  // namespace audio
