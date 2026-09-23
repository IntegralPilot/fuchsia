// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_DEBUGGED_JOB_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_DEBUGGED_JOB_H_

#include "gtest/gtest_prod.h"
#include "lib/syslog/cpp/macros.h"
#include "src/developer/debug/debug_agent/job_exception_channel_type.h"
#include "src/developer/debug/debug_agent/job_exception_observer.h"
#include "src/developer/debug/debug_agent/job_handle.h"
#include "src/developer/debug/ipc/records.h"
#include "src/lib/fxl/macros.h"

namespace debug_agent {

class DebugAgent;
class DebuggedProcess;

struct DebuggedJobCreateInfo {
  explicit DebuggedJobCreateInfo(std::unique_ptr<JobHandle> handle);

  std::unique_ptr<JobHandle> handle;

  // See documentation in AttachConfig.
  debug_ipc::AttachConfig::Priority priority = debug_ipc::AttachConfig::Priority::kStrong;
};

class DebuggedJob : public JobExceptionObserver {
 public:
  explicit DebuggedJob(DebugAgent* debug_agent);
  virtual ~DebuggedJob() = default;

  // Actually bind to the given exception channel of this job to begin watching for exceptions or
  // notifications. See above for descriptions of which channel will yield which types of
  // exceptions. This class is not usable unless this method has returned an OK status.
  debug::Status Init(DebuggedJobCreateInfo&& info);

  const JobHandle& job_handle() const {
    FX_DCHECK(job_handle_);
    return *job_handle_;
  }

  JobHandle& job_handle() {
    FX_DCHECK(job_handle_);
    return *job_handle_;
  }

  DebugAgent* debug_agent() const { return debug_agent_; }

  zx_koid_t koid() const { return job_handle_->GetKoid(); }

  JobExceptionChannelType type() const { return type_; }

 private:
  // For access to the private JobExceptionObserver implementation below.
  FRIEND_TEST(DebuggedJob, ReportUnhandledException);

  // JobExceptionObserver implementation.
  void OnProcessStarting(std::unique_ptr<ProcessHandle> process,
                         std::unique_ptr<ExceptionHandle> exception) override;
  void OnProcessNameChanged(std::unique_ptr<ProcessHandle> process,
                            std::unique_ptr<ExceptionHandle> exception) override;
  void OnUnhandledException(std::unique_ptr<ExceptionHandle> exception) override;

  std::unique_ptr<JobHandle> job_handle_ = nullptr;

  JobExceptionChannelType type_;

  DebugAgent* debug_agent_ = nullptr;

  FXL_DISALLOW_COPY_AND_ASSIGN(DebuggedJob);
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_DEBUGGED_JOB_H_
