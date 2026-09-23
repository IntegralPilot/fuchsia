// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_JOB_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_JOB_HANDLE_H_

#include "src/developer/debug/debug_agent/job_handle.h"

namespace debug_agent {

// Linux doesn't have jobs so this object just stores the "ps" listing to expose it in a
// Fuchsia-like interface.
class LinuxJobHandle : public JobHandle {
 public:
  LinuxJobHandle() = default;
  ~LinuxJobHandle() = default;

  // JobHandle implementation.
  std::unique_ptr<JobHandle> Duplicate() const override;
  zx_koid_t GetKoid() const override { return 0; }
  std::string GetName() const override { return "<root>"; }
  std::vector<std::unique_ptr<JobHandle>> GetChildJobs() const override { return {}; }
  std::vector<std::unique_ptr<ProcessHandle>> GetChildProcesses() const override;
  debug::Status Attach(JobExceptionObserver* observer, AttachConfig config) override;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_JOB_HANDLE_H_
