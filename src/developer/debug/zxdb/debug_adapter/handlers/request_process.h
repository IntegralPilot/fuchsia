// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_PROCESS_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_PROCESS_H_

#include <dap/protocol.h>
#include <dap/typeof.h>
#include <dap/types.h>

#include "src/developer/debug/zxdb/debug_adapter/context.h"

namespace dap {

struct ZxdbProcessInfo {
  integer id;
  string name;
  array<Thread> threads;
};

DAP_DECLARE_STRUCT_TYPEINFO(ZxdbProcessInfo);

struct ZxdbProcessResponse : public Response {
  array<ZxdbProcessInfo> processes;
};

DAP_DECLARE_STRUCT_TYPEINFO(ZxdbProcessResponse);

struct ZxdbProcessRequest : public Request {
  using Response = ZxdbProcessResponse;
  optional<integer> pid;
};

DAP_DECLARE_STRUCT_TYPEINFO(ZxdbProcessRequest);

}  // namespace dap

namespace zxdb {

dap::ResponseOrError<dap::ZxdbProcessResponse> OnRequestZxdbProcess(
    DebugAdapterContext* ctx, const dap::ZxdbProcessRequest& req);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_PROCESS_H_
