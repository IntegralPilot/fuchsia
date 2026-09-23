// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/linux_task.h"

#include <fcntl.h>
#include <lib/syslog/cpp/macros.h>
#include <signal.h>
#include <sys/ptrace.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <zircon/errors.h>

#include <linux/unistd.h>

#include "src/developer/debug/debug_agent/linux_exception_handle.h"
#include "src/developer/debug/debug_agent/linux_suspend_handle.h"
#include "src/developer/debug/debug_agent/linux_task_observer.h"
#include "src/developer/debug/debug_agent/linux_utils.h"
#include "src/developer/debug/shared/logging/logging.h"
#include "src/developer/debug/shared/message_loop_linux.h"

namespace debug_agent {

namespace {

const long kPtraceOpts = PTRACE_O_TRACEEXIT | PTRACE_O_TRACECLONE | PTRACE_O_TRACEEXEC |
                         PTRACE_O_TRACEFORK | PTRACE_O_TRACEVFORK;

// Checks if the given status code from waitpid() is the given ptrace event.
bool IsPtraceEvent(int status, int ptrace_event_type) {
  return status >> 8 == (SIGTRAP | (ptrace_event_type << 8));
}

// Returns true for all messages that can be used with PTRACE_GETEVENTMSG.
bool HasEventMessage(int status) {
  return IsPtraceEvent(status, PTRACE_EVENT_CLONE) || IsPtraceEvent(status, PTRACE_EVENT_EXIT) ||
         IsPtraceEvent(status, PTRACE_EVENT_FORK) || IsPtraceEvent(status, PTRACE_EVENT_SECCOMP) ||
         IsPtraceEvent(status, PTRACE_EVENT_VFORK) ||
         IsPtraceEvent(status, PTRACE_EVENT_VFORK_DONE);
}

// Calls the tkill() syscall which has no library wrapper.
//
// Note: tgkill() is supposed to be better than tkill() to avoid shutdown races, but since we're
// attached to the PID, it shouldn't be recycled which will prevent this race. Calling tgkill
// requires that we know the main thread's ID which we don't currently know from the task. So
// we use tkill().
int tkill(int tid, int signo) { return syscall(__NR_tkill, tid, signo); }

// Debugging aid for waitpid status values.
std::string WaitpidStatusToString(int status) {
  std::stringstream out;
  out << "status=0x" << std::hex << status << std::dec << ", ";
  if (WIFEXITED(status)) {
    out << "EXITED, exit status=" << WEXITSTATUS(status);
  } else if (WIFSIGNALED(status)) {
    out << "SIGNALED";
    if (WCOREDUMP(status))
      out << " CORE DUMP";
    out << ", signal=" << linux::SignalToString(WTERMSIG(status), true);
  } else if (WIFSTOPPED(status)) {
    out << "STOPPED, signal=" << linux::SignalToString(WSTOPSIG(status), true);
  } else if (WIFCONTINUED(status)) {
    out << "CONTINUED";
  } else {
    out << "NOT DECODABLE";
  }

  return out.str();
}

}  // namespace

LinuxTask::LinuxTask(uint64_t pid, bool is_attached)
    : pid_(pid), is_attached_(is_attached), weak_factory_(this) {}

LinuxTask::~LinuxTask() {
  if (is_attached_)
    Detach();
}

debug::Status LinuxTask::Attach() {
  if (is_attached_)
    return debug::Status();

  DEBUG_LOG(Process) << "Attaching to " << pid_;
  if (ptrace(PTRACE_SEIZE, pid_, nullptr, (void*)kPtraceOpts) == -1) {
    DEBUG_LOG(Process) << "PTRACE_SEIZE failed for PID " << pid_ << " errno = " << errno;
    return debug::ErrnoStatus(errno);
  }

  is_attached_ = true;
  return debug::Status();
}

void LinuxTask::Detach() {
  if (!is_attached_)
    return;

  // Linux processes can only be detached during a ptrace-stop so suspend.
  IncrementSuspendCount();
  if (ptrace(PTRACE_DETACH, pid_, nullptr, nullptr) == -1) {
    DEBUG_LOG(Process) << "PTRACE_DETACH failed for PID " << pid_ << " errno = " << errno;
  }
  is_attached_ = false;
  DecrementSuspendCount();
}

void LinuxTask::SetObserver(LinuxTaskObserver* observer) {
  if (observer && !observer_) {
    // Installing observer.
    if (is_attached_) {
      if (ptrace(PTRACE_SETOPTIONS, pid_, nullptr, (void*)kPtraceOpts) == -1) {
        DEBUG_LOG(Process) << "PTRACE_SETOPTIONS failed for PID " << pid_ << " errno = " << errno;
      }
    }

    debug::MessageLoopLinux* loop = debug::MessageLoopLinux::Current();
    process_watch_handle_ =
        loop->WatchChildSignals(pid_, [this](int pid, int status) { OnPidReady(pid, status); });
  } else if (!observer && observer_) {
    // Uninstalling observer.
    process_watch_handle_.StopWatching();
  }
  observer_ = observer;
}

void LinuxTask::OnPidReady(int pid, int status) { DispatchEvent(MakeSignalRecord(pid, status)); }

void LinuxTask::DispatchEvent(const SignalRecord& record) {
  if (IsPtraceEvent(record.status, PTRACE_EVENT_EXEC)) {
    OnExec(record);
  } else if (IsPtraceEvent(record.status, PTRACE_EVENT_CLONE)) {
    OnClone(record);
  } else if (IsPtraceEvent(record.status, PTRACE_EVENT_FORK)) {
    OnFork(record);
  } else if (IsPtraceEvent(record.status, PTRACE_EVENT_VFORK)) {
    OnVFork(record);
  } else if (IsPtraceEvent(record.status, PTRACE_EVENT_EXIT)) {
    OnExit(record);
  } else if (WIFEXITED(record.status)) {
    DEBUG_LOG(Thread) << WaitpidStatusToString(record.status);
    // TODO(brettw) I'm not sure how this differs from PTRACE_EVENT_EXIT. It seems to get sent
    // when we attach to a process and if we treat this as an exit and call OnExit() we'll detach
    // early.
  } else if (WIFSIGNALED(record.status)) {
    OnSignaled(record);
  } else if (WIFSTOPPED(record.status)) {
    OnStopped(record);
  } else if (WIFCONTINUED(record.status)) {
    OnContinued();
  } else {
    // Unexpected wait type.
    DEBUG_LOG(Thread) << "LinuxTask::OnFDReady UNEXPECTED STATUS";
    FX_NOTREACHED();
  }
}

void LinuxTask::OnExec(const SignalRecord& record) {
  DEBUG_LOG(Thread) << "LinuxTask::OnExec PID=" << record.pid;

  suspend_count_++;
  observer_->OnStopSignal(
      this, std::make_unique<LinuxExceptionHandle>(fxl::RefPtr<LinuxTask>(this), siginfo_t()));
}

void LinuxTask::OnClone(const SignalRecord& record) {
  // This is the Linux equivalent of "new thread." The new PID is stored in the event message.
  if (record.event_msg) {
    // Account for the implicit stop during processing, the suspend handle will own the suspension.
    suspend_count_++;
    LinuxSuspendHandle sus(fxl::RefPtr<LinuxTask>(this), LinuxSuspendHandle::AlreadySuspended());

    int new_pid = static_cast<int>(*record.event_msg);
    DEBUG_LOG(Thread) << "LinuxTask::OnClone new PID=" << new_pid;
    FX_DCHECK(pid_ != new_pid);  // Expecting a different PID for the new thread.

    auto new_task = fxl::MakeRefCounted<LinuxTask>(new_pid, true);
    new_task->suspend_count_++;  // Account for the count the LinuxExceptionHandle will take.
    observer_->OnThreadStarting(std::make_unique<LinuxExceptionHandle>(
        debug_ipc::ExceptionType::kThreadStarting, std::move(new_task)));

    // The current thread will also be suspended. The suspend handle going out of scope will
    // resume it.
    DEBUG_LOG(Thread) << "Resuming cloning source thread.";
  } else {
    DEBUG_LOG(Thread) << "LinuxTask::OnClone: GetEventMsg failed.";
  }
}

void LinuxTask::OnFork(const SignalRecord& record) {
  if (record.event_msg) {
    // Account for the implicit stop during processing, the suspend handle will own the suspension.
    suspend_count_++;
    LinuxSuspendHandle sus(fxl::RefPtr<LinuxTask>(this), LinuxSuspendHandle::AlreadySuspended());

    int new_pid = static_cast<int>(*record.event_msg);
    DEBUG_LOG(Thread) << "LinuxTask::OnFork of PID=" << pid() << " new PID=" << new_pid
                      << " suspend count = " << suspend_count_;
    FX_DCHECK(pid_ != new_pid);  // Expecting a different PID for the new thread.

    // We should already be attached to the new process.
    auto new_task = fxl::MakeRefCounted<LinuxTask>(new_pid, true);
    new_task->suspend_count_++;  // Account for the count the LinuxExceptionHandle will take.

    observer_->OnProcessStarting(std::make_unique<LinuxExceptionHandle>(
        debug_ipc::ExceptionType::kThreadStarting, std::move(new_task)));

    // The current thread will also be suspended. The suspend handle going out of scope will
    // resume it.
    DEBUG_LOG(Thread) << "Resuming forking source thread for " << pid();
  } else {
    DEBUG_LOG(Thread) << "LinuxTask::OnFork GetEventMsg failed.";
  }
}

void LinuxTask::OnVFork(const SignalRecord& record) {
  DEBUG_LOG(Thread) << "LinuxTask::OnVFork UNIMPLEMENTED" << pid_;
  // TODO(brettw) implement this.
}

void LinuxTask::OnExit(const SignalRecord& record) {
  DEBUG_LOG(Thread) << "LinuxTask::OnExit " << pid_;

  // Tolerate failure since we need to send the exit notification even if ptrace fails.
  unsigned long result_code = static_cast<unsigned long>(-1);
  if (record.event_msg)
    result_code = *record.event_msg;

  exiting_ = true;

  DEBUG_LOG(Thread) << "  LinuxTask::OnFDReady TRACE EXIT, result=" << result_code;
  exit_code_ = static_cast<int>(result_code);
  suspend_count_++;  // Account for suspend count the LinuxException will take.

  LinuxSuspendHandle sus(fxl::RefPtr<LinuxTask>(this));
  observer_->OnExited(
      this, std::make_unique<LinuxExceptionHandle>(debug_ipc::ExceptionType::kThreadExiting,
                                                   fxl::RefPtr<LinuxTask>(this)));
}

void LinuxTask::OnSignaled(const SignalRecord& record) {
  DEBUG_LOG(Thread) << "  LinuxTask::OnFDReady SIGNALED "
                    << linux::SignalToString(WTERMSIG(record.status), true);
  observer_->OnTermSignal(pid_, WTERMSIG(record.status));
}

void LinuxTask::OnStopped(const SignalRecord& record) {
  DEBUG_LOG(Thread) << "  LinuxTask::OnFDReady " << pid_ << " STOPPED";
  if (!record.siginfo)
    return;  // This can happen when a process is killed.

  DEBUG_LOG(Thread) << "Got stop signal " << record.siginfo->si_signo << " code "
                    << record.siginfo->si_code;

  // The thread was stopped and the exception handle is taking ownership of that stop and will
  // resume it in its destructor.
  suspend_count_++;
  DEBUG_LOG(Thread) << "Making exception handle, new suspend count = " << suspend_count_;
  observer_->OnStopSignal(
      this, std::make_unique<LinuxExceptionHandle>(fxl::RefPtr<LinuxTask>(this), *record.siginfo));
}

void LinuxTask::OnContinued() {
  DEBUG_LOG(Thread) << "  LinuxTask::OnFDReady " << pid_ << " CONTINUED";
  observer_->OnContinued(pid_);
}

LinuxTask::SignalRecord LinuxTask::MakeSignalRecord(int pid, int status) {
  SignalRecord rec{.pid = pid_, .status = status};
  if (HasEventMessage(status)) {
    unsigned long msg = 0;
    if (ptrace(PTRACE_GETEVENTMSG, pid, 0, &msg) != -1) {
      rec.event_msg = msg;
    } else {
      DEBUG_LOG(Thread) << "PTRACE_GETEVENTMSG failed with errno=" << errno;
    }
  }

  if (WIFSTOPPED(status)) {
    siginfo_t sig = {};
    if (ptrace(PTRACE_GETSIGINFO, pid, nullptr, &sig) != -1) {
      rec.siginfo = sig;
    }
  }

  return rec;
}

void LinuxTask::IncrementSuspendCount() {
  DEBUG_LOG(Thread) << "LinuxTask::IncrementSuspendCount of PID=" << pid() << " from "
                    << suspend_count_;
  suspend_count_++;
  if (suspend_count_ > 1)
    return;  // Already suspended, nothing to do.

  // Check after adjusting the suspend reference counts so they balance even when unattached.
  if (!is_attached_) {
    DEBUG_LOG(Thread) << "Skipping pause due to not being attached.";
    return;
  }
  if (exiting_) {
    DEBUG_LOG(Thread) << "Skipping pause due to exiting.";
    return;
  }

  // Suspend.
  DEBUG_LOG(Thread) << "Sending SIGSTOP";
  if (tkill(pid_, SIGSTOP) == -1) {
    DEBUG_LOG(Thread) << "Sending SIGSTOP failed with errno = " << errno;
    return;
  }

  // Synchronously wait for the stop reply. This is necessary because ptrace has a synchronous
  // model. It will send a SIGSTOP ptrace event back to the message loop which we won't be able to
  // differentiate from our own, and which will race with a continue call (PTRACE_CONT will have no
  // effect until we've received the STOP signal).
  int stop_status = 0;
  if (waitpid(pid_, &stop_status, __WNOTHREAD) == -1) {
    DEBUG_LOG(Thread) << "waitpid failed with errno = " << errno;
    return;
  }
  DEBUG_LOG(Thread) << "waitpid " << WaitpidStatusToString(stop_status);

  // Waiting for a ptrace event will itself race with other reasons the thread may stop and waitpid
  // will report a stop here we would want to dispatch. This code attempts to filter out these
  // cases.
  //
  // It's possible somebody else sent us a SIGSTOP at the exact time we're doing this and we never
  // report that stop. That should be very uncommon as generally non-debugger code never uses
  // SIGSTOP.
  if (WIFSTOPPED(stop_status) && WSTOPSIG(stop_status) == SIGSTOP)
    return;  // This is our SIGSTOP.

  // Got a ptrace event for something else. We can't recursively handle that now without causing
  // horrible reentrant bugs. So post that message back to the message loop.
  //
  // Take a suspend reference for the duration of this state to represent this stop so we don't try
  // to resume the thread before that ptrace event is processed. Don't use a SuspendHandle because
  // that will take a reference to |this|, and we might curretly be destructing this object.
  DEBUG_LOG(Thread) << "Got a waitpid result for a different signal, re-posting";
  SignalRecord wait_record = MakeSignalRecord(pid_, stop_status);
  suspend_count_++;  // Take reference for the existing stop. Balanced in the callback.
  debug::MessageLoopPoll::Current()->PostTask(
      FROM_HERE, [weak_this = weak_factory_.GetWeakPtr(), wait_record]() {
        DEBUG_LOG(Thread) << "Re-dispatching previous stop.";
        if (weak_this)
          weak_this->DispatchEvent(wait_record);
        if (weak_this)                         // OnPidRead could have deleted us!
          weak_this->DecrementSuspendCount();  // Balance the suspend_count_++ above.
      });

  // In this case the process is stopped, just not for the reason we expected. This is fine as it
  // will be the same as taking a suspend handle on a thread in an exception.
}

void LinuxTask::DecrementSuspendCount() {
  DEBUG_LOG(Thread) << "LinuxTask::DecrementSuspendCount of PID=" << pid() << " from "
                    << suspend_count_;

  FX_DCHECK(suspend_count_ > 0);
  suspend_count_--;

  if (exiting_) {
    DEBUG_LOG(Thread) << "Skipping resume due to exiting.";
    return;
  }

  if (suspend_count_ == 0 && is_attached_) {
    // Resume.
    if (single_step_) {
      DEBUG_LOG(Thread) << "LinuxTask: Resuming single step";
      if (ptrace(PTRACE_SINGLESTEP, pid_, 0, 0) == -1) {
        DEBUG_LOG(Thread) << "PTRACE_SINGLESTEP failed with errno = " << errno;
      }
    } else {
      DEBUG_LOG(Thread) << "LinuxTask: Resuming continue";
      if (ptrace(PTRACE_CONT, pid_, 0, 0) == -1) {
        DEBUG_LOG(Thread) << "PTRACE_CONT failed with errno = " << errno;
      }
    }
  }
  DEBUG_LOG(Thread);
}

}  // namespace debug_agent
