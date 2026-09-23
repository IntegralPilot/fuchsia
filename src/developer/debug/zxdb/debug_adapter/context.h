// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_CONTEXT_H_
#define SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_CONTEXT_H_

#include <cstdint>
#include <filesystem>
#include <utility>

#include <dap/protocol.h>
#include <dap/session.h>

#include "src/developer/debug/shared/stream_buffer.h"
#include "src/developer/debug/zxdb/client/breakpoint_observer.h"
#include "src/developer/debug/zxdb/client/frame.h"
#include "src/developer/debug/zxdb/client/process_observer.h"
#include "src/developer/debug/zxdb/client/session_observer.h"
#include "src/developer/debug/zxdb/client/system_observer.h"
#include "src/developer/debug/zxdb/client/thread_observer.h"
#include "src/developer/debug/zxdb/common/err.h"
#include "src/developer/debug/zxdb/console/console.h"
#include "src/developer/debug/zxdb/debug_adapter/async_backtrace_subscription.h"
#include "src/developer/debug/zxdb/expr/format_node.h"
#include "src/lib/fxl/memory/weak_ptr.h"

namespace zxdb {

class Session;
class Breakpoint;
class Filter;

class DebugAdapterServer;
class DebugAdapterReader;
class DebugAdapterWriter;

// Types of variables reported in variables request.
enum class VariablesType {
  kLocal = 0,
  kArguments,
  kRegister,
  kChildVariable,
  kVariablesTypeCount,  // Keep this in the end always
};

struct VariablesRecord {
  int64_t frame_id;
  VariablesType type = VariablesType::kVariablesTypeCount;
  // Fields to store children information corresponding to the record so that subsequent variables
  // request can be processed. Store the format node in `parent` if children exist. If `parent`'s
  // child has children, store a weak pointer to it in `child`.
  std::unique_ptr<FormatNode> parent;
  fxl::WeakPtr<FormatNode> child;
};

// Handles processing requests from debug adapter client with help from zxdb client session and dap
// library.
// Note: All methods in this class need to be executed on main thread to avoid concurrency bugs.
class DebugAdapterContext : public ThreadObserver,
                            public ProcessObserver,
                            public SessionObserver,
                            public BreakpointObserver,
                            public SystemObserver {
 public:
  using DestroyConnectionCallback = std::function<void()>;

  explicit DebugAdapterContext(Console* console, debug::StreamBuffer* stream);
  virtual ~DebugAdapterContext();

  Console* console() { return console_; }
  Session* session() { return console_->context().session(); }
  dap::Session& dap() { return *dap_; }
  debug::StreamBuffer* stream() { return stream_; }
  bool supports_run_in_terminal() { return supports_run_in_terminal_; }

  // Notification about the stream.
  void OnStreamReadable();

  void OnProcessPaused(Process* process);
  void OnThreadPaused(Thread* thread);

  // Callback to delete the connection and hence this context. This callback will be posted on
  // message loop.
  void set_destroy_connection_callback(DestroyConnectionCallback cb) {
    destroy_connection_cb_ = std::move(cb);
  }

  // SessionObserver implementation:
  void DidResolveConnection(const Err& err) override;

  // ThreadObserver implementation:
  void DidCreateThread(Thread* thread) override;
  void WillDestroyThread(Thread* thread) override;
  void OnThreadStopped(Thread* thread, const StopInfo& info) override;
  void DidUpdateStackFrames(Thread* thread) override;

  // ProcessObserver implementation:
  void DidCreateProcess(Process* process, uint64_t timestamp) override;
  void WillDestroyProcess(Process* process, DestroyReason reason, int exit_code,
                          uint64_t timestamp) override;

  // BreakpointObserver implementation:
  void OnBreakpointMatched(Breakpoint* breakpoint, bool user_requested) override;

  // SystemObserver implementation:
  void WillDestroyFilter(Filter* filter) override;

  Thread* GetThread(uint64_t koid);

  // Checks if thread is in stopped state; returns error if not stopped.
  // `thread` can be nullptr, in which case an error is returned.
  Err CheckStoppedThread(Thread* thread);

  // Returns a vector of elided frame matches against a stack.
  // The returned vector will have the same `size()` as the `stack`.
  std::vector<PrettyStackManager::Match> GetElidedFrames(const Stack& stack);

  // Helper methods to get/set frame to ID mapping
  int64_t IdForFrame(uint64_t thread_koid, int64_t stack_index);
  Frame* FrameforId(int64_t id);
  void DeleteFrameIdsForThread(Thread* thread);

  // Helper methods to get/set variables references
  int64_t IdForVariables(int64_t frame_id, VariablesType type,
                         std::unique_ptr<FormatNode> parent = nullptr,
                         fxl::WeakPtr<FormatNode> child = nullptr);
  VariablesRecord* VariablesRecordForID(int64_t id);
  void DeleteVariablesIdsForFrameId(int64_t id);

  // Helper methods to get/set breakpoint to source file mapping.
  void StoreBreakpointForSource(const std::filesystem::path& source, Breakpoint* bp);
  std::vector<fxl::WeakPtr<Breakpoint>>* GetBreakpointsForSource(
      const std::filesystem::path& source);

  // Helper methods to get/set function breakpoint mapping.
  void StoreFunctionBreakpoint(Breakpoint* bp);
  const std::vector<fxl::WeakPtr<Breakpoint>>& GetFunctionBreakpoints() const {
    return function_bps_;
  }

  // Helper methods to get/set breakpoint to ID mapping
  int64_t IdForBreakpoint(Breakpoint* breakpoint);

  // These methods only delete breakpoints added by the debug adapter.
  // Breakpoints added from console are not deleted.
  void DeleteBreakpointsForSource(const std::filesystem::path& source);
  void DeleteAllFunctionBreakpoints();
  void DeleteAllBreakpoints();

  void StoreFilter(Filter* filter);
  void DeleteAllFilters();
  const std::vector<Filter*>& filters() const { return filters_; }

  // Deinitializes the `AsyncBacktraceSubscription` for testing purposes.
  //
  // This can be used to reduce noise for tests that don't care about async backtrace behavior, as
  // leaving it in-place results in additional `dap::Event`s on thread events, requiring extra
  // `RunClient()` calls.
  //
  // This can only be called once, after `DebugAdapterContext::Init`.
  void DeinitializeAsyncBacktraceSubscriptionForTesting() {
    FX_DCHECK(async_backtrace_subscription_.has_value())
        << "DeinitializeAsyncBacktraceSubscriptionForTesting can only be called once after "
           "DebugAdapterContext::Init";
    async_backtrace_subscription_.reset();
  }

  fxl::WeakPtr<DebugAdapterContext> GetWeakPtr() { return weak_factory_.GetWeakPtr(); }

 private:
  Console* const console_;
  const std::shared_ptr<dap::Session> dap_;
  std::shared_ptr<DebugAdapterReader> reader_;
  std::shared_ptr<DebugAdapterWriter> writer_;

  bool supports_run_in_terminal_ = false;
  bool supports_invalidate_event_ = false;
  bool init_done_ = false;

  struct FrameRecord {
    uint64_t thread_koid = 0;
    int64_t stack_index = 0;
  };
  std::map<int64_t, FrameRecord> id_to_frame_;
  int64_t next_frame_id_ = 1;

  std::map<int64_t, VariablesRecord> id_to_variables_;
  int64_t next_variables_id_ = 1;

  std::map<const Breakpoint*, int64_t> breakpoint_to_id_;
  int64_t next_breakpoint_id_ = 1;

  DestroyConnectionCallback destroy_connection_cb_;

  // This is used when the DAP initialize request comes when the debugger has a pending connection
  // to the device. In this case, we want to defer the DAP initialze response until the connection
  // is resolved.
  fit::callback<void(dap::ResponseOrError<dap::InitializeResponse>)> send_initialize_response_;

  // Stores all breakpoints added by the debug adapter client.
  // While may be redundant since `System::GetBreakpoints` gives us `Breakpoint` instances and
  // `Breakpoint::GetLocations` can get us `Location` instances with source `FileLine` details,
  // `FileLine` instances have weaker guarantees about the normalization/existence of its path
  // members, so `source_to_bp_` trades off a potential simplification for sake of correctness.
  // See https://fxbug.dev/377344509 and `FileLine::comp_dir()` documentation for more context.
  std::map<std::filesystem::path, std::vector<fxl::WeakPtr<Breakpoint>>> source_to_bp_;

  // Stores all function breakpoints added by the debug adapter client.
  std::vector<fxl::WeakPtr<Breakpoint>> function_bps_;

  // Stores all filters added by the debug adapter client.
  std::vector<Filter*> filters_;

  // Monitors async-backtrace state changes and propagates via custom DAP events.
  std::optional<AsyncBacktraceSubscription> async_backtrace_subscription_;

  // Checks if a complete DAP message is available in the stream before parsing. The `dap::Reader`
  // interface (defined in `dap/io.h`) expects `read()` to block until data is available, but our
  // `DebugAdapterReader` implementation is non-blocking. To prevent `cppdap` from corrupting the
  // stream when reading partial packets, we gate calls with this check to ensure a full message is
  // buffered. See more in https://fxbug.dev/521233855.
  bool HasCompleteMessage();

  debug::StreamBuffer* stream_ = nullptr;

  void Init();

  fxl::WeakPtrFactory<DebugAdapterContext> weak_factory_{this};
};

class DebugAdapterReader : public dap::Reader {
 public:
  explicit DebugAdapterReader(debug::StreamBuffer* stream) : stream_(stream) {}
  size_t read(void* buffer, size_t n) override {
    if (!stream_) {
      return 0;
    }
    auto ret = stream_->Read(static_cast<char*>(buffer), n);
    return ret;
  }
  bool isOpen() override { return !!stream_; }

  void close() override { stream_ = nullptr; }

 private:
  debug::StreamBuffer* stream_ = nullptr;
};

class DebugAdapterWriter : public dap::Writer {
 public:
  explicit DebugAdapterWriter(debug::StreamBuffer* stream) : stream_(stream) {}
  bool write(const void* buffer, size_t n) override {
    if (!stream_) {
      return false;
    }
    stream_->Write(
        std::vector<char>(static_cast<const char*>(buffer), static_cast<const char*>(buffer) + n));
    return true;
  }
  bool isOpen() override { return !!stream_; }

  void close() override { stream_ = nullptr; }

 private:
  debug::StreamBuffer* stream_ = nullptr;
};

}  // namespace zxdb

#endif  // SRC_DEVELOPER_DEBUG_ZXDB_DEBUG_ADAPTER_CONTEXT_H_
