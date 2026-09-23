// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_attach.h"

#include <lib/syslog/cpp/macros.h>
#include <zircon/types.h>

#include <algorithm>
#include <cerrno>
#include <cstdlib>

#include "src/developer/debug/ipc/filter_utils.h"
#include "src/developer/debug/ipc/records.h"
#include "src/developer/debug/shared/string_util.h"
#include "src/developer/debug/zxdb/client/filter.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/debug_adapter/handlers/request_launch.h"
#include "src/developer/debug/zxdb/expr/expr_number_utils.h"

namespace dap {

DAP_IMPLEMENT_STRUCT_TYPEINFO_EXT(AttachRequestZxdb, AttachRequest, "attach",
                                  DAP_FIELD(process, "process"), DAP_FIELD(command, "command"),
                                  DAP_FIELD(recursive, "recursive"), DAP_FIELD(cwd, "cwd"))

}  // namespace dap

namespace zxdb {

namespace {}  // namespace

dap::ResponseOrError<dap::AttachResponse> OnRequestAttach(DebugAdapterContext* context,
                                                          const dap::AttachRequestZxdb& req) {
  dap::AttachResponse response;

  // TODO(https://fxbug.dev/515680788): Support zxdb attach features (job, exact, weak,
  // job-only) in DAP.

  // Fail-Fast Validation for Empty Patterns.
  if (req.process.empty()) {
    dap::Error err;
    err.message = "Process attach pattern must not be empty!";
    return err;
  }

  std::string pattern = req.process;

  // Detect KOID (Process ID) Attach.
  // If the pattern is a purely decimal numeric string, treat it as an immediate KOID attach.
  // We cast char to unsigned char to prevent undefined behavior in ::isdigit.
  if (std::all_of(pattern.begin(), pattern.end(), [](unsigned char c) { return ::isdigit(c); })) {
    uint64_t koid = 0;
    if (StringToUint64(pattern, &koid).has_error()) {
      dap::Error err;
      err.message = "Failed to parse process ID \"" + pattern + "\"";
      return err;
    }

    // Fetch a runnable target (not currently running), or create one.
    Target* target = nullptr;
    for (auto* t : context->session()->system().GetTargets()) {
      if (t && t->GetState() == Target::State::kNone) {
        target = t;
        break;
      }
    }
    if (!target) {
      target = context->session()->system().CreateNewTarget(nullptr);
    }

    target->Attach(koid,
                   {.priority = debug_ipc::AttachConfig::Priority::kStrong,
                    .target = debug_ipc::TaskType::kProcess},
                   [](fxl::WeakPtr<Target> target, const Err& err, uint64_t timestamp) {
                     if (err.has_error()) {
                       LOGS(Error) << "Async process attach failed: " << err.msg();
                     }
                   });

    return response;
  }

  // Resolve Filter Type for Symbolic Matches.
  auto type = debug_ipc::ResolveFilterType(pattern);

  // Duplicate Filter Deduplication (Scope to this DAP session only).
  Filter* filter = nullptr;
  for (const auto& existing_filter : context->filters()) {
    if (existing_filter && existing_filter->pattern() == pattern &&
        existing_filter->type() == type) {
      filter = existing_filter;
      break;
    }
  }

  // Create Filter if not exists, or configure existing.
  bool created_new = false;
  if (!filter) {
    filter = context->session()->system().CreateNewFilter();
    filter->SetPattern(pattern);
    filter->SetType(type);
    created_new = true;
  }

  // Synchronize recursive state to prevent stale value pollution.
  filter->SetRecursive(req.recursive && *req.recursive);

  // Store filter in context for lifecycle management.
  // We track all filters created by this attach handler inside the connection context rather
  // than relying on global system filters lists. This ensures that when the DAP client disconnects,
  // the context teardown cleanly purges only connection-scoped filters, leaving persistent startup
  // filters pre-configured in `~/.fuchsia/debug/zxdbrc` (like `attach cobalt.cm`) completely
  // untouched.
  if (created_new) {
    context->StoreFilter(filter);
  }

  // Send RunInTerminal event if requested.
  if (req.command) {
    dap::RunInTerminalRequest run_request;
    run_request.title = "zxdb launch";
    run_request.kind = "integrated";
    SplitDapCommand(req.command.value(), run_request.args);
    if (req.cwd) {
      run_request.cwd = req.cwd.value();
    }
    context->dap().send(run_request);
  }

  return response;
}

}  // namespace zxdb
