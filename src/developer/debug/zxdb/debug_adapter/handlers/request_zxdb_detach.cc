// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_zxdb_detach.h"

#include <string>
#include <utility>
#include <vector>

#include <dap/types.h>

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/target.h"

namespace dap {

DAP_IMPLEMENT_STRUCT_TYPEINFO(ZxdbDetachRequest, "zxdb.Detach", DAP_FIELD(pid, "pid"),
                              DAP_FIELD(all, "all"))
DAP_IMPLEMENT_STRUCT_TYPEINFO(ZxdbDetachResponse, "")

}  // namespace dap

namespace zxdb {

void OnRequestZxdbDetach(
    DebugAdapterContext* ctx, const dap::ZxdbDetachRequest& req,
    const std::function<void(dap::ResponseOrError<dap::ZxdbDetachResponse>)>& callback) {
  if (req.all && *req.all) {
    if (req.pid && *req.pid > 0) {
      callback(dap::Error("Cannot specify both 'all' and 'pid'"));
      return;
    }

    const size_t current_targets = ctx->session()->system().GetTargets().size();
    ctx->session()->system().DetachFromAllTargets(
        [current_targets, callback](int detached_targets) {
          if (std::cmp_not_equal(current_targets, detached_targets)) {
            callback(dap::Error("Detached from %d targets but had %d", detached_targets,
                                current_targets));
          } else {
            callback(dap::ZxdbDetachResponse());
          }
        });

    return;
  }

  if (!req.pid) {
    callback(dap::Error("PID is required when 'all' is not specified."));
    return;
  }

  if (*req.pid <= 0) {
    callback(dap::Error("PID must be positive"));
    return;
  }

  uint64_t pid = static_cast<uint64_t>(*req.pid);

  // Find target with given PID
  auto targets = ctx->session()->system().GetTargets();
  Target* match = nullptr;
  for (auto target : targets) {
    if (target && target->GetProcess() && target->GetProcess()->GetKoid() == pid) {
      match = target;
      break;
    }
  }

  if (!match) {
    callback(dap::Error("Process not found"));
    return;
  }

  match->Detach([callback](const fxl::WeakPtr<Target>&, const Err& err) {
    if (err.has_error()) {
      callback(dap::Error(err.msg()));
    } else {
      callback(dap::ZxdbDetachResponse());
    }
  });
}

}  // namespace zxdb
