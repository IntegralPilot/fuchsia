// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_EXCEPTION_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_EXCEPTION_HANDLE_H_

#include <signal.h>

#include "src/developer/debug/debug_agent/exception_handle.h"
#include "src/lib/fxl/macros.h"
#include "src/lib/fxl/memory/ref_counted.h"

namespace debug_agent {

class LinuxTask;

class LinuxExceptionHandle : public ExceptionHandle {
 public:
  // This constructor assumes the thread is already stopped on behalf of this class. The destructor
  // will resume the thread, but the constructor does nothing.
  LinuxExceptionHandle(fxl::RefPtr<LinuxTask> task, const siginfo_t& info);

  // This contructor creates thread start/exit exceptions. There is no siginfo_t since it doesn't
  // correspond to a real Linux exception.
  LinuxExceptionHandle(debug_ipc::ExceptionType type, fxl::RefPtr<LinuxTask> thread);

  ~LinuxExceptionHandle() override;

  fxl::RefPtr<LinuxTask>& task() { return task_; }

  // ExceptionHandle implementation:
  std::unique_ptr<ThreadHandle> GetThreadHandle() const override;
  std::unique_ptr<ProcessHandle> GetProcessHandle() const override;
  debug_ipc::ExceptionType GetType(const ThreadHandle& thread) const override;
  fit::result<debug::Status, Resolution> GetResolution() const override;
  debug::Status SetResolution(Resolution resolution) override;
  fit::result<debug::Status, debug_ipc::ExceptionStrategy> GetStrategy() const override;
  debug::Status SetStrategy(debug_ipc::ExceptionStrategy strategy) override;
  debug_ipc::ExceptionRecord GetRecord() const override;
  uint64_t GetPid() const override;
  uint64_t GetTid() const override;

 private:
  // Moveable but not copyable.
  LinuxExceptionHandle(const LinuxExceptionHandle&) = delete;
  LinuxExceptionHandle(LinuxExceptionHandle&&) = default;

  fxl::RefPtr<LinuxTask> task_;
  debug_ipc::ExceptionType type_;
  siginfo_t info_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_EXCEPTION_HANDLE_H_
