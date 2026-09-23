// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/media/audio/drivers/lib/inspect/task-metrics.h"

namespace audio {

Subtask::Subtask(std::string_view name, bool collect_thread_metrics)
    : collect_thread_metrics_(collect_thread_metrics) {
  metrics_.name.Append(name);
}

std::string Subtask::Start() {
  // Start running the timer.
  metrics_.start_time = zx::clock::get_monotonic();

  std::string warning_message;
  if (collect_thread_metrics_) {
    start_.status = thread_->get_info(ZX_INFO_TASK_RUNTIME, &start_.info, sizeof(start_.info),
                                      nullptr, nullptr);
    if (start_.status != ZX_OK) {
      // Because `get_info` failed, we will not gather ZX_INFO_TASK_RUNTIME durations.
      warning_message = std::format("ZX_INFO_TASK_RUNTIME failed with status {}", start_.status);
    }
  }
  return warning_message;
}

bool Subtask::Done() {
  ZX_ASSERT_MSG(running_, "Task already stopped running.");
  running_ = false;

  metrics_.got_detailed_thread_metrics = false;
  // Stop the timer.
  // We can always return "wall-clock" durations even if the `zx::thread::get_info` call fails.
  metrics_.wall_time = zx::clock::get_monotonic() - metrics_.start_time;
  if (start_.status != ZX_OK) {
    return false;  // We previously failed and cannot return ZX_INFO_TASK_RUNTIME durations.
  }

  if (collect_thread_metrics_) {
    zx_info_task_runtime_t end_info;
    auto end_status =
        thread_->get_info(ZX_INFO_TASK_RUNTIME, &end_info, sizeof(end_info), nullptr, nullptr);
    if (end_status != ZX_OK) {
      return false;  // `get_info` just failed; we cannot return ZX_INFO_TASK_RUNTIME durations.
    }

    metrics_.got_detailed_thread_metrics = true;
    metrics_.cpu_time += zx::nsec(end_info.cpu_time - start_.info.cpu_time);
    metrics_.queue_time += zx::nsec(end_info.queue_time - start_.info.queue_time);
    metrics_.page_fault_time += zx::nsec(end_info.page_fault_time - start_.info.page_fault_time);
    metrics_.kernel_lock_contention_time +=
        zx::nsec(end_info.lock_contention_time - start_.info.lock_contention_time);
  }
  return true;
}

}  // namespace audio
