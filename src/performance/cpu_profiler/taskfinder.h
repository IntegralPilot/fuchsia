// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_CPU_PROFILER_TASKFINDER_H_
#define SRC_PERFORMANCE_CPU_PROFILER_TASKFINDER_H_

#include <lib/zx/handle.h>
#include <lib/zx/result.h>
#include <lib/zx/task.h>

#include <set>
#include <vector>

#include <task-utils/walker.h>

// A Wrapper Implementation of TaskEnumerator that allows us to specify a list of handles to look
// for based on koids.
class TaskFinder : TaskEnumerator {
 public:
  struct FoundTasks {
    std::vector<std::pair<zx_koid_t, zx::job>> jobs;
    std::vector<std::pair<zx_koid_t, zx::process>> processes;
    std::vector<std::pair<zx_koid_t, zx::thread>> threads;

    bool empty() const { return jobs.empty() && processes.empty() && threads.empty(); }
  };

  zx::result<FoundTasks> FindHandles();
  bool Empty() { return jobs_.empty() && processes_.empty() && threads_.empty(); }
  // Each of these methods visits the corresponding task type. If any On*()
  // method returns a value other than ZX_OK, the enumeration stops. See
  // |task_callback_t| for a description of parameters.
  zx_status_t OnJob(int depth, zx_handle_t job, zx_koid_t koid, zx_koid_t parent_koid) override;
  zx_status_t OnProcess(int depth, zx_handle_t process, zx_koid_t koid,
                        zx_koid_t parent_koid) override;
  zx_status_t OnThread(int depth, zx_handle_t process, zx_koid_t koid,
                       zx_koid_t parent_koid) override;
  TaskFinder() = default;
  ~TaskFinder() override = default;

  void AddJob(zx_koid_t job) { jobs_.insert(job); }
  void AddProcess(zx_koid_t process) { processes_.insert(process); }
  void AddThread(zx_koid_t thread) { threads_.insert(thread); }

 protected:
  bool has_on_job() const override { return true; }
  bool has_on_process() const override { return true; }
  bool has_on_thread() const override { return true; }

 private:
  std::set<zx_koid_t> jobs_;
  std::set<zx_koid_t> threads_;
  std::set<zx_koid_t> processes_;

  FoundTasks found_tasks_;
};

#endif  // SRC_PERFORMANCE_CPU_PROFILER_TASKFINDER_H_
