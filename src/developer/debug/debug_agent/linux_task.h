// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_TASK_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_TASK_H_

#include <sys/wait.h>

#include <optional>

#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/status.h"
#include "src/lib/fxl/memory/ref_counted.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace debug_agent {

class LinuxBinaryLauncher;
class LinuxExceptionHandle;
class LinuxSuspendHandle;
class LinuxTaskObserver;
class ProcessHandleObserver;

// Represents a thread/process on Linux for which ptrace is involed.
//
// Threads and processes aren't really different on Linux. Threads are special-cases of processes
// that share address space, and the main thread is indistinguishable from the process.
//
// To implement our process/thread split, the process and thread handles must share the underlying
// handle. In addition, exceptions and suspend tokens need to refer to the pthread information in
// a way that's independent of the ThreadHandle or ProcessHandle.
//
// This reference-counted object can be referenced by all the necessary places and collects the
// state.
class LinuxTask final : public fxl::RefCountedThreadSafe<LinuxTask> {
 public:
  int pid() const { return pid_; }

  // Registers/unregisters this task with ptrace for getting exceptions. Attach() and Detach() can
  // be called multiple times without side effects.
  bool is_attached() const { return is_attached_; }
  debug::Status Attach();
  void Detach();

  // Sets the observer. Callers will also want to call Attach first to get events.
  // Null means no observer.
  //
  // There can be multiple LinuxTask objects for a process since the debug agent expects that
  // Thread/ProcessHandles are like Zircon "handles." There should only be one task with an observer
  // set on it, or there will be failures. Normally this will correspond to a DebuggedProcess
  // so maintaining a unique observer across all LinuxTasks with a given PID is trivial.
  void SetObserver(LinuxTaskObserver* observer);

  void set_single_step(bool ss) { single_step_ = ss; }

  bool is_suspended() const { return suspend_count_ > 0; }
  int exit_code() const { return exit_code_; }

 private:
  FRIEND_REF_COUNTED_THREAD_SAFE(LinuxTask);
  FRIEND_MAKE_REF_COUNTED(LinuxTask);

  // Encapsulates everything from an event delivery. We sometimes need to re-post these so anything
  // that must be handled as part of the event (like the PTRACE_GETEVENTMSG) must be encapsulated
  // here.
  //
  // Some code might be simpler if we broke out some of the fields of the event status into
  // separate fields.
  struct SignalRecord {
    int pid = 0;
    int status = 0;

    // PTRACE_GETEVENTMSG if it applies to this event.
    std::optional<unsigned long> event_msg;

    // Populate for stop events. But this can still be missing when the process is killed.
    std::optional<siginfo_t> siginfo;
  };

  friend LinuxBinaryLauncher;
  friend LinuxExceptionHandle;
  friend LinuxSuspendHandle;

  // Depending on how a task is created, it might already be attached via ptrace. If so, set
  // is_attached to prevent re-attempting to attach.
  explicit LinuxTask(uint64_t pid, bool is_attached = false);
  ~LinuxTask();

  void OnPidReady(int pid, int status);

  void DispatchEvent(const SignalRecord& record);

  void OnExec(const SignalRecord& record);
  void OnClone(const SignalRecord& record);
  void OnFork(const SignalRecord& record);
  void OnVFork(const SignalRecord& reocr);
  void OnExit(const SignalRecord& record);
  void OnSignaled(const SignalRecord& record);
  void OnStopped(const SignalRecord& record);
  void OnContinued();

  // Constucts a SignalRecord with the given information and any relevant side-channel information
  // from ptrace. This should only be called when handling a ptrace event.
  SignalRecord MakeSignalRecord(int pid, int status);

  // For LinuxSuspendHandle. See suspend_count_.
  void IncrementSuspendCount();
  void DecrementSuspendCount();

  int pid_ = 0;

  bool is_attached_ = false;

  // Handle for watching the process exceptions.
  debug::MessageLoop::WatchHandle process_watch_handle_;

  LinuxTaskObserver* observer_ = nullptr;

  // This task maintains the reference count of the number of live LinuxSuspendHandles. When this
  // count is positive, the task is suspended, when 0 it is allowed to run. Modify only via
  // Increment/DecrementSuspendCount().
  int suspend_count_ = 0;

  bool single_step_ = false;

  bool exiting_ = false;
  int exit_code_ = -1;

  fxl::WeakPtrFactory<LinuxTask> weak_factory_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_TASK_H_
