// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_SUSPEND_HANDLE_H_
#define SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_SUSPEND_HANDLE_H_

#include "src/developer/debug/debug_agent/linux_task.h"
#include "src/developer/debug/debug_agent/suspend_handle.h"
#include "src/developer/debug/shared/logging/logging.h"

namespace debug_agent {

class LinuxTask;

class LinuxSuspendHandle : public SuspendHandle {
 public:
  explicit LinuxSuspendHandle(fxl::RefPtr<LinuxTask> task) : task_(std::move(task)) {
    task_->IncrementSuspendCount();
  }
  ~LinuxSuspendHandle() override {
    DEBUG_LOG(Thread) << "LinuxSuspendHandle::~LinuxSuspendHandle\n";
    task_->DecrementSuspendCount();
  }

 private:
  friend LinuxTask;

  // The LinuxTask object can call this to create a suspend handle when a suspend reference count
  // has already been created to "move" the suspend reference to this handle.
  struct AlreadySuspended {};
  LinuxSuspendHandle(fxl::RefPtr<LinuxTask> task, AlreadySuspended) : task_(std::move(task)) {}

  fxl::RefPtr<LinuxTask> task_;
};

}  // namespace debug_agent

#endif  // SRC_DEVELOPER_DEBUG_DEBUG_AGENT_LINUX_SUSPEND_HANDLE_H_
