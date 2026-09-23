// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "magma_system_connection.h"

#include <lib/magma/util/macros.h>

#include <vector>

#include "magma_system_device.h"

namespace msd {
MagmaSystemConnection::MagmaSystemConnection(Owner* owner,
                                             std::unique_ptr<msd::Connection> msd_connection_t)
    : owner_(owner), msd_connection_(std::move(msd_connection_t)) {
  MAGMA_DASSERT(msd_connection_);
}

MagmaSystemConnection::~MagmaSystemConnection() {
  // Remove all contexts before clearing buffers, to give the hardware driver an
  // indication that faults afterwards may be due to buffer mappings having gone
  // away due to the shutdown.
  context_map_.clear();
  for (auto iter = buffer_map_.begin(); iter != buffer_map_.end();) {
    const bool kShuttingDown = true;
    msd_connection()->MsdReleaseBuffer(*iter->second->msd_buf(), kShuttingDown);
    iter = buffer_map_.erase(iter);
  }

  // Iterating over pool_map_ without the mutex held is safe because the map is only modified from
  // this thread.
  for (auto& pool_map_entry : pool_map_) {
    msd_connection()->MsdReleasePerformanceCounterBufferPool(
        std::move(pool_map_entry.second.msd_pool));
  }
  {
    // We still need to lock the mutex before modifying the map.
    std::lock_guard<std::mutex> lock(pool_map_mutex_);
    pool_map_.clear();
  }
  // Reset all MSD objects before calling ConnectionClosed() because the msd device might go away
  // any time after ConnectionClosed() and we don't want any dangling dependencies.
  semaphore_map_.clear();
  msd_connection_.reset();
}

uint32_t MagmaSystemConnection::GetDeviceId() { return owner_->GetDeviceId(); }

magma::Status MagmaSystemConnection::CreateContext(uint32_t context_id) {
  return CreateContext2(context_id,
                        static_cast<uint64_t>(fuchsia_gpu_magma::wire::Priority::kMedium));
}

magma::Status MagmaSystemConnection::CreateContext2(uint32_t context_id, uint64_t priority) {
  auto iter = context_map_.find(context_id);
  if (iter != context_map_.end())
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Attempting to add context with duplicate id");

  auto msd_ctx = msd_connection_->MsdCreateContext2(priority);
  if (!msd_ctx)
    return MAGMA_DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Failed to create msd context");

  auto ctx = std::unique_ptr<MagmaSystemContext>(new MagmaSystemContext(this, std::move(msd_ctx)));

  context_map_.insert(std::make_pair(context_id, std::move(ctx)));
  return MAGMA_STATUS_OK;
}

magma::Status MagmaSystemConnection::DestroyContext(uint32_t context_id) {
  auto iter = context_map_.find(context_id);
  if (iter == context_map_.end()) {
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS,
                          "MagmaSystemConnection:Attempting to destroy invalid context id: %d",
                          context_id);
  }

  context_map_.erase(iter);
  return MAGMA_STATUS_OK;
}

MagmaSystemContext* MagmaSystemConnection::LookupContext(uint32_t context_id) {
  auto iter = context_map_.find(context_id);
  if (iter == context_map_.end())
    return MAGMA_DRETP(nullptr, "MagmaSystemConnection: Attempting to lookup invalid context id");

  return iter->second.get();
}

magma::Status MagmaSystemConnection::ExecuteCommandBuffers(
    uint32_t context_id, std::vector<magma_exec_command_buffer>& command_buffers,
    std::vector<magma_exec_resource>& resources, std::vector<uint64_t>& wait_semaphores,
    std::vector<uint64_t>& signal_semaphores, uint64_t flags) {
  auto context = LookupContext(context_id);
  if (!context)
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS,
                          "Attempting to execute command buffer on invalid context");

  return context->ExecuteCommandBuffers(command_buffers, resources, wait_semaphores,
                                        signal_semaphores, flags);
}

magma::Status MagmaSystemConnection::ExecuteInlineCommands(
    uint32_t context_id, std::vector<magma_inline_command_buffer> commands) {
  auto context = LookupContext(context_id);
  if (!context)
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS,
                          "Attempting to execute inline commands on invalid context");

  return context->ExecuteInlineCommands(std::move(commands));
}

