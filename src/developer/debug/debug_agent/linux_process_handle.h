// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_PROCESS_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_PROCESS_HANDLE_H_

#include <fbl/unique_fd.h>

#include "src/developer/debug/debug_agent/linux_task.h"
#include "src/developer/debug/debug_agent/linux_task_observer.h"
#include "src/developer/debug/debug_agent/process_handle.h"
#include "src/developer/debug/shared/message_loop.h"

namespace debug_agent {

// The details of a Linux process/thread is stored in a "Task". This object wraps a Task and
// implements the ProcessHandle functions for it.
class LinuxProcessHandle final : public ProcessHandle, public LinuxTaskObserver {
 public:
  // Prototypes for the callbacks to WriteMemoryByWord().
  using ReadWordFunc = debug::Status (*)(pid_t pid, uintptr_t word_address, uintptr_t* data_out);
  using WriteWordFunc = debug::Status (*)(pid_t pid, uintptr_t word_address, uintptr_t data);

  explicit LinuxProcessHandle(fxl::RefPtr<LinuxTask> task);
  ~LinuxProcessHandle() override;

  // ProcessHandle implementation.
  const NativeProcessHandle& GetNativeHandle() const override { return task_; }
  NativeProcessHandle& GetNativeHandle() override { return task_; }
  zx_koid_t GetKoid() const override { return task_->pid(); }
  std::string GetName() const override;
  std::vector<std::unique_ptr<ThreadHandle>> GetChildThreads() const override;
  zx_koid_t GetJobKoid() const override { return 0; }
  debug::Status Kill() override;
  int64_t GetReturnCode() const override { return task_->exit_code(); }
  debug::Status Attach(ProcessHandleObserver* observer, AttachConfig config) override;
  bool IsAttached() const override;
  void Detach() override;
  uint64_t GetLoaderBreakpointAddress() override;
  std::vector<debug_ipc::AddressRegion> GetAddressSpace(uint64_t address) const override;
  std::optional<debug_ipc::AddressRegion> GetSharedAddressSpace() const override;
  std::vector<debug_ipc::Module> GetModules() const override;
  fit::result<debug::Status, std::vector<debug_ipc::InfoHandle>> GetHandles() const override;
  debug::Status ReadMemory(uintptr_t address, void* buffer, size_t len,
                           size_t* actual) const override;
  debug::Status WriteMemory(uintptr_t address, const void* buffer, size_t len,
                            size_t* actual) override;
  std::vector<debug_ipc::MemoryBlock> ReadMemoryBlocks(uint64_t address,
                                                       uint32_t size) const override;
  debug::Status SaveMinidump(const std::vector<DebuggedThread*>& threads,
                             std::vector<uint8_t>* core_data) override;

  // Writes a byte buffer using word-aligned read/write functions.
  //
  // In use, WriteMemory() will call this and provide read/write implementations via ptrace. But
  // this structure allows us to more easily unit test the edge cases in process.
  static debug::Status WriteMemoryByWord(pid_t pid, uintptr_t address, const void* buffer,
                                         size_t len, ReadWordFunc read_fn, WriteWordFunc write_fn,
                                         size_t* actual);

 private:
  // LinuxTaskObserver implementat.
  void OnExited(LinuxTask* task, std::unique_ptr<LinuxExceptionHandle> exception) override;
  void OnProcessStarting(std::unique_ptr<LinuxExceptionHandle> exception) override;
  void OnThreadStarting(std::unique_ptr<LinuxExceptionHandle> exception) override;
  void OnTermSignal(int pid, int signal_number) override;
  void OnStopSignal(LinuxTask* task, std::unique_ptr<LinuxExceptionHandle> exception) override;
  void OnContinued(int pid) override;

  fxl::RefPtr<LinuxTask> task_;

  ProcessHandleObserver* observer_ = nullptr;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_PROCESS_HANDLE_H_
