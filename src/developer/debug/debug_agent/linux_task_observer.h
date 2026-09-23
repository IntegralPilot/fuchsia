// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_TASK_OBSERVER_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_TASK_OBSERVER_H_

#include <signal.h>

#include <memory>

namespace debug_agent {

class LinuxExceptionHandle;

// These callbacks correspond to the results of waitpid(). See that man page for more.
class LinuxTaskObserver {
 public:
  // The "exit" message means a thread is exiting. On LInux, since threads and processes are
  // basically the same thing, this also indicates process exit when it's the main thread.
  virtual void OnExited(LinuxTask* task, std::unique_ptr<LinuxExceptionHandle> exception) = 0;

  // Indicates a fork. The exception handle will refer to the *new* process.
  virtual void OnProcessStarting(std::unique_ptr<LinuxExceptionHandle> exception) = 0;

  virtual void OnThreadStarting(std::unique_ptr<LinuxExceptionHandle> exception) = 0;

  virtual void OnTermSignal(int pid, int signal_number) = 0;
  virtual void OnStopSignal(LinuxTask* task, std::unique_ptr<LinuxExceptionHandle> exception) = 0;
  virtual void OnContinued(int pid) = 0;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_TASK_OBSERVER_H_