magma::Status MagmaSystemConnection::EnablePerformanceCounterAccess(zx::handle access_token) {
  uint64_t perf_count_access_token_id = owner_->perf_count_access_token_id();
  MAGMA_DASSERT(perf_count_access_token_id);
  if (!access_token) {
    return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);
  }
  zx_info_handle_basic_t handle_info{};
  zx_status_t status = access_token.get_info(ZX_INFO_HANDLE_BASIC, &handle_info,
                                             sizeof(handle_info), nullptr, nullptr);
  if (status != ZX_OK) {
    return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);
  }
  if (handle_info.koid != perf_count_access_token_id) {
    // This is not counted as an error, since it can happen if the client uses the event from the
    // wrong driver.
    return MAGMA_STATUS_OK;
  }

  MAGMA_DLOG("Performance counter access enabled");
  can_access_performance_counters_ = true;
  return MAGMA_STATUS_OK;
}

magma::Status MagmaSystemConnection::ImportBuffer(zx::handle handle, uint64_t id) {
  auto buffer = magma::PlatformBuffer::Import(zx::vmo(std::move(handle)));
  if (!buffer)
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "failed to import buffer");

  buffer->set_local_id(id);

  auto iter = buffer_map_.find(id);
  if (iter != buffer_map_.end())
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "buffer id %lu already imported", id);

  auto sys_buffer = MagmaSystemBuffer::Create(owner_->driver(), std::move(buffer));
  MAGMA_DASSERT(sys_buffer);

  buffer_map_.insert({id, std::move(sys_buffer)});

  return MAGMA_STATUS_OK;
}

magma::Status MagmaSystemConnection::ReleaseBuffer(uint64_t id) {
  auto iter = buffer_map_.find(id);
  if (iter == buffer_map_.end())
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Attempting to free invalid buffer id %lu",
                          id);

  msd_connection()->MsdReleaseBuffer(*iter->second->msd_buf());
  buffer_map_.erase(iter);

  return MAGMA_STATUS_OK;
}

magma::Status MagmaSystemConnection::MapBuffer(uint64_t id, uint64_t hw_va, uint64_t offset,
                                               uint64_t length, uint64_t flags) {
  auto iter = buffer_map_.find(id);
  if (iter == buffer_map_.end())
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Attempting to map invalid buffer id %lu", id);

  if (length + offset < length)
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Offset overflows");

  if (length + offset > iter->second->size())
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Offset + length too large for buffer");

  if (!flags)
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Flags must be nonzero");

  magma::Status status =
      msd_connection()->MsdMapBuffer(*iter->second->msd_buf(), hw_va, offset, length, flags);
  if (!status.ok())
    return MAGMA_DRET_MSG(status.get(), "msd_connection_map_buffer failed");

  return MAGMA_STATUS_OK;
}

magma::Status MagmaSystemConnection::UnmapBuffer(uint64_t id, uint64_t hw_va) {
  auto iter = buffer_map_.find(id);
  if (iter == buffer_map_.end())
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Attempting to unmap invalid buffer id");

  magma::Status status = msd_connection()->MsdUnmapBuffer(*iter->second->msd_buf(), hw_va);
  if (!status.ok())
    return MAGMA_DRET_MSG(status.get(), "msd_connection_unmap_buffer failed");

  return MAGMA_STATUS_OK;
}

magma::Status MagmaSystemConnection::BufferRangeOp(uint64_t id, uint32_t op, uint64_t start,
                                                   uint64_t length) {
  auto iter = buffer_map_.find(id);
  if (iter == buffer_map_.end())
    return MAGMA_DRETF(false, "Attempting to commit invalid buffer id");
  if (start + length < start) {
    return MAGMA_DRETF(false, "Offset overflows");
  }
  if (start + length > iter->second->size()) {
    return MAGMA_DRETF(false, "Page offset too large for buffer");
  }
  return msd_connection()->MsdBufferRangeOp(*iter->second->msd_buf(), op, start, length);
}

void MagmaSystemConnection::SetNotificationCallback(
    msd::NotificationHandler* notification_handler) {
  if (notification_handler) {
    notification_handler_ = notification_handler;
    msd_connection()->MsdSetNotificationCallback(this);
  } else {
    msd_connection()->MsdSetNotificationCallback(nullptr);
  }
}

void MagmaSystemConnection::NotificationChannelSend(cpp20::span<uint8_t> data) {
  MAGMA_DASSERT(notification_handler_);
  notification_handler_->NotificationChannelSend(data);
}

