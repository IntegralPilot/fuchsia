// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_RECORDER_H_
#define SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_RECORDER_H_

#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>

#include <memory>

#include "src/media/audio/drivers/lib/inspect/buffer-tracker.h"
#include "src/media/audio/drivers/lib/inspect/task-metrics.h"

namespace audio {

//
// Recorder class and subclasses
// The Recorder class is responsible for creating and updating Inspect -- so we don't need to
// implement this functionality in classes dedicated to other functions.
//

// Use StringReferences to save space in the Inspect VMO.
static constexpr std::string_view kCurrentPowerState = "current_power_state";

static constexpr std::string_view kPowerTransitions = "power_transitions";
static constexpr std::string_view kCalledAt = "called_at";
static constexpr std::string_view kEffectiveAt = "effective_at";
static constexpr std::string_view kPowerState = "power_state";

static constexpr std::string_view kRingBuffers = "RingBuffers";
static constexpr std::string_view kElementId = "element_id";
static constexpr std::string_view kSupportsActiveChannels = "supports_active_channels";
static constexpr std::string_view kIsOutgoingStream = "is_outgoing_stream";
static constexpr std::string_view kCtorTime = "ctor_time";
static constexpr std::string_view kDtorTime = "dtor_time";
static constexpr std::string_view kRunningIntervals = "running_intervals";
static constexpr std::string_view kStartedAtUsec = "started_at_us";
static constexpr std::string_view kStoppedAtUsec = "stopped_at_us";
static constexpr std::string_view kAudioDuration = "audio_duration_us";
static constexpr std::string_view kSetActiveChannelsCalls = "SetActiveChannels_calls";
static constexpr std::string_view kChannelBitmask = "channel_bitmask";

static constexpr std::string_view kDAIs = "DAIs";

static constexpr std::string_view kDiagnosticsSummary = "diagnostics_summary";
static constexpr std::string_view kDiagnostics = "diagnostics";
// TODO(b/458465136): Eliminate unnecessary indirection (task_records/metrics)
static constexpr std::string_view kTaskRecords = "task_records";
static constexpr std::string_view kStartupTaskRecords = "startup_task_records";
static constexpr std::string_view kFinalTaskRecords = "final_task_records";
static constexpr std::string_view kMin = "min";
static constexpr std::string_view kMax = "max";
static constexpr std::string_view kSum = "sum";
static constexpr std::string_view kAvg = "avg";

static constexpr std::string_view kEndToEndIntervalUsec = "task_interval_end_to_end_us";
static constexpr std::string_view kStartToStartIntervalUsec = "task_interval_start_to_start_us";
static constexpr std::string_view kSchedulingDelayUsec = "task_scheduling_delay_us";
static constexpr std::string_view kCpuTimeUsec = "task_time_cpu_us";
static constexpr std::string_view kKernelLockContentionTimeUsec =
    "task_time_kernel_lock_contention_us";
static constexpr std::string_view kPageFaultTimeUsec = "task_time_page_fault_us";
static constexpr std::string_view kQueueTimeUsec = "task_time_queue_us";
static constexpr std::string_view kWallTimeUsec = "task_time_wall_us";

static constexpr std::string_view kCountTasks = "count_tasks";
static constexpr std::string_view kCountDroppedTransfers = "count_dropped_transfers";
static constexpr std::string_view kCountOverruns = "count_overruns";
static constexpr std::string_view kCountUnderruns = "count_underruns";
static constexpr std::string_view kWorstOverrunFrames = "worst_overrun_frames";
static constexpr std::string_view kWorstUnderrunFrames = "worst_underrun_frames";

static constexpr std::string_view kBufferAccounting = "buffer_accounting";
static constexpr std::string_view kBuffersProcessedCount = "buffers_processed_count";
static constexpr std::string_view kBuffersProcessedDurationCumulativeUsec =
    "buffers_processed_duration_us";
static constexpr std::string_view kEmptyBufferDurationMaxUsec = "empty_buffer_duration_max_us";
static constexpr std::string_view kEmptyBufferDurationCumulativeUsec =
    "empty_buffer_duration_sum_us";
static constexpr std::string_view kEmptyBufferEpisodesCount = "empty_buffer_episodes_count";
static constexpr std::string_view kFullBufferDurationMaxUsec = "full_buffer_duration_max_us";
static constexpr std::string_view kFullBufferDurationCumulativeUsec = "full_buffer_duration_sum_us";
static constexpr std::string_view kFullBufferEpisodesCount = "full_buffer_episodes_count";
static constexpr std::string_view kOutstandingBuffersCountAvg = "outstanding_buffers_count_avg";
static constexpr std::string_view kOutstandingBuffersCountMax = "outstanding_buffers_count_max";
static constexpr std::string_view kOutstandingBuffersCountMin = "outstanding_buffers_count_min";
static constexpr std::string_view kProcessingTimeAvgUsec = "processing_time_avg_us";
static constexpr std::string_view kProcessingTimeMaxUsec = "processing_time_max_us";

// Represents a single power transition.
class PowerTransition {
 public:
  PowerTransition(inspect::Node node, bool state, const zx::time& called_at,
                  const zx::time& completed_at);

