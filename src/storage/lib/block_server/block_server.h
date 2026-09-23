// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_BLOCK_SERVER_BLOCK_SERVER_H_
#define SRC_STORAGE_LIB_BLOCK_SERVER_BLOCK_SERVER_H_

#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/zx/result.h>
#include <zircon/assert.h>
#include <zircon/compiler.h>

#include <atomic>
#include <memory>
#include <mutex>
#include <span>

#include "src/storage/lib/block_server/block_server_c.h"

namespace block_server {

using RequestId = internal::RequestId;
using TraceFlowId = uint64_t;
using Operation = internal::Operation;
using Request = internal::Request;
using PartitionInfo = internal::PartitionInfo;

// Represents a session.  New sessions appear via `OnNewSession`.
class Session {
 public:
  Session(Session&& other) : session_(other.session_) { other.session_ = nullptr; }
  Session& operator=(Session&& other);

  // NOTE: The `BlockServer` destructor will be unblocked before this returns, so take
  // care with any code that runs *after* this returns.
  ~Session();

  // Runs the session (blocking).
  void Run();

 private:
  friend class BlockServer;

  explicit Session(const internal::Session* session) : session_(session) {}
  const internal::Session* session_;
};

// Represents the thread that services all FIDL requests.  This appears via `StartThread`.
class Thread {
 public:
  Thread(Thread&& other) : arg_(other.arg_) { other.arg_ = nullptr; }
  Thread& operator=(Thread&& other) = delete;

  // NOTE: The `BlockServer` destructor will be unblocked before this returns, so take
  // care with any code that runs *after* this returns.
  ~Thread() {
    if (arg_)
      internal::block_server_thread_release(arg_);
  }

  // Runs the thread (blocking).
  void Run() { internal::block_server_thread(arg_); }

 private:
  friend class BlockServer;

  explicit Thread(const void* arg) : arg_(arg) {}
  const void* arg_;
};

// The interface which all block servers must implement.
class Interface {
 public:
  virtual ~Interface() {}
  // Called to start the thread that processes all FIDL requests.  The implementation must start a
  // thread and then call `Thread::Run`.
  virtual void StartThread(std::unique_ptr<Thread>) = 0;

  // Called when a new session is started.  The implementation must start a thread and then call
  // `Session::Run`.  The callback takes ownership of `Session`.
  virtual void OnNewSession(std::unique_ptr<Session>) = 0;

  // Called when new requests arrive.  It is OK for this method to block so as to cause push back on
  // the fifo (which is recommended for effective flow control).  Each request must eventually be
  // completed by calling BlockServer::SendReply; failure to do so will result in resource leaks
  // until the block server terminates. This remains true even during server shutdown; all requests
  // received must be completed (e.g. with ZX_ERR_CANCELED).
  //
  // Implementations are responsible for checking that request block ranges fall within valid
  // device/partition bounds, and completing with ZX_ERR_OUT_OF_RANGE if out of bounds.
  virtual void OnRequests(std::span<Request>) = 0;

  // Called for log messages.
  virtual void Log(std::string_view msg) const {}
};

// Helper class for drivers to use when implementing the block server interface. Simplifies
// integration with the Fuchsia driver framework for logging and async dispatchers.
// TODO(https://fxbug.dev/42085539): Each session runs in a blocking manner on a dedicated
// dispatcher, however the driver framework uses a fixed-size thread pool for running these tasks.
// Once this limit is hit (currently 10), new sessions will be blocked from running until existing
// ones are closed.
class DriverInterface : public Interface {
 public:
  DriverInterface() = default;

  void StartThread(std::unique_ptr<Thread> thread) final;
  void OnNewSession(std::unique_ptr<Session> session) final;
  void Log(std::string_view msg) const final { logger().log(fdf::LogSeverity::INFO, "{}", msg); }

  // The logger to use for log messages. By default uses the global logger instance.
  virtual fdf::Logger& logger() const {
    ZX_ASSERT(fdf::Logger::HasGlobalInstance());
    return *fdf::Logger::GlobalInstance();
  }

  // The scheduler role name to use for session worker threads.
  virtual std::string_view SessionSchedulerRole() const { return {}; }

 protected:
  // A hook which is run whenever a dispatcher shuts down.  The hook must call
  // `fdf_dispatcher_destroy`.
  virtual void OnDispatcherShutdown(fdf_dispatcher_t* dispatcher) const {
    fdf_dispatcher_destroy(dispatcher);
  }

 private:
  using ShutdownHandler = fdf::Dispatcher::ShutdownHandler;

  ShutdownHandler ThreadDispatcherShutdownHandler(std::unique_ptr<Thread> thread) const;
  ShutdownHandler SessionDispatcherShutdownHandler(std::unique_ptr<Session> session) const;
};

class BlockServer {
 public:
  // Constructs a new server.
  BlockServer(const PartitionInfo&, Interface*);
  BlockServer(const BlockServer&) = delete;
  BlockServer(BlockServer&&) = delete;
  BlockServer& operator=(const BlockServer&) = delete;
  BlockServer& operator=(BlockServer&&) = delete;

  // Destroys the server.  This will trigger termination and then block until:
  //
  //   1. `Thread::Run()` returns.
  //   2. All `Session` objects have been destroyed i.e. `Session::Run` has returned
  //      *and* `Session` has been destroyed.
  //
  // Once this returns, there will be no subsequent calls via `Interface`.
  ~BlockServer();

  // Destroys the server asynchronously and calls `callback` when complete.
  //
  // Once the callback is invoked, it is guaranteed that all sessions will have terminated, and no
  // new sessions wil start.  The server can be deleted at any point after the callback is invoked
  // (including in the callback itself).
  template <typename Callback>
  void DestroyAsync(Callback callback) {
    internal::BlockServer* server;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      ZX_ASSERT_MSG(server_ && !shutdown_, "DestroyAsync called multiple times!");
      shutdown_ = true;
      server = server_;
    }

    struct State {
      BlockServer* self;
      Callback callback;
    };

    auto state = std::make_unique<State>(State{this, std::move(callback)});

    block_server_delete_async(
        server,
        [](void* arg) {
          auto state = std::unique_ptr<State>(reinterpret_cast<State*>(arg));
          {
            std::lock_guard<std::mutex> lock(state->self->mutex_);
            state->self->server_ = nullptr;
          }
          state->callback();
        },
        state.release());
  }

  // Serves a new connection.  The FIDL handling is multiplexed onto a single per-server thread.
  void Serve(fidl::ServerEnd<fuchsia_storage_block::Block>);
  void ServeMapper(fidl::ServerEnd<fuchsia_storage_block::Mapper>);

  void SendReply(RequestId, zx::result<>) const;

 private:
  Interface* interface_ = nullptr;
  mutable std::mutex mutex_;
  internal::BlockServer* server_ __TA_GUARDED(mutex_) = nullptr;
  bool shutdown_ __TA_GUARDED(mutex_) = false;
};

// Splits the request at `block_offset` returning the head and leaving the tail in `request`.
Request SplitRequest(Request& request, uint32_t block_offset, uint32_t block_size);

zx_status_t CheckIoRange(const Request& request, uint64_t total_block_count);

}  // namespace block_server

#endif  // SRC_STORAGE_LIB_BLOCK_SERVER_BLOCK_SERVER_H_
