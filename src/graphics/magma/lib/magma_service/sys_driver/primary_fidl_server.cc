// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "primary_fidl_server.h"

#include <lib/async/cpp/task.h>
#include <lib/magma/magma_common_defs.h>
#include <lib/magma/platform/platform_trace.h>
#include <lib/magma/platform/zircon/zircon_platform_status.h>
#include <lib/magma/util/utils.h>
#include <zircon/assert.h>

#include <optional>

#include "fidl/fuchsia.gpu.magma/cpp/wire_types.h"
#include "lib/magma_service/msd_defs.h"

namespace {
std::optional<fuchsia_gpu_magma::ObjectType> ValidateObjectType(
    fuchsia_gpu_magma::ObjectType fidl_type) {
  switch (fidl_type) {
    case fuchsia_gpu_magma::ObjectType::kBuffer:
    case fuchsia_gpu_magma::ObjectType::kSemaphore:
      return {fidl_type};
    default:
      return std::nullopt;
  }
}

std::optional<int> GetBufferOp(fuchsia_gpu_magma::BufferOp fidl_type) {
  switch (fidl_type) {
    case fuchsia_gpu_magma::wire::BufferOp::kPopulateTables:
      return MAGMA_BUFFER_RANGE_OP_POPULATE_TABLES;
    case fuchsia_gpu_magma::wire::BufferOp::kDepopulateTables:
      return MAGMA_BUFFER_RANGE_OP_DEPOPULATE_TABLES;
    default:
      return std::nullopt;
  }
}

}  // namespace

namespace msd::internal {

class FidlPerfCountPoolServer : public PerfCountPoolServer {
 public:
  FidlPerfCountPoolServer(uint64_t id, zx::channel channel)
      : pool_id_(id), server_end_(std::move(channel)) {}

  uint64_t pool_id() override { return pool_id_; }

  // Sends a OnPerformanceCounterReadCompleted. May be called from any thread.
  magma::Status SendPerformanceCounterCompletion(uint32_t trigger_id, uint64_t buffer_id,
                                                 uint32_t buffer_offset, uint64_t time,
                                                 uint32_t result_flags) override {
    fidl::Arena allocator;
    auto builder = fuchsia_gpu_magma::wire::
        PerformanceCounterEventsOnPerformanceCounterReadCompletedRequest::Builder(allocator);
    builder.trigger_id(trigger_id)
        .buffer_id(buffer_id)
        .buffer_offset(buffer_offset)
        .timestamp(time)
        .flags(fuchsia_gpu_magma::wire::ResultFlags::TruncatingUnknown(result_flags));

    fidl::Status result =
        fidl::WireSendEvent(server_end_)->OnPerformanceCounterReadCompleted(builder.Build());
    switch (result.status()) {
      case ZX_OK:
        return MAGMA_STATUS_OK;
      case ZX_ERR_PEER_CLOSED:
        return MAGMA_STATUS_CONNECTION_LOST;
      case ZX_ERR_TIMED_OUT:
        return MAGMA_STATUS_TIMED_OUT;
      default:
        return MAGMA_STATUS_INTERNAL_ERROR;
    }
  }

