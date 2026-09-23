// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_PAUSE_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_PAUSE_H_
#include <dap/protocol.h>
#include <dap/typeof.h>
#include <dap/types.h>

#include "src/developer/debug/zxdb/debug_adapter/context.h"

namespace dap {

struct PauseResponseZxdb : public PauseResponse {};
DAP_DECLARE_STRUCT_TYPEINFO(PauseResponseZxdb);

struct PauseRequestZxdb : public PauseRequest {
  using Response = PauseResponseZxdb;
  optional<integer> processId;
};
DAP_DECLARE_STRUCT_TYPEINFO(PauseRequestZxdb);

// Sent when a process-level pause request completes and all threads in the process
// have been suspended.
struct ProcessStoppedEventZxdb : public Event {
  integer processId;
  optional<string> name;
  array<integer> threads;
};
DAP_DECLARE_STRUCT_TYPEINFO(ProcessStoppedEventZxdb);

}  // namespace dap

namespace zxdb {

void OnRequestPause(DebugAdapterContext* ctx, const dap::PauseRequestZxdb& request,
                    std::function<void(dap::ResponseOrError<dap::PauseResponseZxdb>)> callback);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_PAUSE_H_