void MagmaSystemConnection::ContextKilled() {
  MAGMA_DASSERT(notification_handler_);
  notification_handler_->ContextKilled();
}
void MagmaSystemConnection::PerformanceCounterReadCompleted(const msd::PerfCounterResult& result) {
  MAGMA_DASSERT(notification_handler_);
  std::lock_guard<std::mutex> lock(pool_map_mutex_);

  auto pool_it = pool_map_.find(result.pool_id);
  if (pool_it == pool_map_.end()) {
    MAGMA_DLOG("Driver attempted to lookup deleted pool id %ld\n", result.pool_id);
    return;
  }

  pool_it->second.platform_pool->SendPerformanceCounterCompletion(
      result.trigger_id, result.buffer_id, result.buffer_offset, result.timestamp,
      result.result_flags);
}

async_dispatcher_t* MagmaSystemConnection::GetAsyncDispatcher() {
  MAGMA_DASSERT(notification_handler_);
  return notification_handler_->GetAsyncDispatcher();
}

magma::Status MagmaSystemConnection::ImportObject(zx::handle handle, uint64_t flags,
                                                  fuchsia_gpu_magma::wire::ObjectType object_type,
                                                  uint64_t client_id) {
  if (!client_id)
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "client_id must be non zero");

  switch (object_type) {
    case fuchsia_gpu_magma::wire::ObjectType::kBuffer:
      return ImportBuffer(std::move(handle), client_id);

    case fuchsia_gpu_magma::wire::ObjectType::kSemaphore: {
      auto semaphore =
          MagmaSystemSemaphore::Create(owner_->driver(), std::move(handle), client_id, flags);
      if (!semaphore)
        return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "failed to import semaphore");

      auto iter = semaphore_map_.find(client_id);
      if (iter != semaphore_map_.end())
        return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "semaphore id %lu already imported",
                              client_id);

      semaphore_map_.insert(std::make_pair(client_id, std::move(semaphore)));
    } break;

    default:
      return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);
  }

  return MAGMA_STATUS_OK;
}

magma::Status MagmaSystemConnection::ReleaseObject(
    uint64_t object_id, fuchsia_gpu_magma::wire::ObjectType object_type) {
  switch (object_type) {
    case fuchsia_gpu_magma::wire::ObjectType::kBuffer:
      return ReleaseBuffer(object_id);

    case fuchsia_gpu_magma::wire::ObjectType::kSemaphore: {
      auto iter = semaphore_map_.find(object_id);
      if (iter == semaphore_map_.end())
        return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS,
                              "Attempting to release invalid semaphore id 0x%" PRIx64, object_id);

      semaphore_map_.erase(iter);
    } break;
    default:
      return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);
  }
  return MAGMA_STATUS_OK;
}

magma::Status MagmaSystemConnection::EnablePerformanceCounters(const uint64_t* counters,
                                                               uint64_t counter_count) {
  if (!can_access_performance_counters_)
    return MAGMA_DRET(MAGMA_STATUS_ACCESS_DENIED);

  return msd_connection()->MsdEnablePerformanceCounters(cpp20::span(counters, counter_count));
}

magma::Status MagmaSystemConnection::CreatePerformanceCounterBufferPool(
    std::unique_ptr<msd::PerfCountPoolServer> pool) {
  if (!can_access_performance_counters_)
    return MAGMA_DRET(MAGMA_STATUS_ACCESS_DENIED);

  uint64_t pool_id = pool->pool_id();
  if (pool_map_.count(pool_id))
    return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);

  {
    std::lock_guard<std::mutex> lock(pool_map_mutex_);
    pool_map_[pool_id].platform_pool = std::move(pool);
  }
  // |pool_map_mutex_| is unlocked before calling into the driver to prevent deadlocks if the driver
  // synchronously does MSD_CONNECTION_NOTIFICATION_PERFORMANCE_COUNTERS_READ_COMPLETED.
  magma_status_t status = msd_connection()->MsdCreatePerformanceCounterBufferPool(
      pool_id, &pool_map_[pool_id].msd_pool);
  if (status != MAGMA_STATUS_OK) {
    std::lock_guard<std::mutex> lock(pool_map_mutex_);
    pool_map_.erase(pool_id);
  }
  return MAGMA_STATUS_OK;
}

