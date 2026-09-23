// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/block_server/block_server.h"

#include <lib/async/cpp/task.h>
#include <lib/driver/logging/cpp/logger.h>
#include <zircon/assert.h>

#include <utility>

#include "src/storage/lib/block_server/block_server_c.h"

namespace block_server {

BlockServer::BlockServer(const PartitionInfo& info, Interface* interface)
    : interface_(interface),
      server_(block_server_new(
          &info, internal::Callbacks{
                     .context = this,
                     .start_thread =
                         [](void* context, const void* arg) {
                           reinterpret_cast<BlockServer*>(context)->interface_->StartThread(
                               std::unique_ptr<Thread>(new Thread(arg)));
                         },
                     .on_new_session =
                         [](void* context, const internal::Session* session) {
                           reinterpret_cast<BlockServer*>(context)->interface_->OnNewSession(
                               std::unique_ptr<Session>(new Session(session)));
                         },
                     .on_requests =
                         [](void* context, Request* requests, uintptr_t request_count) {
                           reinterpret_cast<BlockServer*>(context)->interface_->OnRequests(
                               std::span<Request>(requests, request_count));
                         },
                     .log =
                         [](void* context, const char* msg, size_t len) {
                           reinterpret_cast<BlockServer*>(context)->interface_->Log(
                               std::string_view(msg, len));
                         },
                 })) {
  ZX_ASSERT_MSG(server_, "Failed to create block server");
}

Session& Session::operator=(Session&& other) {
  if (this == &other)
    return *this;
  if (session_)
    block_server_session_release(session_);
  session_ = other.session_;
  other.session_ = nullptr;
  return *this;
}

Session::~Session() {
  if (session_) {
    block_server_session_release(session_);
  }
}

void Session::Run() { block_server_session_run(session_); }

BlockServer::~BlockServer() {
  internal::BlockServer* server;
  {
    std::lock_guard<std::mutex> lock(mutex_);
    server = std::exchange(server_, nullptr);
  }
  if (server) {
    block_server_delete(server);
  }
}

void BlockServer::Serve(fidl::ServerEnd<fuchsia_storage_block::Block> server_end) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (shutdown_ || !server_) {
    return;
  }
  block_server_serve(server_, server_end.TakeChannel().release());
}

void BlockServer::ServeMapper(fidl::ServerEnd<fuchsia_storage_block::Mapper> server_end) {
  std::lock_guard<std::mutex> lock(mutex_);
  if (shutdown_ || !server_) {
    return;
  }
  block_server_serve_mapper(server_, server_end.TakeChannel().release());
}

void BlockServer::SendReply(RequestId request_id, zx::result<> result) const {
  std::lock_guard<std::mutex> lock(mutex_);
  if (server_) {
    block_server_send_reply(server_, request_id, result.status_value());
  }
}

Request SplitRequest(Request& request, uint32_t block_offset, uint32_t block_size) {
  Request head = request;
  switch (request.operation.tag) {
    case Operation::Tag::Read:
    case Operation::Tag::Write:
      request.operation.read.vmo_offset += static_cast<uint64_t>(block_offset) * block_size;
      request.operation.read.options.inline_crypto.dun += block_offset;
      break;
    case Operation::Tag::Trim:
      break;
    case Operation::Tag::Flush:
      ZX_PANIC("Can't split Flush");
    case Operation::Tag::CloseVmo:
    case Operation::Tag::StartDecompressedRead:
    case Operation::Tag::ContinueDecompressedRead:
      __UNREACHABLE;
  }
  head.operation.read.block_count = block_offset;
  request.operation.read.device_block_offset += block_offset;
  request.operation.read.block_count -= block_offset;
  return head;
}

zx_status_t CheckIoRange(const Request& request, uint64_t total_block_count) {
  uint64_t start;
  uint64_t length;
  switch (request.operation.tag) {
    case Operation::Tag::Read:
      start = request.operation.read.device_block_offset;
      length = request.operation.read.block_count;
      break;
    case Operation::Tag::Write:
      start = request.operation.write.device_block_offset;
      length = request.operation.write.block_count;
      break;
    case Operation::Tag::Trim:
      start = request.operation.trim.device_block_offset;
      length = request.operation.trim.block_count;
      break;
    case Operation::Tag::Flush:
      return ZX_OK;
    case Operation::Tag::CloseVmo:
    case Operation::Tag::StartDecompressedRead:
    case Operation::Tag::ContinueDecompressedRead:
      __UNREACHABLE;
  }
  if (length == 0 || length > total_block_count) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  if (start >= total_block_count || start > total_block_count - length) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  return ZX_OK;
}

