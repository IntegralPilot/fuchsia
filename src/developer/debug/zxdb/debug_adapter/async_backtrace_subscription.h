// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_ASYNC_BACKTRACE_SUBSCRIPTION_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_ASYNC_BACKTRACE_SUBSCRIPTION_H_

#include <unordered_map>

#include "dap/protocol.h"
#include "dap/session.h"
#include "dap/typeof.h"
#include "src/developer/debug/zxdb/client/thread_observer.h"
#include "src/lib/fxl/functional/cancelable_callback.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace dap {

// NOTE: This needs to be backwards compatible and in sync with `AsyncTaskNodeData` here:
// https://fuchsia.googlesource.com/vscode-plugins/+/refs/heads/main/src/zxdb/async_backtrace.ts
struct AsyncTaskNode {
  // `id` is omitted (nullopt) if the task doesn't have an ID (i.e., `GetId()` == 0).
  // TODO(https://fxbug.dev/494811949): Make this field required once `AsyncTask` can generate
  // synthetic stable IDs when the real ones aren't available.
  optional<string> id;
  string name;
  optional<string> file;
  optional<integer> line;
  array<AsyncTaskNode> children;
};

DAP_DECLARE_STRUCT_TYPEINFO(AsyncTaskNode);

// This custom event allows DAP clients to construct a custom async-backtrace UI that mirrors the
// standard multi-threaded stacktrace UI.
//
// Just as the standard stacktrace UI shows a list of threads, each with their own stack of frames
// underneath, this custom event makes it possible for clients to show a list of threads, each with
// their own tree of async-backtrace tasks underneath.
//
// Here is how thread lifecycle events impact the `AsyncBacktraceUpdate` event:
//  - When a thread is created:   An empty `tasks` array is sent.
//  - When a thread is stopped:   An async-backtrace is collected and used to populate `tasks`.
//  - When a thread is resumed:   An empty `tasks` array is sent.
//  - When a thread is destroyed: The `tasks` property is omitted from the event.
//
// NOTE: This needs to be backwards compatible and in sync with `AsyncBacktraceUpdateData` here:
// https://fuchsia.googlesource.com/vscode-plugins/+/refs/heads/main/src/zxdb/async_backtrace.ts
struct AsyncBacktraceUpdate : Event {
  integer id;
  string name;

  integer processId;
  optional<array<AsyncTaskNode>> tasks;
};

DAP_DECLARE_STRUCT_TYPEINFO(AsyncBacktraceUpdate);

}  // namespace dap

namespace zxdb {

class Session;
class Process;
class Err;
class Frame;

class AsyncBacktraceSubscription : public ThreadObserver {
 public:
  explicit AsyncBacktraceSubscription(fxl::WeakPtr<Session> session,
                                      std::shared_ptr<dap::Session> dap);
  virtual ~AsyncBacktraceSubscription();

  void DidCreateThread(Thread* thread) override;
  void OnThreadStopped(Thread* thread, const StopInfo& info) override;
  void OnProcessStopped(Process* process);
  // TODO(https://fxbug.dev/493935127): Switch to `OnThreadResumed` once it's available.
  void DidUpdateStackFrames(Thread* thread) override;
  void WillDestroyThread(Thread* thread) override;

 private:
  // Invalidates the pending async-backtrace callback for the given thread.
  void CancelPendingBacktrace(const Thread* thread);

  // Collects an async-backtrace from `thread` and sends a custom `AsyncBacktraceUpdate` DAP event.
  void CollectAndReportAsyncBacktrace(Thread* thread);

  fxl::WeakPtr<Session> session_;
  std::shared_ptr<dap::Session> dap_;
  std::unordered_map<uint64_t, fxl::CancelableCallback<void(const Err&, const Frame*)>>
      pending_backtraces_;

  fxl::WeakPtrFactory<AsyncBacktraceSubscription> weak_factory_;

  FXL_DISALLOW_COPY_AND_ASSIGN(AsyncBacktraceSubscription);
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_ASYNC_BACKTRACE_SUBSCRIPTION_H_
