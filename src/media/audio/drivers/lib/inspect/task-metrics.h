// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_TASK_METRICS_H_
#define SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_TASK_METRICS_H_

#include <lib/zx/clock.h>
#include <lib/zx/profile.h>
#include <lib/zx/thread.h>
#include <lib/zx/time.h>

#include "string_buffer.h"

namespace audio {

// Collects thread-duration metrics during a time-sensitive Task. Can be aggregated/accumulated.
class Subtask {
 public:
  // Statistics about this task.
  struct Metrics {
    Metrics() = default;
    explicit Metrics(std::string_view metrics_name) : Metrics() { name.Append(metrics_name); }

    // Accumulate. Used when maintaining a standalone Subtask::Metrics struct for sum total.
    Metrics& operator+=(const Metrics& rhs) {
      wall_time += rhs.wall_time;
      got_detailed_thread_metrics = got_detailed_thread_metrics || rhs.got_detailed_thread_metrics;
      cpu_time += rhs.cpu_time;
      queue_time += rhs.queue_time;
      page_fault_time += rhs.page_fault_time;
      kernel_lock_contention_time += rhs.kernel_lock_contention_time;
      return *this;
    }

    static constexpr size_t kMaxNameLength = 127;
    StringBuffer<kMaxNameLength> name;         // as a StringBuffer, to avoid heap allocations
    zx::time start_time;                       // wall-clock time when this sub-task started
    zx::duration wall_time;                    // elapsed wall-clock time while this sub-task ran
    bool got_detailed_thread_metrics = false;  // indicates whether the durations below are valid
    zx::duration cpu_time;                     // see zx_info_task_runtime.cpu_time
    zx::duration queue_time;                   // see zx_info_task_runtime.queue_time
    zx::duration page_fault_time;              // see zx_info_task_runtime.page_fault_time
    zx::duration
        kernel_lock_contention_time;  // see zx_info_task_runtime.kernel_lock_contention_time
  };

  // Creates a new task.
  explicit Subtask(std::string_view name, bool collect_thread_metrics = false);

  // Starts the task, and returns a warning message if any (else string will be empty).
  std::string Start();

  // Signals the end of the task. Must be called before retrieving stats with `FinalMetrics`.
  // Elapsed wall-clock time is always captured, but the returned bool indicated whether additional
  // thread_info durations are captured as well.
  bool Done();

  // Report the current accumulated metrics. Cannot be called before `Done()`.
  // Even if 'Done()' returns false, the returned struct still contains `start_time` and `wall_time`
  const Metrics& FinalMetrics() const {
    ZX_ASSERT_MSG(!running_, "Task is still running.");
    return metrics_;
  }

  Subtask(const Subtask&) = delete;
  Subtask& operator=(const Subtask&) = delete;
  Subtask(Subtask&&) = delete;
  Subtask& operator=(Subtask&&) = delete;

 private:
  struct StartInfo {
    zx_status_t status;
    zx_info_task_runtime_t info;
  };

  zx::unowned_thread thread_ = zx::thread::self();
  bool running_ = true;
  StartInfo start_;
  Metrics metrics_;
  bool collect_thread_metrics_;
};

}  // namespace audio

#endif  // SRC_MEDIA_AUDIO_DRIVERS_LIB_INSPECT_TASK_METRICS_H_