void DriverInterface::StartThread(std::unique_ptr<Thread> thread) {
  // We retain a weak reference to `thread` to pass into the task which runs the thread, and keep
  // the strong reference in the dispatcher itself (to be invoked when the dispatcher shutdown
  // completes, in its shutdown callback).
  // The weak reference cannot outlive the strong reference because the task is running on the very
  // dispatcher which holds the strong reference.
  Thread* thread_ptr = thread.get();

  // Create a new dispatcher to run `thread` on.
  zx::result new_dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "Block Server",
      ThreadDispatcherShutdownHandler(std::move(thread)));
  if (new_dispatcher.is_error()) {
    logger().log(fdf::LogSeverity::ERROR, "Failed to create dispatcher for block server thread: {}",
                 new_dispatcher);
    return;
  }

  async_dispatcher_t* const async_dispatcher = new_dispatcher->async_dispatcher();

  // The dispatcher is *always* destroyed via the shutdown handler.
  fdf_dispatcher_t* dispatcher = new_dispatcher->release();

  const zx_status_t status = async::PostTask(async_dispatcher, [thread_ptr, dispatcher]() mutable {
    thread_ptr->Run();
    fdf_dispatcher_shutdown_async(dispatcher);
  });

  // Make sure we destroy the dispatcher if we fail to post the task.
  if (status != ZX_OK) {
    logger().log(fdf::LogSeverity::ERROR, "Failed to post task to run block server thread: {}",
                 zx_status_get_string(status));
    fdf_dispatcher_shutdown_async(dispatcher);
  }
}

void DriverInterface::OnNewSession(std::unique_ptr<Session> session) {
  // We retain a weak reference to `session` to pass into the task which runs the session, and keep
  // the strong reference in the dispatcher itself (to be invoked when the dispatcher shutdown
  // completes, in its shutdown callback).
  // The weak reference cannot outlive the strong reference because the task is running on the very
  // dispatcher which holds the strong reference.
  Session* const session_ptr = session.get();
  // Create a new dispatcher to run `session` on.
  zx::result new_dispatcher = fdf::SynchronizedDispatcher::Create(
      fdf::SynchronizedDispatcher::Options::kAllowSyncCalls, "Block Server",
      SessionDispatcherShutdownHandler(std::move(session)), SessionSchedulerRole());
  if (new_dispatcher.is_error()) {
    logger().log(fdf::LogSeverity::ERROR,
                 "Failed to create dispatcher for block server session: {}", new_dispatcher);
    return;
  }

  async_dispatcher_t* const async_dispatcher = new_dispatcher->async_dispatcher();

  // The dispatcher is *always* destroyed via the shutdown handler.
  fdf_dispatcher_t* dispatcher = new_dispatcher->release();

  const zx_status_t status = async::PostTask(async_dispatcher, [session_ptr, dispatcher]() mutable {
    session_ptr->Run();
    fdf_dispatcher_shutdown_async(dispatcher);
  });

  // Make sure we destroy the dispatcher if we fail to post the task.
  if (status != ZX_OK) {
    logger().log(fdf::LogSeverity::ERROR, "Failed to post task to run block server session: {}",
                 zx_status_get_string(status));
    fdf_dispatcher_shutdown_async(dispatcher);
  }
}

DriverInterface::ShutdownHandler DriverInterface::ThreadDispatcherShutdownHandler(
    std::unique_ptr<Thread> thread) const {
  return [this, thread = std::move(thread)](fdf_dispatcher_t* dispatcher) mutable {
    // Destroy the dispatcher *before* dropping the thread.
    OnDispatcherShutdown(dispatcher);
    thread = nullptr;
  };
}

DriverInterface::ShutdownHandler DriverInterface::SessionDispatcherShutdownHandler(
    std::unique_ptr<Session> session) const {
  return [this, session = std::move(session)](fdf_dispatcher_t* dispatcher) mutable {
    // Destroy the dispatcher *before* dropping the session.  As soon as the Session destructor
    // runs, the session will terminate and a client which called Close will be unblocked.  Clients
    // which continuously start and stop sessions could exhaust the dispatcher's pool limit if we do
    // not destroy the dispatcher before closing the session. See https://fxbug.dev/510041620 for
    // context.
    OnDispatcherShutdown(dispatcher);
    session = nullptr;
  };
}

}  // namespace block_server