 private:
  inspect::Node node_;
  inspect::BoolProperty state_;
  inspect::IntProperty called_at_;
  inspect::IntProperty completed_at_;
};

// Represents the specification (unchanging information) of a Dai element.
class DaiEntry {
 public:
  DaiEntry(inspect::Node node, uint64_t element_id);
  inspect::Node& node() { return node_; }

 private:
  inspect::Node node_;
  inspect::UintProperty element_id_;
};

// Represents a call to SetActiveChannels.
class ActiveChannelsCall {
 public:
  ActiveChannelsCall(inspect::Node node, uint64_t channel_mask, const zx::time& called_at,
                     const zx::time& completed_at);

 private:
  inspect::Node node_;
  inspect::UintProperty channel_mask_;
  inspect::IntProperty called_at_;
  inspect::IntProperty completed_at_;
};

struct InterTaskDurations {
  zx::duration start_to_start;
  zx::duration end_to_end;
};

// Record diagnostic info about this streaming session. This includes:
// - the first 5 data-transport tasks: wall, cpu, queue, page, kernel lock times, plus s2s and e2e;
// - the last 25 data-transport tasks: same (s2s / e2e are start-to-start / end-to-end durations);
// - the min and max and sum values for the above as well (except s2s and e2e for sum values);
// - underruns (read too close to the producer): total count/duration, longest single underrun;
// - overruns (read too far ahead of the producer): total count/duration, longest single overrun.
class TaskRecords {
 public:
  TaskRecords(inspect::Node node, size_t max_entry_count)
      : node_(std::move(node)), max_entry_count_(max_entry_count) {
    task_times_entries_ = std::vector<TaskTimes>(max_entry_count_);
  }

  static void UpdateInt(inspect::IntProperty* property, inspect::Node* node, std::string_view name,
                        int64_t value);

  void RecordTaskMetrics(const Subtask::Metrics& metrics,
                         std::optional<InterTaskDurations> inter_task_durations,
                         std::optional<zx::duration> scheduling_delay = std::nullopt);

 private:
  struct TaskTimes {
    std::string name;
    inspect::Node node;
    inspect::IntProperty start_to_start_us;
    inspect::IntProperty end_to_end_us;
    inspect::IntProperty wall_time_us;
    inspect::IntProperty cpu_time_us;
    inspect::IntProperty queue_time_us;
    inspect::IntProperty page_fault_time_us;
    inspect::IntProperty kernel_lock_contention_time_us;
    inspect::IntProperty scheduling_delay_us;
  };

  inspect::Node node_;
  size_t max_entry_count_;
  size_t next_entry_index_ = 0;
  std::vector<TaskTimes> task_times_entries_;
};

class AggregateRecords {
 public:
  AggregateRecords(inspect::Node& node, std::string_view name);

  void RecordTaskMetrics(const Subtask::Metrics& metrics,
                         std::optional<InterTaskDurations> inter_task_durations);

  void RecordTaskUnderrun(int64_t underrun_frames) {
    task_underrun_count_prop_.Add(1);
    worst_underrun_frames_ = std::max(worst_underrun_frames_, underrun_frames);
    if (!worst_underrun_frames_prop_) {
      worst_underrun_frames_prop_ =
          diagnostics_node_.CreateUint(kWorstUnderrunFrames, worst_underrun_frames_);
    } else {
      worst_underrun_frames_prop_.Set(worst_underrun_frames_);
    }
  }

  void RecordTaskOverrun(int64_t overrun_frames) {
    task_overrun_count_prop_.Add(1);
    worst_overrun_frames_ = std::max(worst_overrun_frames_, overrun_frames);
    if (!worst_overrun_frames_prop_) {
      worst_overrun_frames_prop_ =
          diagnostics_node_.CreateUint(kWorstOverrunFrames, worst_overrun_frames_);
    } else {
      worst_overrun_frames_prop_.Set(worst_overrun_frames_);
    }
  }

  void RecordDroppedTransfer() { dropped_transfer_count_prop_.Add(1); }

