// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_ZXDB_DETACH_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_ZXDB_DETACH_H_

#include <dap/protocol.h>
#include <dap/typeof.h>
#include <dap/types.h>

#include "src/developer/debug/zxdb/debug_adapter/context.h"

namespace dap {

struct ZxdbDetachResponse : public Response {};

DAP_DECLARE_STRUCT_TYPEINFO(ZxdbDetachResponse);

struct ZxdbDetachRequest : public Request {
  using Response = ZxdbDetachResponse;
  optional<integer> pid;
  optional<boolean> all;
};

DAP_DECLARE_STRUCT_TYPEINFO(ZxdbDetachRequest);

}  // namespace dap

namespace zxdb {

void OnRequestZxdbDetach(
    DebugAdapterContext* ctx, const dap::ZxdbDetachRequest& req,
    const std::function<void(dap::ResponseOrError<dap::ZxdbDetachResponse>)>& callback);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_ZXDB_DETACH_H_
