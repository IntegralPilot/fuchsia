// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ZIRCON_JOB_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ZIRCON_JOB_HANDLE_H_

#include <lib/zx/job.h>

#include "src/developer/debug/debug_agent/job_exception_observer.h"
#include "src/developer/debug/debug_agent/job_handle.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/zircon_exception_watcher.h"

namespace debug_agent {

class ZirconJobHandle final : public JobHandle, public debug::ZirconExceptionWatcher {
 public:
  explicit ZirconJobHandle(zx::job j);
  ZirconJobHandle(const ZirconJobHandle& other);
  ZirconJobHandle(ZirconJobHandle&& other) = default;

  // JobHandle implementation.
  std::unique_ptr<JobHandle> Duplicate() const override;
  zx_koid_t GetKoid() const override { return job_koid_; }
  std::string GetName() const override;
  std::vector<std::unique_ptr<JobHandle>> GetChildJobs() const override;
  std::vector<std::unique_ptr<ProcessHandle>> GetChildProcesses() const override;
  debug::Status Attach(JobExceptionObserver* observer, AttachConfig config) override;

 private:
  // ZirconExceptionWatcher implementation.
  void OnJobException(zx::exception exception_token, zx_exception_info_t exception_info) override;

  zx_koid_t job_koid_;
  zx::job job_;

  debug::MessageLoop::WatchHandle job_watch_handle_;
  JobExceptionObserver* exception_observer_ = nullptr;
  JobExceptionChannelType exception_channel_type_ = JobExceptionChannelType::kNone;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_ZIRCON_JOB_HANDLE_H_
