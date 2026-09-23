// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_BUFFER_TRACKER_H_
#define SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_BUFFER_TRACKER_H_

#include <lib/inspect/cpp/inspect.h>
#include <lib/inspect/cpp/vmo/types.h>
#include <lib/zx/clock.h>
#include <lib/zx/time.h>
#include <zircon/compiler.h>

#include <mutex>
#include <optional>
#include <queue>

namespace audio {
// Represents all statistics for a named buffer type.
class BufferTracker {
 public:
  BufferTracker(inspect::Node node, std::optional<uint32_t> max_buffer_count,
                std::optional<zx::duration> per_buffer_duration = std::nullopt);

  void RecordSubmission();
  void RecordCompletion();
  // Should be called once a pipeline is "primed" with initial data, before streaming begins.
  void StartMonitoringOutstandingBufferCount() {
    std::scoped_lock lock(mutex_);
    started_monitoring_buffer_levels_ = true;
    currently_monitoring_buffer_levels_ = true;
  }
  // Should be called once a pipeline is no longer streaming, before it "drains".
  void StopMonitoringOutstandingBufferCount() {
    std::scoped_lock lock(mutex_);
    currently_monitoring_buffer_levels_ = false;
  }

 private:
  inspect::Node node_;
  inspect::LazyNode buffer_tracker_node_;
  std::mutex mutex_;

  bool started_monitoring_buffer_levels_ __TA_GUARDED(mutex_) = false;
  bool currently_monitoring_buffer_levels_ __TA_GUARDED(mutex_) = false;

  // Total buffer counts
  inspect::UintProperty buffers_processed_count_prop_;
  uint64_t buffers_processed_count_ __TA_GUARDED(mutex_) = 0;
  std::optional<zx::duration> per_buffer_duration_ __TA_GUARDED(mutex_);

  // Outstanding buffer metrics.
  uint64_t outstanding_buffer_count_cumulative_ __TA_GUARDED(mutex_) = 0;
  uint64_t outstanding_buffers_count_min_ __TA_GUARDED(mutex_) = UINT64_MAX;
  uint64_t outstanding_buffers_count_max_ __TA_GUARDED(mutex_) = 0;
  std::queue<zx::time> submission_times_ __TA_GUARDED(mutex_);

  // Processing time metrics.
  uint64_t processing_total_duration_us_ __TA_GUARDED(mutex_) = 0;
  zx::duration processing_max_episode_duration_ __TA_GUARDED(mutex_) =
      zx::duration::infinite_past();

  // Empty buffer metrics.
  uint64_t empty_buffer_count_ __TA_GUARDED(mutex_) = 0;
  zx::time empty_buffer_start_time_ __TA_GUARDED(mutex_) = zx::time(0);
  zx::duration empty_buffer_total_duration_ __TA_GUARDED(mutex_) = zx::duration(0);
  zx::duration empty_buffer_max_episode_duration_ __TA_GUARDED(mutex_) =
      zx::duration::infinite_past();

  // Full buffer metrics.
  std::optional<uint32_t> max_buffer_count_;
  uint64_t full_buffer_count_ __TA_GUARDED(mutex_) = 0;
  zx::time full_buffer_start_time_ __TA_GUARDED(mutex_) = zx::time(0);
  zx::duration full_buffer_total_duration_ __TA_GUARDED(mutex_) = zx::duration(0);
  zx::duration full_buffer_max_episode_duration_ __TA_GUARDED(mutex_) =
      zx::duration::infinite_past();
};

}  // namespace audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_BUFFER_TRACKER_H_
