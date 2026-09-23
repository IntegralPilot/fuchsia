// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_pause.h"

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/debug_adapter/context.h"

namespace dap {

DAP_IMPLEMENT_STRUCT_TYPEINFO_EXT(PauseRequestZxdb, PauseRequest, "pause",
                                  DAP_FIELD(processId, "processId"))
DAP_IMPLEMENT_STRUCT_TYPEINFO_EXT(PauseResponseZxdb, PauseResponse, "")
DAP_IMPLEMENT_STRUCT_TYPEINFO(ProcessStoppedEventZxdb, "processStopped",
                              DAP_FIELD(processId, "processId"), DAP_FIELD(name, "name"),
                              DAP_FIELD(threads, "threads"))

}  // namespace dap

namespace zxdb {

void OnRequestPause(DebugAdapterContext* ctx, const dap::PauseRequestZxdb& request,
                    std::function<void(dap::ResponseOrError<dap::PauseResponseZxdb>)> callback) {
  if (request.processId && request.threadId != 0) {
    callback(dap::Error("Cannot specify both processId and threadId"));
    return;
  }

  if (request.processId) {
    if (*request.processId <= 0) {
      callback(dap::Error("PID must be positive"));
      return;
    }

    uint64_t pid = static_cast<uint64_t>(*request.processId);
    Process* process = nullptr;
    for (auto* target : ctx->session()->system().GetTargets()) {
      if (target) {
        if (Process* p = target->GetProcess(); p && p->GetKoid() == pid) {
          process = p;
          break;
        }
      }
    }

    if (!process) {
      callback(dap::Error("Process not found"));
      return;
    }

    process->Pause([weak_process = process->GetWeakPtr(), weak_ctx = ctx->GetWeakPtr()]() {
      if (!weak_ctx || !weak_process) {
        return;
      }

      dap::ProcessStoppedEventZxdb event;
      event.processId = static_cast<dap::integer>(weak_process->GetKoid());
      event.name = weak_process->GetName();
      for (auto* thread : weak_process->GetThreads()) {
        if (thread) {
          event.threads.push_back(static_cast<dap::integer>(thread->GetKoid()));
        }
      }
      weak_ctx->dap().send(event);

      weak_ctx->OnProcessPaused(weak_process.get());
    });
    callback(dap::PauseResponseZxdb());
    return;
  }

  auto thread = ctx->GetThread(request.threadId);

  if (!thread) {
    callback(dap::Error("Invalid thread ID"));
    return;
  }

  thread->Pause([weak_thread = thread->GetWeakPtr(), weak_ctx = ctx->GetWeakPtr()]() {
    if (!weak_ctx || !weak_thread) {
      return;
    }

    // Send stopped event with reason "pause"
    dap::StoppedEvent event;
    event.reason = "pause";
    event.threadId = weak_thread->GetKoid();
    weak_ctx->dap().send(event);
    weak_ctx->OnThreadPaused(weak_thread.get());
  });
  callback(dap::PauseResponseZxdb());
}

}  // namespace zxdb
