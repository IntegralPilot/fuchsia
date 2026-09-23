// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_STACKTRACE_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_STACKTRACE_H_

#include <dap/typeof.h>

#include "src/developer/debug/zxdb/debug_adapter/context.h"

namespace dap {
class StackTraceRequestZxdb : public StackTraceRequest {
 public:
  optional<boolean> remoteUnwind;
};
DAP_DECLARE_STRUCT_TYPEINFO(StackTraceRequestZxdb);
}  // namespace dap

namespace zxdb {
void OnRequestStackTrace(
    DebugAdapterContext* ctx, const dap::StackTraceRequestZxdb& req,
    std::function<void(dap::ResponseOrError<dap::StackTraceResponse>)> response_cb);
}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_STACKTRACE_H_