magma::Status MagmaSystemConnection::ReleasePerformanceCounterBufferPool(uint64_t pool_id) {
  if (!can_access_performance_counters_)
    return MAGMA_DRET(MAGMA_STATUS_ACCESS_DENIED);

  auto it = pool_map_.find(pool_id);
  if (it == pool_map_.end())
    return MAGMA_DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Invalid pool id %ld", pool_id);
  std::unique_ptr<msd::PerfCountPool>& msd_pool = it->second.msd_pool;

  // |pool_map_mutex_| is unlocked before calling into the driver to prevent deadlocks if the driver
  // synchronously does MSD_CONNECTION_NOTIFICATION_PERFORMANCE_COUNTERS_READ_COMPLETED.
  magma_status_t status =
      msd_connection()->MsdReleasePerformanceCounterBufferPool(std::move(msd_pool));
  {
    std::lock_guard<std::mutex> lock(pool_map_mutex_);
    pool_map_.erase(pool_id);
  }
  return MAGMA_DRET(status);
}

magma::Status MagmaSystemConnection::AddPerformanceCounterBufferOffsetToPool(uint64_t pool_id,
                                                                             uint64_t buffer_id,
                                                                             uint64_t buffer_offset,
                                                                             uint64_t buffer_size) {
  if (!can_access_performance_counters_)
    return MAGMA_DRET(MAGMA_STATUS_ACCESS_DENIED);
  std::shared_ptr<MagmaSystemBuffer> buffer = LookupBuffer(buffer_id);
  if (!buffer) {
    return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);
  }
  msd::PerfCountPool* msd_pool = LookupPerfCountPool(pool_id);
  if (!msd_pool) {
    return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);
  }
  magma_status_t status = msd_connection()->MsdAddPerformanceCounterBufferOffsetToPool(
      *msd_pool, *buffer->msd_buf(), buffer_id, buffer_offset, buffer_size);
  return MAGMA_DRET(status);
}

magma::Status MagmaSystemConnection::RemovePerformanceCounterBufferFromPool(uint64_t pool_id,
                                                                            uint64_t buffer_id) {
  if (!can_access_performance_counters_)
    return MAGMA_DRET(MAGMA_STATUS_ACCESS_DENIED);
  std::shared_ptr<MagmaSystemBuffer> buffer = LookupBuffer(buffer_id);
  if (!buffer) {
    return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);
  }

  msd::PerfCountPool* msd_pool = LookupPerfCountPool(pool_id);
  if (!msd_pool)
    return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);
  magma_status_t status =
      msd_connection()->MsdRemovePerformanceCounterBufferFromPool(*msd_pool, *buffer->msd_buf());

  return MAGMA_DRET(status);
}

magma::Status MagmaSystemConnection::DumpPerformanceCounters(uint64_t pool_id,
                                                             uint32_t trigger_id) {
  if (!can_access_performance_counters_)
    return MAGMA_DRET(MAGMA_STATUS_ACCESS_DENIED);
  msd::PerfCountPool* msd_pool = LookupPerfCountPool(pool_id);
  if (!msd_pool)
    return MAGMA_DRET(MAGMA_STATUS_INVALID_ARGS);
  return msd_connection()->MsdDumpPerformanceCounters(*msd_pool, trigger_id);
}

magma::Status MagmaSystemConnection::ClearPerformanceCounters(const uint64_t* counters,
                                                              uint64_t counter_count) {
  if (!can_access_performance_counters_)
    return MAGMA_DRET(MAGMA_STATUS_ACCESS_DENIED);
  return msd_connection()->MsdClearPerformanceCounters(cpp20::span(counters, counter_count));
}

std::shared_ptr<MagmaSystemBuffer> MagmaSystemConnection::LookupBuffer(uint64_t id) {
  auto iter = buffer_map_.find(id);
  if (iter == buffer_map_.end())
    return MAGMA_DRETP(nullptr, "Attempting to lookup invalid buffer id");

  return iter->second;
}

std::shared_ptr<MagmaSystemSemaphore> MagmaSystemConnection::LookupSemaphore(uint64_t id) {
  auto iter = semaphore_map_.find(id);
  if (iter == semaphore_map_.end())
    return nullptr;
  return iter->second;
}

msd::PerfCountPool* MagmaSystemConnection::LookupPerfCountPool(uint64_t id) {
  auto it = pool_map_.find(id);
  if (it == pool_map_.end())
    return MAGMA_DRETP(nullptr, "Invalid pool id %ld", id);
  return it->second.msd_pool.get();
}

}  // namespace msd
