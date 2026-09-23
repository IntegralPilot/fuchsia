// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_breakpoint.h"

#include <lib/syslog/cpp/macros.h>

#include "src/developer/debug/zxdb/client/breakpoint.h"
#include "src/developer/debug/zxdb/client/breakpoint_settings.h"
#include "src/developer/debug/zxdb/client/session.h"

namespace zxdb {

dap::ResponseOrError<dap::SetBreakpointsResponse> OnRequestBreakpoint(
    DebugAdapterContext* ctx, const dap::SetBreakpointsRequest& req) {
  // A source path needs to be specified in the dap::SetBreakpointsRequest request.
  // name and sourceReference are insufficient.
  if (!req.source.path.has_value()) {
    dap::Error err;
    err.message = "Expected dap::SetBreakpointsRequest::source::path to be set!";
    FX_LOGS(ERROR) << err.message;
    return err;
  }

  // Relative source paths are disallowed since we may not be able to reliably update/remove
  // breakpoints if future requests mix relative/absolute paths.
  std::filesystem::path path(req.source.path.value());
  if (path.is_relative()) {
    dap::Error err;
    err.message = "SetBreakpointsRequest path \"" + path.string() + "\" must be absolute!";
    FX_LOGS(ERROR) << err.message;
    return err;
  }

  // Canonicalizing this path is unlikely to produce an exception, but if it does return a
  // dap::Error to make this failure mode more explicit.
  std::error_code error;
  path = std::filesystem::weakly_canonical(path, error);
  if (error) {
    dap::Error err;
    err.message = "Could not canonicalize SetBreakpointsRequest path \"" + path.string() + "\"!";
    FX_LOGS(ERROR) << err.message << "\nError details: " << error.message();
    return err;
  }

  // Delete any existing breakpoints in the file.
  ctx->DeleteBreakpointsForSource(path);

  // Add any specified breakpoints.
  dap::SetBreakpointsResponse response;
  if (!req.breakpoints.has_value()) {
    return response;
  }
  for (const auto& request_bp : req.breakpoints.value()) {
    Breakpoint* breakpoint = ctx->session()->system().CreateNewBreakpoint();
    BreakpointSettings settings;

    std::vector<InputLocation> locations;
    locations.emplace_back(FileLine(path, request_bp.line));
    settings.locations = locations;
    if (request_bp.condition.has_value()) {
      settings.condition = request_bp.condition.value();
    }
    breakpoint->SetSettings(settings);
    ctx->StoreBreakpointForSource(path, breakpoint);

    response.breakpoints.push_back(dap::Breakpoint{
        .id = ctx->IdForBreakpoint(breakpoint),
        .line = request_bp.line,
        .source = req.source,
        .verified = (!breakpoint->GetLocations().empty()),
    });
  }
  return response;
}

}  // namespace zxdb
