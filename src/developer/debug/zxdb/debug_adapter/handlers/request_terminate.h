// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_TERMINATE_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_TERMINATE_H_

#include <dap/protocol.h>
#include <dap/typeof.h>
#include <dap/types.h>

#include "src/developer/debug/zxdb/debug_adapter/context.h"

namespace dap {

// Custom ZxdbTerminateRequest to support targeting a specific process.
// Since zxdb can attach to multiple processes at the same time, we explicitly
// require the user to specify the target process KOID. Standard TerminateRequest
// (without KOID) is not supported. So far, we haven't found any existing IDE
// extension that uses this command.
struct ZxdbTerminateRequest : public TerminateRequest {
  optional<integer> koid;
};

DAP_DECLARE_STRUCT_TYPEINFO(ZxdbTerminateRequest);

}  // namespace dap

namespace zxdb {

void OnRequestZxdbTerminate(
    DebugAdapterContext* ctx, const dap::ZxdbTerminateRequest& req,
    const std::function<void(dap::ResponseOrError<dap::TerminateResponse>)>& callback);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_TERMINATE_H_
