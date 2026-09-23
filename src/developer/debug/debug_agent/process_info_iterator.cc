// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/debug_agent/process_info_iterator.h"

#include "src/developer/debug/debug_agent/backtrace_utils.h"
#include "src/developer/debug/debug_agent/component_manager.h"
#include "src/developer/debug/debug_agent/debug_agent.h"
#include "src/developer/debug/debug_agent/debugged_process.h"
#include "src/developer/debug/debug_agent/system_interface.h"

namespace debug_agent {

ProcessInfoIterator::ProcessInfoIterator(
    fxl::WeakPtr<DebugAgent> debug_agent, std::vector<DebuggedProcess*> processes,
    std::optional<fuchsia_debugger::ThreadDetailsInterest> interest)
    : debug_agent_(std::move(debug_agent)),
      interest_(std::move(interest)),
      processes_(std::move(processes)) {
  // Do initializations if we were given some processes. The case where no processes matched a
  // filter or DebugAgent isn't actually attached to anything will return an error to the client
  // when they call |GetNext|.
  if (!processes_.empty()) {
    current_process_ = processes_[process_index_];
    if (current_process_) {
      current_process_->PopulateCurrentThreads();
      current_process_threads_ = current_process_->GetThreads();
      auto component_info =
          debug_agent_->system_interface().GetComponentManager().FindComponentInfo(
              current_process_->process_handle());
      if (!component_info.empty()) {
        current_moniker_ = component_info[0].moniker;
      }
    }
  }
}

void ProcessInfoIterator::GetNext(GetNextCompleter::Sync& completer) {
  FX_DCHECK(debug_agent_);

  fuchsia_debugger::ProcessInfoIteratorGetNextResponse response;

  if (processes_.empty()) {
    // Not attached to anything yet. Return an error and hang up.
    completer.Reply(fit::error(fuchsia_debugger::ProcessInfoError::kNoProcesses));
    completer.Close(ZX_ERR_BAD_STATE);
    return;
  }

  if (!Advance()) {
    // No more processes, we're done.
    completer.Reply(fit::success(response));
    completer.Close(ZX_OK);
    return;
  }

  auto& info = response.info().emplace_back();

  info.process(current_process_->koid());
  info.moniker(current_moniker_);
  info.thread(current_thread_->koid());

  {
    // Suspend the thread while we gather all the requested data.
    auto suspend_handle = current_thread_->InternalSuspend(true);

    if (CaptureBacktrace()) {
      info.details().backtrace(GetBacktraceMarkupForThread(current_process_->process_handle(),
                                                           current_thread_->thread_handle()));
    }
  }

  completer.Reply(fit::success(std::move(response)));
}

bool ProcessInfoIterator::Advance() {
  if (thread_index_ == current_process_threads_.size()) {
    // The check for empty here is to ensure we have valid behavior if we somehow get into the case
    // where the calling code doesn't bail out before calling this function with an empty vector of
    // processes.
    if (processes_.empty() || process_index_ + 1 == processes_.size()) {
      return false;
    }

    // Move on to the next process. This is a prefix operator since we initialized
    // |current_process_| in the constructor.
    current_process_ = processes_[++process_index_];
    FX_DCHECK(current_process_);
    current_process_->PopulateCurrentThreads();
    current_process_threads_ = current_process_->GetThreads();

    // Reset the thread index for the new process.
    thread_index_ = 0;

    // Update component moniker information.
    auto component_info = debug_agent_->system_interface().GetComponentManager().FindComponentInfo(
        current_process_->process_handle());

    FX_DCHECK(!component_info.empty());

    // Report the first matching moniker.
    current_moniker_ = component_info[0].moniker;
  }

  // Grab this thread and advance the index.
  current_thread_ = current_process_threads_[thread_index_++];

  return true;
}

bool ProcessInfoIterator::CaptureBacktrace() const {
  return interest_ && interest_->backtrace() && *interest_->backtrace();
}

}  // namespace debug_agent
