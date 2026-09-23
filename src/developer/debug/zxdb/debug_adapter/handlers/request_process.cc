// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_process.h"

#include <dap/types.h>

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/client/thread.h"

namespace dap {

DAP_IMPLEMENT_STRUCT_TYPEINFO(ZxdbProcessInfo, "", DAP_FIELD(id, "id"), DAP_FIELD(name, "name"),
                              DAP_FIELD(threads, "threads"))

DAP_IMPLEMENT_STRUCT_TYPEINFO(ZxdbProcessResponse, "", DAP_FIELD(processes, "processes"))

DAP_IMPLEMENT_STRUCT_TYPEINFO(ZxdbProcessRequest, "zxdb.Process", DAP_FIELD(pid, "pid"))

}  // namespace dap

namespace zxdb {

namespace {

dap::ZxdbProcessInfo ProcessToInfo(const Process* process) {
  dap::ZxdbProcessInfo info;
  info.id = static_cast<int64_t>(process->GetKoid());
  info.name = process->GetName();
  for (const auto* thread : process->GetThreads()) {
    if (!thread) {
      continue;
    }
    dap::Thread thread_info;
    thread_info.id = static_cast<int64_t>(thread->GetKoid());
    thread_info.name = thread->GetName();
    info.threads.push_back(thread_info);
  }
  return info;
}

}  // namespace

dap::ResponseOrError<dap::ZxdbProcessResponse> OnRequestZxdbProcess(
    DebugAdapterContext* ctx, const dap::ZxdbProcessRequest& req) {
  dap::ZxdbProcessResponse response = {};

  if (req.pid) {
    if (*req.pid <= 0) {
      return dap::Error("PID must be positive");
    }

    uint64_t pid = static_cast<uint64_t>(*req.pid);
    Target* match = nullptr;
    for (auto* target : ctx->session()->system().GetTargets()) {
      if (target && target->GetProcess() && target->GetProcess()->GetKoid() == pid) {
        match = target;
        break;
      }
    }

    if (!match) {
      return dap::Error("Process not found");
    }

    response.processes.push_back(ProcessToInfo(match->GetProcess()));
    return response;
  }

  for (auto* target : ctx->session()->system().GetTargets()) {
    if (!target) {
      continue;
    }
    auto* process = target->GetProcess();
    if (!process) {
      continue;
    }
    response.processes.push_back(ProcessToInfo(process));
  }

  return response;
}

}  // namespace zxdb
