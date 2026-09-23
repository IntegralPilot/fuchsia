// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_terminate.h"

#include <dap/session.h>

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/target.h"

namespace dap {

DAP_IMPLEMENT_STRUCT_TYPEINFO_EXT(ZxdbTerminateRequest, TerminateRequest, "terminate",
                                  DAP_FIELD(koid, "koid"))

}  // namespace dap
namespace zxdb {

// OnRequestZxdbTerminate processes requests to terminate (kill) a specific process by KOID.
// Unlike standard DAP where terminate might stop everything, zxdb's multi-process capabilities
// mean a user can target individual processes to kill while leaving other attached sessions
// running.
void OnRequestZxdbTerminate(
    DebugAdapterContext* ctx, const dap::ZxdbTerminateRequest& req,
    const std::function<void(dap::ResponseOrError<dap::TerminateResponse>)>& callback) {
  if (!req.koid) {
    callback(dap::Error("zxdb does not support terminate without a process KOID."));
    return;
  }
  if (*req.koid <= 0) {
    callback(dap::Error("KOID must be positive"));
    return;
  }
  uint64_t koid = static_cast<uint64_t>(*req.koid);
  Target* match = nullptr;
  for (auto* target : ctx->session()->system().GetTargets()) {
    if (target && target->GetProcess() && target->GetProcess()->GetKoid() == koid) {
      match = target;
      break;
    }
  }
  if (!match) {
    callback(dap::Error("Process not found"));
    return;
  }
  match->Kill([callback](fxl::WeakPtr<Target>, const Err& err) {
    if (err.has_error()) {
      callback(dap::Error(err.msg()));
    } else {
      callback(dap::TerminateResponse());
    }
  });
}

}  // namespace zxdb
