// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_JOB_WATCHER_H_
#define SRC_PERFORMANCE_CPU_PROFILER_JOB_WATCHER_H_

#include <lib/async/cpp/wait.h>
#include <lib/async/dispatcher.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <zircon/types.h>

#include <optional>
#include <utility>

namespace profiler {
/// A class which monitors a job using task_create_exception_channel to listen for new processes
/// created by the job.
class JobWatcher {
 public:
  explicit JobWatcher(zx::unowned_job job, fit::function<void(zx_koid_t pid, zx::process)> handler)
      : job_(std::move(job)), handler_(std::move(handler)) {}

  // Begin monitoring the watched job for new threads. Each new thread will be paused until the
  // handler has been invoked and completed.
  zx::result<> Watch(async_dispatcher_t* dispatcher);

 private:
  void HandleException(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                       const zx_packet_signal_t* signal);

  zx::unowned_job job_;
  zx::channel exception_channel_;
  fit::function<void(zx_koid_t pid, zx::process)> handler_;
  std::optional<async::WaitMethod<JobWatcher, &JobWatcher::HandleException>> wait_;
};
}  // namespace profiler
#endif  // SRC_PERFORMANCE_CPU_PROFILER_JOB_WATCHER_H_
