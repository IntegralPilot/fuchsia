// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_THREADS_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_THREADS_H_

#include <dap/protocol.h>
#include <dap/session.h>
#include <dap/typeof.h>
#include <dap/types.h>

namespace dap {

struct ThreadZxdb : public Thread {
  optional<integer> processId;
};
DAP_DECLARE_STRUCT_TYPEINFO(ThreadZxdb);

struct ThreadEventZxdb : public ThreadEvent {
  optional<integer> processId;
};
DAP_DECLARE_STRUCT_TYPEINFO(ThreadEventZxdb);

struct ThreadsResponseZxdb : public Response {
  array<ThreadZxdb> threads;
};
DAP_DECLARE_STRUCT_TYPEINFO(ThreadsResponseZxdb);

}  // namespace dap

namespace zxdb {

class DebugAdapterContext;

dap::ResponseOrError<dap::ThreadsResponseZxdb> OnRequestThreads(DebugAdapterContext* ctx,
                                                                const dap::ThreadsRequest& req);

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_HANDLERS_REQUEST_THREADS_H_
