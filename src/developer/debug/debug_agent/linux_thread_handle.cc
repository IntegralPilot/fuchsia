// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/linux_thread_handle.h"

#include <map>

#include "src/developer/debug/debug_agent/linux_suspend_handle.h"
#include "src/developer/debug/debug_agent/linux_utils.h"
#include "src/developer/debug/shared/logging/logging.h"

namespace debug_agent {

LinuxThreadHandle::LinuxThreadHandle(fxl::RefPtr<LinuxTask> task) : task_(std::move(task)) {}

std::string LinuxThreadHandle::GetName() const { return linux::GetStatus(task_->pid())["Name"]; }

ThreadHandle::State LinuxThreadHandle::GetState() const {
  auto state = linux::ThreadStateFromLinuxState(linux::GetStatus(task_->pid())["State"]);
  if (!state)
    return State(debug_ipc::ThreadRecord::State::kDead);
  return *state;
}

std::unique_ptr<SuspendHandle> LinuxThreadHandle::Suspend() {
  DEBUG_LOG(Thread) << "Suspending thread " << task_->pid();
  return std::make_unique<LinuxSuspendHandle>(task_);
}

bool LinuxThreadHandle::WaitForSuspension(TickTimePoint deadline) const {
  // Linux suspensions are synchronous so we never need to wait for one.
  return true;
}

debug_ipc::ThreadRecord LinuxThreadHandle::GetThreadRecord(zx_koid_t process_koid) const {
  DEBUG_LOG(Thread) << "LinuxThreadHandle::GetThreadRecord";
  auto map = linux::GetStatus(task_->pid());

  debug_ipc::ThreadRecord record;
  record.id = {.process = process_koid, .thread = static_cast<uint64_t>(task_->pid())};
  record.name = map["Name"];

  if (auto state = linux::ThreadStateFromLinuxState(map["State"])) {
    record.state = state->state;
    record.blocked_reason = state->blocked_reason;
  } else {
    record.state = debug_ipc::ThreadRecord::State::kDead;
  }

  return record;
}

std::optional<GeneralRegisters> LinuxThreadHandle::GetGeneralRegisters() const {
  if (auto result = linux::ReadGeneralRegisters(task_->pid())) {
    DEBUG_LOG(Thread) << "General registers IP = " << std::hex << "0x" << result->rip;
    return GeneralRegisters(*result);
  }
  return std::nullopt;
}

void LinuxThreadHandle::SetGeneralRegisters(const GeneralRegisters& regs) {
  linux::WriteGeneralRegisters(task_->pid(), regs.GetNativeRegisters());
}

std::optional<DebugRegisters> LinuxThreadHandle::GetDebugRegisters() const {
  // TODO(brettw) implement this.
  return std::nullopt;
}

bool LinuxThreadHandle::SetDebugRegisters(const DebugRegisters& regs) {
  // TODO(brettw) implement this.
  return false;
}

void LinuxThreadHandle::SetSingleStep(bool single_step) {
  DEBUG_LOG(Thread) << "LinuxThreadHandle::SetSingleStep " << single_step;
  task_->set_single_step(single_step);
}

std::vector<debug::RegisterValue> LinuxThreadHandle::ReadRegisters(
    const std::vector<debug::RegisterCategory>& cats_to_get) const {
  // TODO(brettw) implement this.
  return {};
}

std::vector<debug::RegisterValue> LinuxThreadHandle::WriteRegisters(
    const std::vector<debug::RegisterValue>& regs) {
  // TODO(brettw) implement this.
  return {};
}

bool LinuxThreadHandle::InstallHWBreakpoint(uint64_t address) {
  // TODO(brettw) implement this.
  return false;
}

bool LinuxThreadHandle::UninstallHWBreakpoint(uint64_t address) {
  // TODO(brettw) implement this.
  return false;
}

std::optional<WatchpointInfo> LinuxThreadHandle::InstallWatchpoint(
    debug_ipc::BreakpointType type, const debug::AddressRange& range) {
  // TODO(brettw) implement this.
  return std::nullopt;
}

bool LinuxThreadHandle::UninstallWatchpoint(const debug::AddressRange& range) {
  // TODO(brettw) implement this.
  return false;
}

}  // namespace debug_agent
