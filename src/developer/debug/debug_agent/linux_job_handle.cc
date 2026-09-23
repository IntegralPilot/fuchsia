// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/linux_job_handle.h"

#include <zircon/status.h>

#include "src/developer/debug/debug_agent/linux_process_handle.h"
#include "src/developer/debug/debug_agent/linux_task.h"
#include "src/developer/debug/debug_agent/linux_utils.h"

namespace debug_agent {

std::unique_ptr<JobHandle> LinuxJobHandle::Duplicate() const {
  return std::make_unique<LinuxJobHandle>();
}

std::vector<std::unique_ptr<ProcessHandle>> LinuxJobHandle::GetChildProcesses() const {
  std::vector<std::unique_ptr<ProcessHandle>> result;
  for (int pid : linux::GetDirectoryPids())
    result.push_back(std::make_unique<LinuxProcessHandle>(fxl::MakeRefCounted<LinuxTask>(pid)));
  return result;
}

debug::Status LinuxJobHandle::Attach(JobExceptionObserver* observer, AttachConfig config) {
  return debug::Status("No jobs on Linux");
}

}  // namespace debug_agent
