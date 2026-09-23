// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MOCK_JOB_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MOCK_JOB_HANDLE_H_

#include "src/developer/debug/debug_agent/job_handle.h"
#include "src/developer/debug/debug_agent/mock_exception_handle.h"
#include "src/developer/debug/debug_agent/mock_process_handle.h"

namespace debug_agent {

enum class MockJobExceptionInfo {
  kProcessStarting,
  kProcessNameChanged,
  kException,
};

class MockJobHandle final : public JobHandle {
 public:
  explicit MockJobHandle(zx_koid_t koid, std::string name = std::string());

  // Sets the child jobs and processes. These will be copied since we need to return a new
  // unique_ptr for each call to GetChildJobs() / GetChildProcesses().
  void set_child_jobs(std::vector<MockJobHandle> jobs) { child_jobs_ = std::move(jobs); }
  void set_child_processes(std::vector<MockProcessHandle> processes) {
    for (auto& p : processes)
      p.set_job_koid(job_koid_);
    child_processes_ = std::move(processes);
  }

  // Simulate a job exception, typically zircon would provide an exception_info struct that lets us
  // determine which JobExceptionObserver method to call, for the purposes of this mock, the caller
  // decides. Since in Zircon, it isn't possible to receive e.g. process starting notifications
  // without being subscribed to the "Debugger" exception channel, and similarly, to receive an
  // exception without being subscribed to the "normal" exception channel, this function will assert
  // if an invalid info is passed to this method with an observer that previously registered with
  // the incorrect job exception channel.
  JobExceptionObserver* observer() const { return observer_; }
  JobExceptionChannelType observer_type() const { return observer_type_; }

  void OnException(std::unique_ptr<MockExceptionHandle> exception, MockJobExceptionInfo info);

  // JobHandle implementation.
  std::unique_ptr<JobHandle> Duplicate() const override;
  zx_koid_t GetKoid() const override { return job_koid_; }
  std::string GetName() const override { return name_; }
  std::vector<std::unique_ptr<JobHandle>> GetChildJobs() const override;
  std::vector<std::unique_ptr<ProcessHandle>> GetChildProcesses() const override;
  debug::Status Attach(JobExceptionObserver* observer, AttachConfig config) override {
    observer_ = observer;
    observer_type_ = config.exception_channel_type;
    return debug::Status();
  }

  void AddChildJob(MockJobHandle job) { child_jobs_.push_back(std::move(job)); }

  void AddChildProcess(MockProcessHandle process) {
    child_processes_.push_back(std::move(process));
  }

  // Returns a mutable pointer to the job with the given |job_koid|.
  MockJobHandle* FindChildJob(zx_koid_t job_koid);

 private:
  zx_koid_t job_koid_;
  std::string name_;

  JobExceptionObserver* observer_;
  JobExceptionChannelType observer_type_;

  std::vector<MockJobHandle> child_jobs_;
  std::vector<MockProcessHandle> child_processes_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_MOCK_JOB_HANDLE_H_
