// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_THREAD_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_THREAD_HANDLE_H_

#include <zircon/types.h>

#include <optional>

#include "src/developer/debug/debug_agent/arch.h"
#include "src/developer/debug/debug_agent/linux_task.h"
#include "src/developer/debug/debug_agent/thread_handle.h"

namespace debug_agent {

// The details of a Linux process/thread is stored in a "Task". This object wraps a Task and
// implements the ThreadHandle functions for it.
class LinuxThreadHandle final : public ThreadHandle {
 public:
  explicit LinuxThreadHandle(fxl::RefPtr<LinuxTask> task);

  // ThreadHandle implementation.
  const NativeThreadHandle& GetNativeHandle() const override { return task_; }
  NativeThreadHandle& GetNativeHandle() override { return task_; }
  zx_koid_t GetKoid() const override { return task_->pid(); }
  std::string GetName() const override;
  State GetState() const override;
  debug_ipc::ThreadRecord GetThreadRecord(zx_koid_t process_koid) const override;
  std::unique_ptr<SuspendHandle> Suspend() override;
  bool WaitForSuspension(TickTimePoint deadline) const override;
  std::optional<GeneralRegisters> GetGeneralRegisters() const override;
  void SetGeneralRegisters(const GeneralRegisters& regs) override;
  std::optional<DebugRegisters> GetDebugRegisters() const override;
  bool SetDebugRegisters(const DebugRegisters& regs) override;
  void SetSingleStep(bool single_step) override;
  std::vector<debug::RegisterValue> ReadRegisters(
      const std::vector<debug::RegisterCategory>& cats_to_get) const override;
  std::vector<debug::RegisterValue> WriteRegisters(
      const std::vector<debug::RegisterValue>& regs) override;
  bool InstallHWBreakpoint(uint64_t address) override;
  bool UninstallHWBreakpoint(uint64_t address) override;
  std::optional<WatchpointInfo> InstallWatchpoint(debug_ipc::BreakpointType type,
                                                  const debug::AddressRange& range) override;
  bool UninstallWatchpoint(const debug::AddressRange& range) override;

 private:
  fxl::RefPtr<LinuxTask> task_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_THREAD_HANDLE_H_
