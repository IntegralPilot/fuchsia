// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_PROCESS_WATCHER_H_
#define SRC_PERFORMANCE_CPU_PROFILER_PROCESS_WATCHER_H_

#include <lib/async-loop/cpp/loop.h>
#include <lib/async/cpp/wait.h>
#include <lib/fit/function.h>
#include <lib/zx/channel.h>
#include <lib/zx/process.h>
#include <lib/zx/result.h>
#include <lib/zx/thread.h>
#include <zircon/types.h>

#include <utility>

namespace profiler {
/// A class which monitors a process using task_create_exception_channel to listen for new threads
/// created by the process.
class ProcessWatcher {
 public:
  explicit ProcessWatcher(
      zx::unowned_process process,
      fit::function<void(zx_koid_t pid, zx_koid_t tid, zx::thread)> start_handler,
      fit::function<void(zx_koid_t pid, zx_koid_t tid)> exit_handler)
      : process_(std::move(process)),
        start_handler_(std::move(start_handler)),
        exit_handler_(std::move(exit_handler)) {}

  // Begin monitoring the watched process for new threads. Each new thread will be paused until the
  // handler has been invoked and completed.
  zx::result<> Watch(async_dispatcher_t* dispatcher);

 private:
  void HandleException(async_dispatcher_t* dispatcher, async::WaitBase* wait, zx_status_t status,
                       const zx_packet_signal_t* signal);

  zx::unowned_process process_;
  zx::channel exception_channel_;
  fit::function<void(zx_koid_t pid, zx_koid_t tid, zx::thread)> start_handler_;
  fit::function<void(zx_koid_t pid, zx_koid_t tid)> exit_handler_;
  std::optional<async::WaitMethod<ProcessWatcher, &ProcessWatcher::HandleException>> wait_;
};
}  // namespace profiler

#endif  // SRC_PERFORMANCE_CPU_PROFILER_PROCESS_WATCHER_H_