  void SetupBufferTracker(const std::string& name,
                          std::optional<uint32_t> max_buffer_count = std::nullopt,
                          std::optional<zx::duration> per_buffer_duration = std::nullopt);
  void StartMonitoringOutstandingBufferCount();
  void StopMonitoringOutstandingBufferCount();
  void RecordBufferSubmission();
  void RecordBufferCompletion();

  void SetTaskScheduleInterval(zx::duration interval);

 private:
  inspect::Node diagnostics_node_;
  inspect::Node task_records_node_;
  inspect::Node sum_node_;
  TaskRecords min_task_records_;
  TaskRecords max_task_records_;
  TaskRecords avg_task_records_;
  inspect::UintProperty worst_underrun_frames_prop_;
  inspect::UintProperty worst_overrun_frames_prop_;
  inspect::UintProperty task_count_prop_;
  inspect::UintProperty task_underrun_count_prop_;
  inspect::UintProperty task_overrun_count_prop_;
  inspect::UintProperty dropped_transfer_count_prop_;
  inspect::IntProperty sum_kernel_lock_time_prop_;

  Subtask::Metrics min_metrics_;
  Subtask::Metrics max_metrics_;
  Subtask::Metrics sum_metrics_;
  zx::duration sum_kernel_lock_contention_time_;
  Subtask::Metrics avg_metrics_;
  InterTaskDurations min_inter_task_durations_ = {
      .start_to_start = zx::duration::infinite(),
      .end_to_end = zx::duration::infinite(),
  };
  zx::duration min_schedule_delay_ = zx::duration::infinite();
  InterTaskDurations max_inter_task_durations_ = {
      .start_to_start = zx::duration::infinite_past(),
      .end_to_end = zx::duration::infinite_past(),
  };
  zx::duration max_schedule_delay_ = zx::duration::infinite_past();
  InterTaskDurations sum_inter_task_durations_ = {
      .start_to_start = zx::duration{0},
      .end_to_end = zx::duration{0},
  };
  int64_t total_inter_task_durations_count_ = 0;
  zx::duration sum_schedule_delay_{0};
  size_t total_scheduling_delay_count_ = 0;
  int64_t worst_underrun_frames_ = 0;
  int64_t worst_overrun_frames_ = 0;
  size_t total_task_count_ = 0;
  int64_t total_thread_metrics_count_ = 0;
  std::unique_ptr<BufferTracker> buffer_tracker_;
  std::optional<zx::duration> task_schedule_interval_;
};

// Represents an interval during which a RingBuffer instance is started.
class RunningInterval {
 public:
  RunningInterval(inspect::Node node, const zx::time& started_at, size_t startup_task_count,
                  size_t final_task_count);

  void RecordStopTime(const zx::time& stopped_at);

  void RecordTaskMetrics(const Subtask::Metrics& metrics,
                         std::optional<InterTaskDurations> inter_task_durations);

  AggregateRecords& diagnostics() { return aggregate_records_; }
  inspect::Node& node() { return node_; }

 private:
  inspect::Node node_;
  zx::time started_at_;
  zx::time stopped_at_;
  std::optional<TaskRecords> startup_task_records_;
  std::optional<TaskRecords> final_task_records_;
  size_t startup_tasks_to_save_;
  size_t final_tasks_to_save_;
  AggregateRecords aggregate_records_;
  size_t record_count_ = 0;
};

// One of the primary classes used by an outside class.
// Records info about a ring buffer instance, such as lifetime, start/stop, SetActiveChannels.
class RingBufferSpecification;
class RingBufferRecorder {
 public:
  RingBufferRecorder(RingBufferSpecification* ring_buffer_spec, inspect::Node node,
                     const zx::time& created_at);

  void RecordDestructionTime(const zx::time& destroyed_at);

  void RecordStartTime(const zx::time& started_at);
  void RecordStopTime(const zx::time& stopped_at);

  void SaveStartupAndFinalTaskMetrics(size_t startup_task_save_count, size_t final_task_save_count);
  void RecordTaskMetrics(const Subtask::Metrics& metrics);
  void RecordTaskUnderrun(int64_t underrun_frames);
  void RecordTaskOverrun(int64_t overrun_frames);
  void RecordDroppedTransfer();

  void RecordActiveChannelsCall(uint64_t active_channels_bitmask,
                                const zx::time& set_active_channels_called_at,
                                const zx::time& active_channels_completed_at);

