// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/debug_adapter/handlers/request_threads.h"

#include "src/developer/debug/zxdb/client/process.h"
#include "src/developer/debug/zxdb/client/session.h"
#include "src/developer/debug/zxdb/client/system.h"
#include "src/developer/debug/zxdb/client/target.h"
#include "src/developer/debug/zxdb/client/thread.h"
#include "src/developer/debug/zxdb/debug_adapter/context.h"

namespace dap {

DAP_IMPLEMENT_STRUCT_TYPEINFO_EXT(ThreadZxdb, Thread, "", DAP_FIELD(processId, "processId"))
DAP_IMPLEMENT_STRUCT_TYPEINFO_EXT(ThreadEventZxdb, ThreadEvent, "thread",
                                  DAP_FIELD(processId, "processId"))
DAP_IMPLEMENT_STRUCT_TYPEINFO(ThreadsResponseZxdb, "", DAP_FIELD(threads, "threads"))

}  // namespace dap

namespace zxdb {

dap::ResponseOrError<dap::ThreadsResponseZxdb> OnRequestThreads(
    DebugAdapterContext* ctx, const dap::ThreadsRequest& /*req*/) {
  dap::ThreadsResponseZxdb response = {};
  auto targets = ctx->session()->system().GetTargets();
  for (auto target : targets) {
    if (!target) {
      continue;
    }
    auto process = target->GetProcess();
    if (!process) {
      continue;
    }
    auto threads = process->GetThreads();
    for (auto thread : threads) {
      dap::ThreadZxdb thread_info;
      thread_info.id = thread->GetKoid();
      thread_info.name = thread->GetName();
      thread_info.processId = process->GetKoid();
      response.threads.push_back(thread_info);
    }
  }

  return response;
}

}  // namespace zxdb
