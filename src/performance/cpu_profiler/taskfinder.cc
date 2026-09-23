// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "taskfinder.h"

#include <lib/trace/event.h>
#include <lib/zx/job.h>
#include <lib/zx/process.h>
#include <lib/zx/thread.h>

zx::result<TaskFinder::FoundTasks> TaskFinder::FindHandles() {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  auto status = WalkRootJobTree();
  if (status != ZX_OK) {
    return zx::error(status);
  }
  return zx::ok(std::move(found_tasks_));
}
// Each of these methods visits the corresponding task type. If any On*()
// method returns a value other than ZX_OK, the enumeration stops. See
// |task_callback_t| for a description of parameters.
zx_status_t TaskFinder::OnJob(int depth, zx_handle_t job, zx_koid_t koid, zx_koid_t parent_koid) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (jobs_.find(koid) != jobs_.end()) {
    zx::job dup;
    zx_status_t result = zx::unowned_job(job)->duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);
    if (result != ZX_OK) {
      return result;
    }
    found_tasks_.jobs.emplace_back(koid, std::move(dup));
  }
  return ZX_OK;
}

zx_status_t TaskFinder::OnProcess(int depth, zx_handle_t process, zx_koid_t koid,
                                  zx_koid_t parent_koid) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (processes_.find(koid) != processes_.end()) {
    zx::process dup;
    zx_status_t result = zx::unowned_process(process)->duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);
    if (result != ZX_OK) {
      return result;
    }
    found_tasks_.processes.emplace_back(koid, std::move(dup));
  }
  return ZX_OK;
}

zx_status_t TaskFinder::OnThread(int depth, zx_handle_t process, zx_koid_t koid,
                                 zx_koid_t parent_koid) {
  TRACE_DURATION("cpu_profiler", __PRETTY_FUNCTION__);
  if (threads_.find(koid) != threads_.end()) {
    zx::thread dup;
    zx_status_t result = zx::unowned_thread(process)->duplicate(ZX_RIGHT_SAME_RIGHTS, &dup);
    if (result != ZX_OK) {
      return result;
    }
    found_tasks_.threads.emplace_back(koid, std::move(dup));
  }
  return ZX_OK;
}
