// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/mock_job_handle.h"

#include "src/developer/debug/debug_agent/job_exception_channel_type.h"
#include "src/developer/debug/debug_agent/mock_process_handle.h"

namespace debug_agent {

MockJobHandle::MockJobHandle(zx_koid_t koid, std::string name)
    : job_koid_(koid), name_(std::move(name)) {}

std::unique_ptr<JobHandle> MockJobHandle::Duplicate() const {
  return std::make_unique<MockJobHandle>(*this);
}

std::vector<std::unique_ptr<JobHandle>> MockJobHandle::GetChildJobs() const {
  // Need to return a unique set of objects every time so make copies.
  std::vector<std::unique_ptr<JobHandle>> result;
  for (auto& job : child_jobs_)
    result.push_back(std::make_unique<MockJobHandle>(job));
  return result;
}

std::vector<std::unique_ptr<ProcessHandle>> MockJobHandle::GetChildProcesses() const {
  // Need to return a unique set of objects every time so make copies.
  std::vector<std::unique_ptr<ProcessHandle>> result;
  for (auto& process : child_processes_)
    result.push_back(std::make_unique<MockProcessHandle>(process));
  return result;
}

MockJobHandle* MockJobHandle::FindChildJob(zx_koid_t job_koid) {
  if (GetKoid() == job_koid) {
    return this;
  }

  for (auto& job : child_jobs_) {
    if (auto found = job.FindChildJob(job_koid)) {
      return found;
    }
  }

  return nullptr;
}

void MockJobHandle::OnException(std::unique_ptr<MockExceptionHandle> exception,
                                MockJobExceptionInfo info) {
  switch (info) {
    case MockJobExceptionInfo::kProcessStarting:
      FX_CHECK(observer_type_ == JobExceptionChannelType::kDebugger);
      observer_->OnProcessStarting(exception->GetProcessHandle(), std::move(exception));
      break;
    case MockJobExceptionInfo::kProcessNameChanged:
      FX_CHECK(observer_type_ == JobExceptionChannelType::kDebugger);
      observer_->OnProcessNameChanged(exception->GetProcessHandle(), std::move(exception));
      break;
    case MockJobExceptionInfo::kException:
      FX_CHECK(observer_type_ == JobExceptionChannelType::kException);
      observer_->OnUnhandledException(std::move(exception));
      break;
  }
}
}  // namespace debug_agent
