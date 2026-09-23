// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be found in the LICENSE file.

#include "src/media/audio/drivers/lib/inspect/buffer-tracker.h"

#include <lib/driver/logging/cpp/logger.h>

#include "src/media/audio/drivers/lib/inspect/recorder.h"

namespace audio {

BufferTracker::BufferTracker(inspect::Node node, std::optional<uint32_t> max_buffer_count,
                             std::optional<zx::duration> per_buffer_duration)
    : node_(std::move(node)),

      per_buffer_duration_(per_buffer_duration),
      max_buffer_count_(max_buffer_count) {
  buffers_processed_count_prop_ = node_.CreateUint(kBuffersProcessedCount, 0);

  // We don't auto-populate ALL Inspect fields; some have meaning only after some initial event.
  // Until then, those fields offer no value to someone perusing Inspect for important information.

  buffer_tracker_node_ =
      node_.CreateLazyValues(kBufferAccounting, [this]() -> fpromise::promise<inspect::Inspector> {
        std::scoped_lock lock(mutex_);
        inspect::Inspector inspector;

        // These fields only have value after we start processing buffers.
        if (buffers_processed_count_ > 0) {
          // Outstanding buffer accounting
          inspector.GetRoot().CreateDouble(
              kOutstandingBuffersCountAvg,
              static_cast<double>(outstanding_buffer_count_cumulative_) /
                  static_cast<double>(buffers_processed_count_),
              &inspector);
          // These only have value if we started monitoring.
          if (started_monitoring_buffer_levels_) {
            inspector.GetRoot().CreateUint(kOutstandingBuffersCountMin,
                                           outstanding_buffers_count_min_, &inspector);
            inspector.GetRoot().CreateUint(kOutstandingBuffersCountMax,
                                           outstanding_buffers_count_max_, &inspector);
          }
          // Empty buffer episode accounting
          inspector.GetRoot().CreateUint(kEmptyBufferEpisodesCount, empty_buffer_count_,
                                         &inspector);
          if (empty_buffer_count_ > 0) {
            inspector.GetRoot().CreateUint(kEmptyBufferDurationMaxUsec,
                                           empty_buffer_max_episode_duration_.to_usecs(),
                                           &inspector);
            inspector.GetRoot().CreateUint(kEmptyBufferDurationCumulativeUsec,
                                           empty_buffer_total_duration_.to_usecs(), &inspector);
          }
          // Full buffer episode accounting
          if (max_buffer_count_.has_value()) {
            inspector.GetRoot().CreateUint(kFullBufferEpisodesCount, full_buffer_count_,
                                           &inspector);
            if (full_buffer_count_ > 0) {
              inspector.GetRoot().CreateUint(kFullBufferDurationMaxUsec,
                                             full_buffer_max_episode_duration_.to_usecs(),
                                             &inspector);
              inspector.GetRoot().CreateUint(kFullBufferDurationCumulativeUsec,
                                             full_buffer_total_duration_.to_usecs(), &inspector);
            }
          }
          // Processing-time accounting
          inspector.GetRoot().CreateUint(kProcessingTimeAvgUsec,
                                         processing_total_duration_us_ / buffers_processed_count_,
                                         &inspector);
          inspector.GetRoot().CreateUint(kProcessingTimeMaxUsec,
                                         processing_max_episode_duration_.to_usecs(), &inspector);
          // This only has value if the caller specified a buffer duration.
          if (per_buffer_duration_.has_value()) {
            inspector.GetRoot().CreateUint(
                kBuffersProcessedDurationCumulativeUsec,
                buffers_processed_count_ * per_buffer_duration_->to_nsecs() / 1000, &inspector);
          }
        }
        return fpromise::make_ok_promise(inspector);
      });
}

void BufferTracker::RecordSubmission() {
  std::scoped_lock lock(mutex_);
  if (submission_times_.size() >= max_buffer_count_.value_or(UINT_MAX)) {
    fdf::warn("RecordSubmission: active count ({}) cannot equal/exceed max_buffer_count_ ({})",
              submission_times_.size(), max_buffer_count_.value_or(UINT_MAX));
    return;
  }
  auto submission_time = zx::clock::get_monotonic();
  if (submission_times_.empty()) {
    // If an empty-buffer episode STARTED while we were monitoring buffer levels, we count it even
    // if we are now no longer monitoring buffer levels.
    if (empty_buffer_start_time_.get() != 0) {
      const zx::duration duration = submission_time - empty_buffer_start_time_;
      empty_buffer_total_duration_ += duration;
      empty_buffer_count_++;
      if (duration > empty_buffer_max_episode_duration_) {
        empty_buffer_max_episode_duration_ = duration;
      }
      empty_buffer_start_time_ = zx::time(0);
    }
  }
  submission_times_.push(submission_time);
  // Don't track min/max outstanding buffers when initially "priming" or "draining" the pipeline.
  if (currently_monitoring_buffer_levels_) {
    outstanding_buffers_count_max_ =
        std::max(outstanding_buffers_count_max_, submission_times_.size());
    // Start... or StopMonitoringOutstandingBufferCount could be called at any time.
    // For full accuracy, account for the outstanding buffer count immediately prior to submission.
    outstanding_buffers_count_min_ =
        std::min(outstanding_buffers_count_min_, submission_times_.size() - 1);

    if (max_buffer_count_.has_value() && submission_times_.size() == max_buffer_count_.value()) {
      if (full_buffer_start_time_.get() == 0) {
        full_buffer_start_time_ = submission_time;
      }
    }
  }
}

void BufferTracker::RecordCompletion() {
  std::scoped_lock lock(mutex_);
  auto completion_time = zx::clock::get_monotonic();
  if (submission_times_.empty()) {
    fdf::warn("RecordCompletion: no active buffers to complete.");
    return;
  }
  if (max_buffer_count_.has_value() && submission_times_.size() == max_buffer_count_.value()) {
    // If a full-buffer episode STARTED while we were monitoring buffer levels, we count it even if
    // we are now no longer monitoring buffer levels.
    if (full_buffer_start_time_.get() != 0) {
      const zx::duration duration = completion_time - full_buffer_start_time_;
      full_buffer_total_duration_ += duration;
      full_buffer_count_++;
      if (duration > full_buffer_max_episode_duration_) {
        full_buffer_max_episode_duration_ = duration;
      }
      full_buffer_start_time_ = zx::time(0);
    }
  }
  outstanding_buffer_count_cumulative_ += submission_times_.size();
  zx::time submission_time = submission_times_.front();
  submission_times_.pop();

  // Don't track min/max outstanding buffers when "draining" the pipeline.
  if (currently_monitoring_buffer_levels_) {
    outstanding_buffers_count_min_ =
        std::min(outstanding_buffers_count_min_, submission_times_.size());
    // Start... or StopMonitoringOutstandingBufferCount could be called at any time.
    // For full accuracy, account for the outstanding buffer count immediately prior to completion.
    outstanding_buffers_count_max_ =
        std::max(outstanding_buffers_count_max_, submission_times_.size() + 1);

    if (submission_times_.empty()) {
      if (empty_buffer_start_time_.get() == 0) {
        empty_buffer_start_time_ = completion_time;
      }
    }
  }

  zx::duration processing_time = completion_time - submission_time;
  processing_total_duration_us_ += processing_time.to_usecs();
  buffers_processed_count_++;
  buffers_processed_count_prop_.Set(buffers_processed_count_);

  if (processing_time > processing_max_episode_duration_) {
    processing_max_episode_duration_ = processing_time;
  }
}

}  // namespace audio
