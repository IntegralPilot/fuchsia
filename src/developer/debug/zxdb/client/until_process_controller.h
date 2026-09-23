// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_UNTIL_PROCESS_CONTROLLER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_UNTIL_PROCESS_CONTROLLER_H_

#include <lib/fit/function.h>

#include <vector>

#include "src/developer/debug/zxdb/client/thread_observer.h"
#include "src/developer/debug/zxdb/symbols/input_location.h"
#include "src/lib/fxl/memory/ref_counted.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

class Breakpoint;
class Err;
class Process;
class Session;
class Thread;
class UntilThreadController;

// Coordinator for process-wide "until" operations.
class ProcessUntilThreadController : public fxl::RefCountedThreadSafe<ProcessUntilThreadController>,
                                     public ThreadObserver {
 public:
  ProcessUntilThreadController(fxl::WeakPtr<Process> process, std::vector<InputLocation> locations,
                               fit::callback<void(const Err&)> cb);
  virtual ~ProcessUntilThreadController();

  // Handles newly spawned threads.
  void DidCreateThread(Thread* thread) override;

  // The worker is owned by the corresponding Thread object, not by this class.(see
  // UntilThreadController::UntilThreadController) This class keeps a WeakPtr to the worker to
  // notify status of breakpoint.
  void AddWorker(fxl::WeakPtr<UntilThreadController> worker);

  void OnThreadHitBreakpoint(Thread* thread);

  fxl::WeakPtr<Breakpoint> breakpoint() { return breakpoint_; }

 private:
  void OnBreakpointSetComplete(const Err& err, fit::callback<void(const Err&)> cb) const;

  fxl::WeakPtr<Process> process_;
  fxl::WeakPtr<Session> session_;
  fxl::WeakPtr<Breakpoint> breakpoint_;
  std::vector<fxl::WeakPtr<UntilThreadController>> workers_;
  fxl::WeakPtrFactory<ProcessUntilThreadController> weak_factory_;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_UNTIL_PROCESS_CONTROLLER_H_
