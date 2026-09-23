// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/client/until_process_controller.h"

#include <lib/syslog/cpp/macros.h>

#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/breakpoint_settings.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/client/until_thread_controller.h"

namespace zxdb {

ProcessUntilThreadController::ProcessUntilThreadController(fxl::WeakPtr<Process> process,
                                                           std::vector<InputLocation> locations,
                                                           fit::callback<void(const Err&)> cb)
    : process_(std::move(process)), weak_factory_(this) {
  if (!process_) {
    cb(Err("No process."));
    return;
  }

  // To handle newly spawned threads.
  session_ = process_->session()->GetWeakPtr();
  session_->thread_observers().AddObserver(this);
  BreakpointSettings settings;
  settings.scope = ExecutionScope(process_->GetTarget());
  settings.locations = std::move(locations);

  // Default value of the original run_until.
  settings.one_shot = true;

  breakpoint_ = process_->session()->system().CreateNewInternalBreakpoint()->GetWeakPtr();
  breakpoint_->SetSettings(settings, [weak_this = weak_factory_.GetWeakPtr(),
                                      cb = std::move(cb)](const Err& err) mutable {
    if (weak_this)
      weak_this->OnBreakpointSetComplete(err, std::move(cb));
  });
}

ProcessUntilThreadController::~ProcessUntilThreadController() {
  if (session_)
    session_->thread_observers().RemoveObserver(this);

  if (breakpoint_ && session_)
    session_->system().DeleteBreakpoint(breakpoint_.get());
}

// Registers the controller to the newly created thread in case this newly created thread is the
// one that actually runs through the breakpoint in the future.
void ProcessUntilThreadController::DidCreateThread(Thread* thread) {
  if (breakpoint_ && process_ && thread->GetProcess() == process_.get()) {
    auto worker =
        std::make_unique<UntilThreadController>(fxl::RefPtr<ProcessUntilThreadController>(this));
    // No need to resume.
    thread->AddController(std::move(worker), {});
  }
}

void ProcessUntilThreadController::AddWorker(fxl::WeakPtr<UntilThreadController> worker) {
  workers_.push_back(std::move(worker));
}

void ProcessUntilThreadController::OnThreadHitBreakpoint(Thread* thread) {
  FX_CHECK(breakpoint_ && process_);
  // The first thread to call this function.
  process_->session()->system().DeleteBreakpoint(breakpoint_.get());
  breakpoint_.reset();

  // Proactively cancel all other worker controllers.
  for (const auto& worker : workers_) {
    if (worker && worker->thread() != thread) {
      worker->Cancel();
    }
  }
}

void ProcessUntilThreadController::OnBreakpointSetComplete(
    const Err& err, fit::callback<void(const Err&)> cb) const {
  if (err.has_error())
    return cb(err);  // Error updating breakpoint.
  const std::vector<BreakpointLocation*> locs = breakpoint_->GetLocations();
  if (locs.empty()) {
    cb(Err("Destination to run until matched no location."));
  } else {
    cb(Err());
  }
}

}  // namespace zxdb
