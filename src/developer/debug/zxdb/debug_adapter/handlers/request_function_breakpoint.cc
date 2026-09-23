// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_function_breakpoint.h"

#include <lib/syslog/cpp/macros.h>

#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/breakpoint_settings.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/expr/expr_parser.h"

namespace zxdb {

dap::ResponseOrError<dap::SetFunctionBreakpointsResponse> OnRequestFunctionBreakpoint(
    DebugAdapterContext* ctx, const dap::SetFunctionBreakpointsRequest& req) {
  // Delete any existing function breakpoints.
  ctx->DeleteAllFunctionBreakpoints();

  dap::SetFunctionBreakpointsResponse response;
  for (const auto& request_bp : req.breakpoints) {
    Identifier ident;
    if (Err err = ExprParser::ParseIdentifier(request_bp.name, &ident); err.has_error()) {
      FX_LOGS(WARNING) << "Failed to parse function identifier '" << request_bp.name
                       << "': " << err.msg();
      response.breakpoints.push_back(dap::Breakpoint{
          .message = err.msg(),
          .verified = false,
      });
      continue;
    }

    Breakpoint* breakpoint = ctx->session()->system().CreateNewBreakpoint();
    BreakpointSettings settings;
    settings.locations = {InputLocation(ident)};
    if (request_bp.condition.has_value()) {
      settings.condition = request_bp.condition.value();
    }
    breakpoint->SetSettings(settings);
    ctx->StoreFunctionBreakpoint(breakpoint);

    response.breakpoints.push_back(dap::Breakpoint{
        .id = ctx->IdForBreakpoint(breakpoint),
        .verified = (!breakpoint->GetLocations().empty()),
    });
  }

  return response;
}

}  // namespace zxdb