 private:
  uint64_t pool_id_;
  fidl::ServerEnd<fuchsia_gpu_magma::PerformanceCounterEvents> server_end_;
};

void PrimaryFidlServer::SetError(fidl::CompleterBase* completer, magma_status_t error) {
  if (!error_) {
    error_ = MAGMA_DRET_MSG(error, "PrimaryFidlServer encountered dispatcher error");
    if (completer) {
      completer->Close(magma::ToZxStatus(error));
    } else {
      server_binding_->Close(magma::ToZxStatus(error));
    }
    async_loop()->Quit();
  }
}

void PrimaryFidlServer::Bind() {
  fidl::OnUnboundFn<PrimaryFidlServer> unbind_callback =
      [](PrimaryFidlServer* self, fidl::UnbindInfo unbind_info,
         fidl::ServerEnd<fuchsia_gpu_magma::Primary> server_channel) {
        // |kDispatcherError| indicates the async loop itself is shutting down,
        // which could only happen when |interface| is being destructed.
        // Therefore, we must avoid using the same object.
        if (unbind_info.reason() == fidl::Reason::kDispatcherError)
          return;

        self->server_binding_ = std::nullopt;
        self->async_loop()->Quit();
      };

  // Note: the async loop should not be started until we assign |server_binding_|.
  server_binding_ = fidl::BindServer(async_loop()->dispatcher(), std::move(primary_), this,
                                     std::move(unbind_callback));
  if (!server_binding_) {
    async_loop()->Quit();
  }
}

void PrimaryFidlServer::NotificationChannelSend(cpp20::span<uint8_t> data) {
  zx_status_t status = server_notification_endpoint_.write(
      0, data.data(), static_cast<uint32_t>(data.size()), nullptr, 0);
  if (status != ZX_OK)
    MAGMA_DLOG("Failed writing to channel: %s", zx_status_get_string(status));
}
void PrimaryFidlServer::ContextKilled() {
  async::PostTask(async_loop()->dispatcher(),
                  [this]() { SetError(nullptr, MAGMA_STATUS_CONTEXT_KILLED); });
}

void PrimaryFidlServer::PerformanceCounterReadCompleted(const msd::PerfCounterResult& result) {
  MAGMA_DASSERT(false);
}

void PrimaryFidlServer::EnableFlowControl(EnableFlowControlCompleter::Sync& completer) {
  flow_control_enabled_ = true;
}

void PrimaryFidlServer::FlowControl(uint64_t size) {
  if (!flow_control_enabled_)
    return;

  messages_consumed_ += 1;
  bytes_imported_ += size;

  if (messages_consumed_ >= kMaxInflightMessages / 2) {
    fidl::Status result =
        fidl::WireSendEvent(server_binding_.value())->OnNotifyMessagesConsumed(messages_consumed_);
    if (result.ok()) {
      messages_consumed_ = 0;
    } else if (!result.is_canceled() && !result.is_peer_closed()) {
      MAGMA_DMESSAGE("SendOnNotifyMessagesConsumedEvent failed: %s",
                     result.FormatDescription().c_str());
    }
  }

  if (bytes_imported_ >= kMaxInflightBytes / 2) {
    fidl::Status result =
        fidl::WireSendEvent(server_binding_.value())->OnNotifyMemoryImported(bytes_imported_);
    if (result.ok()) {
      bytes_imported_ = 0;
    } else if (!result.is_canceled() && !result.is_peer_closed()) {
      MAGMA_DMESSAGE("SendOnNotifyMemoryImportedEvent failed: %s",
                     result.FormatDescription().c_str());
    }
  }
}

void PrimaryFidlServer::ImportObject(ImportObjectRequestView request,
                                     ImportObjectCompleter::Sync& completer) {
  TRACE_DURATION("magma", "ZirconConnection::ImportObject", "type",
                 static_cast<uint32_t>(request->object_type()));
  MAGMA_DLOG("ZirconConnection: ImportObject");

  auto object_type = ValidateObjectType(request->object_type());
  if (!object_type) {
    MAGMA_DMESSAGE("ImportObject: Bad object type");
    SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
    return;
  }

  zx::handle handle;
  switch (*object_type) {
    case fuchsia_gpu_magma::ObjectType::kSemaphore:
      if (request->object().is_semaphore()) {
        handle = std::move(request->object().semaphore());
      }
#if FUCHSIA_API_LEVEL_AT_LEAST(HEAD)
      else if (request->object().is_counter_semaphore()) {
        handle = std::move(request->object().counter_semaphore());
      }
#endif
      break;
    case fuchsia_gpu_magma::ObjectType::kBuffer:
      if (request->object().is_buffer()) {
        handle = std::move(request->object().buffer());
      }
      break;
    default:
      break;
  }

  if (!handle) {
    MAGMA_DMESSAGE("Object type mismatch");
    SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
    return;
  }

  uint64_t flags = request->has_flags() ? static_cast<uint64_t>(request->flags()) : 0;
  uint64_t size = 0;

  if (object_type == fuchsia_gpu_magma::wire::ObjectType::kBuffer) {
    zx::unowned_vmo vmo(handle.get());
    zx_status_t status = vmo->get_size(&size);
    if (status != ZX_OK) {
      MAGMA_DMESSAGE("ImportObject: failed to get vmo size");
      SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
      return;
    }
  }
  FlowControl(size);

  if (!delegate_->ImportObject(std::move(handle), flags, *object_type, request->object_id())) {
    MAGMA_DMESSAGE("ImportObject: failed to import object");
    SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
  }
}

void PrimaryFidlServer::ReleaseObject(ReleaseObjectRequestView request,
                                      ReleaseObjectCompleter::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::ReleaseObject", "type",
                 static_cast<uint32_t>(request->object_type));
  MAGMA_DLOG("PrimaryFidlServer: ReleaseObject");
  FlowControl();

