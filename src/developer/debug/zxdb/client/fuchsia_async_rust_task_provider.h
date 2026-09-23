// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_FUCHSIA_ASYNC_RUST_TASK_PROVIDER_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_FUCHSIA_ASYNC_RUST_TASK_PROVIDER_H_

#include "src/developer/debug/zxdb/client/async_task_provider.h"

namespace zxdb {

class Session;

// This class implements the |AsyncTaskProvider| interface for the fuchsia-async executor
// implementation: https://fuchsia-docs.firebaseapp.com/rust/fuchsia_async/index.html.
//
// This is (at the time of writing) the canonical implementation for all asynchronous Rust code
// written for the Fuchsia platform. Therefore, this provider implementation is *extremely* specific
// and relies on specific implementation details of the Executor in order to provide the level of
// details that we desire.
//
// This class is *not* generalizable to other async ecosystems like Tokio.
//
// This class supports identifying the following constructs from any thread that is found to have
// the fuchsia-async executor present in the stack.
//   * Scope
//   * ScopeHandle
//   * scope::Join
//   * Tasks
//   * Futures (from the futures-rs crate)
//   * futures-util types:
//     * Fuse
//     * Map
//     * MaybeDone
//     * Then
//     * Remote & RemoteHandle
//     * WrappedFuture
//   * futures-util macros:
//     * join!
//     * select!
//   * Fuchsia platform subsystem Task and Future implementations:
//     * fuchsia_trace's TraceFuture
//     * fxfs's FutureWithGuard
//     * starnix_core's WrappedFuture
//     * vfs's TaskRunner and RequestListener
class FuchsiaAsyncRustTaskProvider : public AsyncTaskProvider {
 public:
  FuchsiaAsyncRustTaskProvider();
  ~FuchsiaAsyncRustTaskProvider() override;

  // AsyncTaskProvider implementation:
  bool CanHandle(Frame* frame) const override;
  void GetTasks(
      Frame* frame,
      fit::callback<void(const Err&, std::vector<std::unique_ptr<AsyncTask>>)> cb) override;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_CLIENT_FUCHSIA_ASYNC_RUST_TASK_PROVIDER_H_