  // On some platforms, the ring buffer might be fed into hardware buffers. These methods enable
  // tracking of metrics associated with such hardware buffers.
  void SetupBufferTracker(const std::string& name,
                          std::optional<uint32_t> max_buffer_count = std::nullopt,
                          std::optional<zx::duration> per_buffer_duration = std::nullopt);
  void RecordBufferSubmission();
  void RecordBufferCompletion();
  // Should be called once a pipeline is "primed" with initial data, before streaming begins.
  void StartMonitoringOutstandingBufferCount();
  // Should be called once a pipeline is no longer streaming, before it "drains".
  void StopMonitoringOutstandingBufferCount();

  // Sets the expected interval between tasks. This is used to calculate
  // scheduling delay metrics.
  void SetTaskScheduleInterval(zx::duration interval);

 private:
  static constexpr size_t kMaxStartupTaskRecords = 5;
  static constexpr size_t kMaxFinalTaskRecords = 25;

  RingBufferSpecification* ring_buffer_spec_;
  inspect::Node instance_node_;
  inspect::IntProperty created_at_;
  inspect::IntProperty destroyed_at_;

  inspect::Node active_channels_calls_node_;
  std::vector<ActiveChannelsCall> active_channels_calls_;

  inspect::Node running_intervals_root_;
  std::vector<std::unique_ptr<RunningInterval>> running_intervals_;
  size_t startup_task_save_count_ = 0;
  size_t final_task_save_count_ = 0;

  std::optional<zx::time> prev_start_time_;
  std::optional<zx::duration> prev_wall_time_;

  std::optional<std::string> buffer_tracker_name_;
  std::optional<uint32_t> buffer_tracker_max_count_;
  std::optional<zx::duration> buffer_tracker_per_buffer_duration_;

  std::optional<zx::duration> task_schedule_interval_;
};

// Represents the specification (unchanging information) of a RingBuffer element.
class RingBufferSpecification {
 public:
  RingBufferSpecification(inspect::Node node, uint64_t element_id, bool supports_active_channels,
                          bool outgoing);

  RingBufferRecorder& CreateRingBufferInspectInstance(const zx::time& created_at) {
    RingBufferRecorder instance(
        this, node_.CreateChild(std::format("instance_{}", ring_buffer_instance_count_++)),
        created_at);

    if (ring_buffer_instance_count_ <= kMaxRingBufferInspectInstances) {
      return ring_buffer_inspect_instances_.emplace_back(std::move(instance));
    }

    // Retain (kMaxRingBufferInspectInstances) most recent instances.
    RingBufferRecorder& existing_slot =
        ring_buffer_inspect_instances_[(ring_buffer_instance_count_ - 1) %
                                       kMaxRingBufferInspectInstances];
    existing_slot = std::move(instance);
    return existing_slot;
  }

  AggregateRecords& aggregate_records() { return aggregate_records_; }
  inspect::Node& node() { return node_; }

 private:
  static constexpr size_t kMaxRingBufferInspectInstances = 20;

  inspect::Node node_;
  inspect::UintProperty element_id_;
  inspect::BoolProperty supports_active_channels_;
  inspect::BoolProperty outgoing_;
  std::vector<RingBufferRecorder> ring_buffer_inspect_instances_;
  size_t ring_buffer_instance_count_ = 0;

  AggregateRecords aggregate_records_;  // min/max/sum for this RingBuffer.
};

// One of the primary classes used by an outside class.
// Records info about a device, such as lifetime, power transitions, and Dai/RingBuffer elements.
class Recorder final {
 public:
  explicit Recorder(inspect::Node& inspect_root);

  void PopulateRingBuffer(const std::string& name, uint64_t element_id,
                          bool supports_active_channels, bool outgoing);
  void PopulateDai(const std::string& name, uint64_t element_id);

  void RecordSocPowerUp(const zx::time& called_at, const zx::time& completed_at);
  void RecordSocPowerDown(const zx::time& called_at, const zx::time& completed_at);

  RingBufferRecorder& CreateRingBufferInstance(uint64_t element_id, const zx::time& created_at) {
    auto it = ring_buffer_specs_.find(element_id);
    ZX_ASSERT_MSG(it != ring_buffer_specs_.end(), "No ring buffer with id %lu.", element_id);
    RingBufferSpecification& ring_buffer_spec = it->second;

    return ring_buffer_spec.CreateRingBufferInspectInstance(created_at);
  }

 private:
  void PopulatePowerNodes();

  inspect::Node& inspect_root_;
  inspect::BoolProperty current_power_state_;

  inspect::Node power_transitions_node_;
  std::vector<PowerTransition> power_transitions_;

  inspect::Node ring_buffers_root_node_;
  std::unordered_map<uint64_t, RingBufferSpecification> ring_buffer_specs_;

  inspect::Node dai_root_node_;
  std::vector<DaiEntry> dai_entries_;
};

}  // namespace audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_RECORDER_H_
