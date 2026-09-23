// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_evaluate.h"

#include "dap/session.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/console/command_context.h"
#include "src/developer/debug/zxdb/debug_adapter/context.h"
#include "src/lib/fxl/memory/ref_ptr.h"

namespace zxdb {

void OnRequestEvaluate(
    DebugAdapterContext* ctx, const dap::EvaluateRequest& req,
    const std::function<void(dap::ResponseOrError<dap::EvaluateResponse>)>& callback) {
  // Ignore requests with no context
  if (!req.context.has_value()) {
    callback(dap::Error());
    return;
  }

  // TODO(https://fxbug.dev/527992704): Restrict evaluate handler to expression evaluation
  // only rather than generic console command execution, and verify stopped thread state
  // (CheckStoppedThread). Utilize the console for REPL context.
  if (req.context.value() == "repl") {
    if (req.frameId.has_value()) {
      auto* frame = ctx->FrameforId(req.frameId.value());
      if (!frame) {
        callback(dap::Error("Invalid frame ID"));
        return;
      }
      ctx->console()->context().SetActiveTarget(frame->GetThread()->GetProcess()->GetTarget());
      ctx->console()->context().SetActiveThreadForTarget(frame->GetThread());
      ctx->console()->context().SetActiveFrameForThread(frame);
    }
    ctx->console()->ProcessInputLine(
        req.expression,
        fxl::MakeRefCounted<OfflineCommandContext>(
            ctx->console(), [cb = callback](OutputBuffer output, std::vector<Err> errors) {
              dap::EvaluateResponse resp;
              resp.result = output.AsString();
              cb(resp);
            }));
  } else {
    callback(dap::Error());
  }
}

}  // namespace zxdb
