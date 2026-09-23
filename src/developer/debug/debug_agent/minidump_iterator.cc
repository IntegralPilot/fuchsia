// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/minidump_iterator.h"

#include "fidl/fuchsia.debugger/cpp/fidl.h"
#include "src/developer/debug/debug_agent/debug_agent.h"
#include "src/developer/debug/debug_agent/debugged_process.h"
#include "src/developer/debug/ipc/protocol.h"

namespace debug_agent {

MinidumpIterator::MinidumpIterator(fxl::WeakPtr<DebugAgent> debug_agent,
                                   std::vector<DebuggedProcess*> processes)
    : processes_(std::move(processes)), debug_agent_(std::move(debug_agent)) {}

void MinidumpIterator::GetNext(GetNextCompleter::Sync& completer) {
  FX_CHECK(debug_agent_);

  if (processes_.empty()) {
    completer.Reply(fit::error(fuchsia_debugger::MinidumpError::kNoProcesses));
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  zx::vmo vmo;
  if (!Advance()) {
    zx::vmo::create(0, 0, &vmo);

    completer.Reply(fit::success(
        fuchsia_debugger::MinidumpIteratorGetNextResponse{{.minidump = std::move(vmo)}}));

    completer.Close(ZX_OK);
    return;
  }

  // |current_process_| should always be valid after |Advance| returns true.
  FX_DCHECK(current_process_);

  debug_ipc::SaveMinidumpReply reply;
  debug_agent_->OnSaveMinidump({.process_koid = current_process_->koid()}, &reply);

  if (reply.status.has_error()) {
    LOGS(Warn) << reply.status.message();
    completer.Reply(fit::error(fuchsia_debugger::MinidumpError::kInternalError));
    return;
  }

  if (auto status = zx::vmo::create(reply.core_data.size(), 0, &vmo); status != ZX_OK) {
    completer.Reply(fit::error(fuchsia_debugger::MinidumpError::kInternalError));
    return;
  }

  if (auto status = vmo.write(reply.core_data.data(), 0, reply.core_data.size()); status != ZX_OK) {
    completer.Reply(fit::error(fuchsia_debugger::MinidumpError::kInternalError));
    return;
  }

  completer.Reply(fit::success(
      fuchsia_debugger::MinidumpIteratorGetNextResponse{{.minidump = std::move(vmo)}}));
}

bool MinidumpIterator::Advance() {
  if (processes_.empty() || index_ == processes_.size()) {
    current_process_ = nullptr;
    return false;
  }

  // Grab the process at the current index and move along, at this point we know that
  // processes_[index_] is a valid pointer (the process might not be around anymore).
  current_process_ = processes_[index_++];
  return true;
}

}  // namespace debug_agent