  auto object_type = ValidateObjectType(request->object_type);
  if (!object_type) {
    MAGMA_DMESSAGE("ReleaseObject: failed to validate");
    SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
    return;
  }

  if (!delegate_->ReleaseObject(request->object_id, *object_type)) {
    MAGMA_DMESSAGE("ReleaseObject: failed to release");
    SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
  }
}

void PrimaryFidlServer::CreateContext(CreateContextRequestView request,
                                      CreateContextCompleter::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::CreateContext");
  MAGMA_DLOG("PrimaryFidlServer: CreateContext");
  FlowControl();

  magma::Status status = delegate_->CreateContext(request->context_id);
  if (!status.ok()) {
    MAGMA_DMESSAGE("CreateContext: failed to create context");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::CreateContext2(CreateContext2RequestView request,
                                       CreateContext2Completer::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::CreateContext2");
  MAGMA_DLOG("PrimaryFidlServer: CreateContext2");
  FlowControl();

  uint64_t priority = static_cast<uint64_t>(request->priority);
  if (client_type_ != MagmaClientType::kTrusted && priority > MAGMA_PRIORITY_MEDIUM) {
    MAGMA_LOG(WARNING, "Denying untrusted client that is requesting a high priority");
    SetError(&completer, MAGMA_STATUS_ACCESS_DENIED);
    return;
  }

  magma::Status status = delegate_->CreateContext2(request->context_id, priority);
  if (!status.ok()) {
    MAGMA_DMESSAGE("CreateContext2: failed to create context");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::DestroyContext(DestroyContextRequestView request,
                                       DestroyContextCompleter::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::DestroyContext");
  MAGMA_DLOG("PrimaryFidlServer: DestroyContext");
  FlowControl();

  magma::Status status = delegate_->DestroyContext(request->context_id);
  if (!status.ok()) {
    MAGMA_DMESSAGE("DestroyContext: failed to destroy context");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::ExecuteCommand(ExecuteCommandRequestView request,
                                       ExecuteCommandCompleter::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::ExecuteCommand");
  FlowControl();

  std::vector<magma_exec_command_buffer> command_buffers;
  command_buffers.reserve(request->command_buffers.size());

  for (auto& command_buffer : request->command_buffers) {
    command_buffers.push_back(magma_exec_command_buffer{
        .resource_index = command_buffer.resource_index,
        .start_offset = command_buffer.start_offset,
    });
  }

  std::vector<magma_exec_resource> resources;
  resources.reserve(request->resources.size());

  for (auto& buffer_range : request->resources) {
    resources.push_back({
        buffer_range.buffer_id,
        buffer_range.offset,
        buffer_range.size,
    });
  }

  std::vector<uint64_t> wait_semaphores;
  wait_semaphores.reserve(request->wait_semaphores.size());

  for (uint64_t semaphore_id : request->wait_semaphores) {
    wait_semaphores.push_back(semaphore_id);
  }

  std::vector<uint64_t> signal_semaphores;
  signal_semaphores.reserve(request->signal_semaphores.size());

  for (uint64_t semaphore_id : request->signal_semaphores) {
    signal_semaphores.push_back(semaphore_id);
  }

  magma::Status status = delegate_->ExecuteCommandBuffers(
      request->context_id, command_buffers, resources, wait_semaphores, signal_semaphores,
      static_cast<uint64_t>(request->flags));

  if (!status) {
    MAGMA_DMESSAGE("ExecuteCommand: failed to execute command");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::ExecuteInlineCommands(ExecuteInlineCommandsRequestView request,
                                              ExecuteInlineCommandsCompleter::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::ExecuteInlineCommands");
  MAGMA_DLOG("PrimaryFidlServer: ExecuteInlineCommands");
  FlowControl();

  std::vector<magma_inline_command_buffer> commands;
  commands.reserve(request->commands.size());

  for (auto& fidl_command : request->commands) {
    if (!fidl_command.has_data() || !fidl_command.has_semaphores()) {
      SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
      return;
    }
    commands.push_back({
        .data = fidl_command.data().data(),
        .size = fidl_command.data().size(),
        .semaphore_ids = fidl_command.semaphores().data(),
        .semaphore_count = magma::to_uint32(fidl_command.semaphores().size()),
    });
  }

  magma::Status status = delegate_->ExecuteInlineCommands(request->context_id, std::move(commands));
  if (!status) {
    MAGMA_DMESSAGE("ExecuteInlineCommands: Failed to execute inline command");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::Flush(FlushCompleter::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::Flush");
  MAGMA_DLOG("PrimaryFidlServer: Flush");
  completer.Reply();
}

void PrimaryFidlServer::MapBuffer(MapBufferRequestView request,
                                  MapBufferCompleter::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::MapBuffer");
  MAGMA_DLOG("PrimaryFidlServer: MapBufferFIDL");
  FlowControl();

  if (!request->has_range() || !request->has_hw_va()) {
    MAGMA_DMESSAGE("MapBuffer: Bad request");
    SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
    return;
  }

  auto flags = request->has_flags() ? static_cast<uint64_t>(request->flags()) : 0;

  magma::Status status =
      delegate_->MapBuffer(request->range().buffer_id, request->hw_va(), request->range().offset,
                           request->range().size, flags);
  if (!status.ok()) {
    MAGMA_DMESSAGE("MapBuffer: Failed to map buffer");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::UnmapBuffer(UnmapBufferRequestView request,
                                    UnmapBufferCompleter::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::UnmapBuffer");
  MAGMA_DLOG("PrimaryFidlServer: UnmapBufferFIDL");
  FlowControl();

  if (!request->has_buffer_id() || !request->has_hw_va()) {
    MAGMA_DMESSAGE("Unmapbuffer: Bad request");
    SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
    return;
  }

  magma::Status status = delegate_->UnmapBuffer(request->buffer_id(), request->hw_va());
  if (!status.ok()) {
    MAGMA_DMESSAGE("Unmapbuffer: Failed to unmap buffer");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::BufferRangeOp2(BufferRangeOp2RequestView request,
                                       BufferRangeOp2Completer::Sync& completer) {
  TRACE_DURATION("magma", "PrimaryFidlServer::BufferRangeOp2");
  MAGMA_DLOG("PrimaryFidlServer:::BufferRangeOp2");
  FlowControl();

  std::optional<int> buffer_op = GetBufferOp(request->op);
  if (!buffer_op) {
    SetError(&completer, MAGMA_STATUS_INVALID_ARGS);
    return;
  }

  magma::Status status = delegate_->BufferRangeOp(request->range.buffer_id, *buffer_op,
                                                  request->range.offset, request->range.size);

  if (!status) {
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::EnablePerformanceCounterAccess(
    EnablePerformanceCounterAccessRequestView request,
    EnablePerformanceCounterAccessCompleter::Sync& completer) {
  MAGMA_DLOG("PrimaryFidlServer:::EnablePerformanceCounterAccess");
  FlowControl();

  magma::Status status =
      delegate_->EnablePerformanceCounterAccess(std::move(request->access_token));
  if (!status) {
    MAGMA_DMESSAGE("EnablePerformanceCounterAccess: Failed to enable");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::IsPerformanceCounterAccessAllowed(
    IsPerformanceCounterAccessAllowedCompleter::Sync& completer) {
  MAGMA_DLOG("PrimaryFidlServer:::IsPerformanceCounterAccessAllowed");
  completer.Reply(delegate_->IsPerformanceCounterAccessAllowed());
}

void PrimaryFidlServer::EnablePerformanceCounters(
    EnablePerformanceCountersRequestView request,
    EnablePerformanceCountersCompleter::Sync& completer) {
  FlowControl();
  magma::Status status =
      delegate_->EnablePerformanceCounters(request->counters.data(), request->counters.size());
  if (!status) {
    MAGMA_DMESSAGE("EnablePerformanceCounters: Failed to enable");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::CreatePerformanceCounterBufferPool(
    CreatePerformanceCounterBufferPoolRequestView request,
    CreatePerformanceCounterBufferPoolCompleter::Sync& completer) {
  FlowControl();
  auto pool = std::make_unique<FidlPerfCountPoolServer>(request->pool_id,
                                                        request->event_channel.TakeChannel());
  magma::Status status = delegate_->CreatePerformanceCounterBufferPool(std::move(pool));
  if (!status) {
    MAGMA_DMESSAGE("CreatePerformanceCounters: Failed to create");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::ReleasePerformanceCounterBufferPool(
    ReleasePerformanceCounterBufferPoolRequestView request,
    ReleasePerformanceCounterBufferPoolCompleter::Sync& completer) {
  FlowControl();
  magma::Status status = delegate_->ReleasePerformanceCounterBufferPool(request->pool_id);
  if (!status) {
    MAGMA_DMESSAGE("ReleasePerformanceCounterBufferPool: Failed to release");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::AddPerformanceCounterBufferOffsetsToPool(
    AddPerformanceCounterBufferOffsetsToPoolRequestView request,
    AddPerformanceCounterBufferOffsetsToPoolCompleter::Sync& completer) {
  FlowControl();
  for (auto& offset : request->offsets) {
    magma::Status status = delegate_->AddPerformanceCounterBufferOffsetToPool(
        request->pool_id, offset.buffer_id, offset.offset, offset.size);
    if (!status) {
      SetError(&completer, status.get());
    }
  }
}

void PrimaryFidlServer::RemovePerformanceCounterBufferFromPool(
    RemovePerformanceCounterBufferFromPoolRequestView request,
    RemovePerformanceCounterBufferFromPoolCompleter::Sync& completer) {
  FlowControl();
  magma::Status status =
      delegate_->RemovePerformanceCounterBufferFromPool(request->pool_id, request->buffer_id);
  if (!status) {
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::DumpPerformanceCounters(DumpPerformanceCountersRequestView request,
                                                DumpPerformanceCountersCompleter::Sync& completer) {
  FlowControl();
  magma::Status status = delegate_->DumpPerformanceCounters(request->pool_id, request->trigger_id);
  if (!status) {
    MAGMA_DMESSAGE("DumpPerformanceCounters: Failed");
    SetError(&completer, status.get());
  }
}

void PrimaryFidlServer::ClearPerformanceCounters(
    ClearPerformanceCountersRequestView request,
    ClearPerformanceCountersCompleter::Sync& completer) {
  FlowControl();
  magma::Status status =
      delegate_->ClearPerformanceCounters(request->counters.data(), request->counters.size());
  if (!status) {
    SetError(&completer, status.get());
  }
}

// static
std::unique_ptr<PrimaryFidlServer> PrimaryFidlServer::Create(
    std::unique_ptr<Delegate> delegate, msd_client_id_t client_id,
    fidl::ServerEnd<fuchsia_gpu_magma::Primary> primary,
    fidl::ServerEnd<fuchsia_gpu_magma::Notification> notification, MagmaClientType client_type) {
  if (!delegate)
    return MAGMA_DRETP(nullptr, "attempting to create PlatformConnection with null delegate");

  auto connection = std::make_unique<PrimaryFidlServer>(
      std::move(delegate), client_id, std::move(primary), std::move(notification), client_type);

  return connection;
}

void PrimaryFidlServerHolder::Start(std::unique_ptr<PrimaryFidlServer> server,
                                    ConnectionOwnerDelegate* owner_delegate,
                                    fit::function<void(const char*)> set_thread_priority) {
  server_ = std::move(server);
  loop_thread_ = std::thread([holder = shared_from_this(), owner_delegate,
                              set_thread_priority = std::move(set_thread_priority)]() mutable {
    holder->RunLoop(owner_delegate, std::move(set_thread_priority));
  });
}

void PrimaryFidlServerHolder::Shutdown() {
  {
    std::lock_guard lock(server_lock_);
    if (server_) {
      async::PostTask(server_->async_loop_.dispatcher(), [this]() {
        if (server_->server_binding_) {
          server_->server_binding_->Close(ZX_ERR_CANCELED);
        }
        server_->async_loop_.Quit();
      });
    }
  }
  loop_thread_.join();
}

void PrimaryFidlServerHolder::RunLoop(ConnectionOwnerDelegate* owner_delegate,
                                      fit::function<void(const char*)> set_thread_priority) {
  pthread_setname_np(pthread_self(),
                     ("ConnectionThread " + std::to_string(server_->client_id_)).c_str());
  server_->Bind();

  // Apply the thread role before entering the handler loop.
  set_thread_priority("fuchsia.graphics.magma.connection");

  while (HandleRequest()) {
    server_->request_count_ += 1;
  }

  // Loop has been quit at this point.
  server_->delegate_->SetNotificationCallback(nullptr);
  server_->async_loop_.Shutdown();

  {
    std::lock_guard lock(server_lock_);
    // the runloop terminates when the remote closes, or an error is experienced
    // so this is the appropriate time to let the server go out of scope and be destroyed
    MAGMA_DASSERT(server_.use_count() == 1);
    server_.reset();
  }

  if (owner_delegate) {
    // Must be called after the server_ is destructed, to ensure calls to Shutdown only return after
    // server_ is destructed.
    bool need_detach = false;
    owner_delegate->ConnectionClosed(shared_from_this(), &need_detach);
    if (need_detach)
      loop_thread_.detach();
  }
}

bool PrimaryFidlServerHolder::HandleRequest() {
  zx_status_t status = server_->async_loop_.Run(zx::time::infinite(), true /* once */);
  if (status != ZX_OK)
    return false;
  return true;
}

}  // namespace msd::internal
