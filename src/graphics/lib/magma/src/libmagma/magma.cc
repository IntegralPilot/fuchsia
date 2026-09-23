// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/result.h>
#include <lib/magma/magma.h>
#include <lib/magma/magma_common_defs.h>
#include <lib/magma/magma_logging.h>
#include <lib/magma/platform/platform_connection_client.h>
#include <lib/magma/platform/platform_device_client.h>
#include <lib/magma/platform/platform_handle.h>
#include <lib/magma/platform/platform_logger.h>
#include <lib/magma/platform/platform_logger_provider.h>
#include <lib/magma/platform/platform_object.h>
#include <lib/magma/platform/platform_port.h>
#include <lib/magma/platform/platform_semaphore.h>
#include <lib/magma/platform/platform_thread.h>
#include <lib/magma/platform/platform_trace.h>
#include <lib/magma/platform/platform_trace_provider.h>
#include <lib/magma/util/macros.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma/util/utils.h>
#if defined(MAGMA_ENABLE_TRACING)
#include <lib/trace-engine/instrumentation.h>
#include <zircon/threads.h>
#else
#error MAGMA_ENABLE_TRACING required
#endif

#include <atomic>
#include <chrono>
#include <map>

#include "enumerator.h"

namespace {

class IdGenerator {
 public:
  uint64_t get() {
    uint64_t id = counter_.fetch_add(1);
    DASSERT(id);
    return id;
  }

 private:
  std::atomic_uint64_t counter_ = 1;
};

IdGenerator s_semaphore_id_generator;

void set_buffer_local_id(magma::PlatformBuffer* platform_buffer) {
  // Local id must be locally unique so a buffer can be imported more than once.
  // It should also be globally unique to satisfy perfetto trace flow events, and
  // trace events in magma_connection)execute_command use local id because koid
  // isn't available.
  // TODO(https://fxbug.dev/458808956) - revert to the simple ID generator.
#if FUCHSIA_API_LEVEL_AT_LEAST(28)
  // Like TRACE_RANDOM_ID() but usable when tracing is disabled.
  uint64_t id = trace_time_based_id(thrd_get_zx_handle(thrd_current()));
#else
  static IdGenerator s_buffer_id_generator;
  uint64_t id = s_buffer_id_generator.get();
#endif
  platform_buffer->set_local_id(id);
}

}  // namespace

magma_status_t magma_enumerate_devices(const char* device_namespace,
                                       magma_handle_t device_directory_channel,
                                       uint32_t* device_count_inout, uint32_t device_path_size,
                                       char* device_paths_out) {
  magma::Enumerator enumerator(device_namespace, zx::channel(device_directory_channel));

  uint32_t device_path_count = *device_count_inout;

  fit::result result = enumerator.Enumerate(device_path_count, device_path_size, device_paths_out);
  if (result.is_ok()) {
    *device_count_inout = result.value();
    return MAGMA_STATUS_OK;
  }

  return result.error_value();
}

magma_status_t magma_device_import(uint32_t device_handle, magma_device_t* device) {
  auto platform_device_client = magma::PlatformDeviceClient::Create(device_handle);
  if (!platform_device_client) {
    return DRET(MAGMA_STATUS_INTERNAL_ERROR);
  }
  *device = reinterpret_cast<magma_device_t>(platform_device_client.release());
  return MAGMA_STATUS_OK;
}

void magma_device_release(magma_device_t device) {
  delete reinterpret_cast<magma::PlatformDeviceClient*>(device);
}

magma_status_t magma_device_query(magma_device_t device, uint64_t id,
                                  magma_handle_t* result_buffer_out, uint64_t* result_out) {
  auto platform_device_client = reinterpret_cast<magma::PlatformDeviceClient*>(device);

  return platform_device_client->Query(id, result_buffer_out, result_out).get();
}

magma_status_t magma_device_create_connection(magma_device_t device,
                                              magma_connection_t* connection_out) {
  auto platform_device_client = reinterpret_cast<magma::PlatformDeviceClient*>(device);

  auto connection = platform_device_client->Connect();
  if (!connection) {
    return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "couldn't connect");
  }

  *connection_out = reinterpret_cast<magma_connection_t>(connection.release());
  return MAGMA_STATUS_OK;
}

void magma_connection_release(magma_connection_t connection) {
  // TODO(https://fxbug.dev/42077901): close the connection
  delete magma::PlatformConnectionClient::cast(connection);
}

magma_status_t magma_connection_get_error(magma_connection_t connection) {
  return magma::PlatformConnectionClient::cast(connection)->GetError();
}

magma_status_t magma_connection_flush(magma_connection_t connection) {
  return magma::PlatformConnectionClient::cast(connection)->Flush();
}

magma_status_t magma_connection_create_context(magma_connection_t connection,
                                               uint32_t* context_id_out) {
  return magma::PlatformConnectionClient::cast(connection)->CreateContext(context_id_out);
}

magma_status_t magma_connection_create_context2(magma_connection_t connection,
                                                magma_priority_t priority,
                                                uint32_t* context_id_out) {
  return magma::PlatformConnectionClient::cast(connection)
      ->CreateContext2(context_id_out, priority);
}

void magma_connection_release_context(magma_connection_t connection, uint32_t context_id) {
  magma::PlatformConnectionClient::cast(connection)->DestroyContext(context_id);
}

magma_status_t magma_connection_create_buffer(magma_connection_t connection, uint64_t size,
                                              uint64_t* size_out, magma_buffer_t* buffer_out,
                                              magma_buffer_id_t* buffer_id_out) {
  auto platform_buffer = magma::PlatformBuffer::Create(size, "magma_create_buffer");
  if (!platform_buffer)
    return DRET(MAGMA_STATUS_MEMORY_ERROR);

  uint32_t handle;
  if (!platform_buffer->duplicate_handle(&handle))
    return DRET_MSG(MAGMA_STATUS_ACCESS_DENIED, "failed to duplicate handle");

  set_buffer_local_id(platform_buffer.get());

  magma_status_t result =
      magma::PlatformConnectionClient::cast(connection)
          ->ImportObject(handle, /*flags=*/0, magma::PlatformObject::BUFFER, platform_buffer->id());
  if (result != MAGMA_STATUS_OK)
    return DRET_MSG(result, "ImportObject failed");

  *size_out = platform_buffer->size();
  *buffer_id_out = platform_buffer->id();
  *buffer_out = reinterpret_cast<magma_buffer_t>(platform_buffer.release());  // Ownership passed

  return MAGMA_STATUS_OK;
}

void magma_connection_release_buffer(magma_connection_t connection, magma_buffer_t buffer) {
  auto platform_buffer = reinterpret_cast<magma::PlatformBuffer*>(buffer);
  magma::PlatformConnectionClient::cast(connection)
      ->ReleaseObject(platform_buffer->id(), magma::PlatformObject::BUFFER);
  delete platform_buffer;
}

magma_status_t magma_buffer_set_cache_policy(magma_buffer_t buffer, magma_cache_policy_t policy) {
  bool result = reinterpret_cast<magma::PlatformBuffer*>(buffer)->SetCachePolicy(policy);
  return result ? MAGMA_STATUS_OK : MAGMA_STATUS_INTERNAL_ERROR;
}

uint32_t magma_connection_get_notification_channel_handle(magma_connection_t connection) {
  return magma::PlatformConnectionClient::cast(connection)->GetNotificationChannelHandle();
}

magma_status_t magma_connection_read_notification_channel(magma_connection_t connection,
                                                          void* buffer, uint64_t buffer_size,
                                                          uint64_t* buffer_size_out,
                                                          magma_bool_t* more_data_out) {
  return magma::PlatformConnectionClient::cast(connection)
      ->ReadNotificationChannel(buffer, buffer_size, buffer_size_out, more_data_out);
}

magma_status_t magma_buffer_clean_cache(magma_buffer_t buffer, uint64_t offset, uint64_t size,
                                        magma_cache_operation_t operation) {
  auto platform_buffer = reinterpret_cast<magma::PlatformBuffer*>(buffer);
  bool invalidate;
  switch (operation) {
    case MAGMA_CACHE_OPERATION_CLEAN:
      invalidate = false;
      break;
    case MAGMA_CACHE_OPERATION_CLEAN_INVALIDATE:
      invalidate = true;
      break;
    default:
      return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "invalid cache operations");
  }

  bool result = platform_buffer->CleanCache(offset, size, invalidate);
  return result ? MAGMA_STATUS_OK : MAGMA_STATUS_INTERNAL_ERROR;
}

magma_status_t magma_connection_import_buffer(magma_connection_t connection, uint32_t buffer_handle,
                                              uint64_t* size_out, magma_buffer_t* buffer_out,
                                              magma_buffer_id_t* buffer_id_out) {
  auto platform_buffer = magma::PlatformBuffer::Import(buffer_handle);
  if (!platform_buffer)
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "PlatformBuffer::Import failed");

  uint32_t handle;
  if (!platform_buffer->duplicate_handle(&handle))
    return DRET_MSG(MAGMA_STATUS_ACCESS_DENIED, "failed to duplicate handle");

  set_buffer_local_id(platform_buffer.get());

  magma_status_t result =
      magma::PlatformConnectionClient::cast(connection)
          ->ImportObject(handle, /*flags=*/0, magma::PlatformObject::BUFFER, platform_buffer->id());
  if (result != MAGMA_STATUS_OK)
    return DRET_MSG(result, "ImportObject failed");

  *size_out = platform_buffer->size();
  *buffer_id_out = platform_buffer->id();
  *buffer_out = reinterpret_cast<magma_buffer_t>(platform_buffer.release());

  return MAGMA_STATUS_OK;
}

magma_status_t magma_buffer_export(magma_buffer_t buffer, uint32_t* buffer_handle_out) {
  auto platform_buffer = reinterpret_cast<magma::PlatformBuffer*>(buffer);

  if (!platform_buffer->duplicate_handle(buffer_handle_out))
    return DRET(MAGMA_STATUS_INVALID_ARGS);

  return MAGMA_STATUS_OK;
}

void magma_log(magma_log_severity_t severity, const char* tag, const char* file, int line,
               const char* format, va_list va) {
  magma::PlatformLogger::LogLevel level;
  switch (severity) {
    case MAGMA_LOG_WARNING:
      level = magma::PlatformLogger::LOG_WARNING;
      break;
    case MAGMA_LOG_ERROR:
      level = magma::PlatformLogger::LOG_ERROR;
      break;
    case MAGMA_LOG_INFO:
    default:
      level = magma::PlatformLogger::LOG_INFO;
  }
  magma::PlatformLogger::LogVa(level, file, line, format, va);
}

// Deprecated
void magma_fuchsia_log(int8_t severity, const char* tag, const char* file, int line,
                       const char* format, va_list va) {
  magma::PlatformLogger::LogLevel level;
  switch (severity) {
    case 0x30:
      level = magma::PlatformLogger::LOG_INFO;
      break;
    case 0x40:
      level = magma::PlatformLogger::LOG_WARNING;
      break;
    case 0x50:
      level = magma::PlatformLogger::LOG_ERROR;
      break;
    default:
      level = magma::PlatformLogger::LOG_INFO;
  }
  magma::PlatformLogger::LogVa(level, file, line, format, va);
}

magma_status_t magma_connection_map_buffer(magma_connection_t connection, uint64_t hw_va,
                                           magma_buffer_t buffer, uint64_t offset, uint64_t length,
                                           uint64_t map_flags) {
  auto platform_buffer = reinterpret_cast<magma::PlatformBuffer*>(buffer);
  uint64_t buffer_id = platform_buffer->id();
  return magma::PlatformConnectionClient::cast(connection)
      ->MapBuffer(buffer_id, hw_va, offset, length, map_flags);
}

magma_status_t magma_buffer_get_cache_policy(magma_buffer_t buffer,
                                             magma_cache_policy_t* cache_policy_out) {
  auto platform_buffer = reinterpret_cast<magma::PlatformBuffer*>(buffer);
  return platform_buffer->GetCachePolicy(cache_policy_out);
}

void magma_connection_unmap_buffer(magma_connection_t connection, uint64_t hw_va,
                                   magma_buffer_t buffer) {
  auto platform_buffer = reinterpret_cast<magma::PlatformBuffer*>(buffer);
  uint64_t buffer_id = platform_buffer->id();
  magma::PlatformConnectionClient::cast(connection)->UnmapBuffer(buffer_id, hw_va);
}

magma_status_t magma_connection_perform_buffer_op(magma_connection_t connection,
                                                  magma_buffer_t buffer, uint32_t options,
                                                  uint64_t start_offset, uint64_t length) {
  auto platform_buffer = reinterpret_cast<magma::PlatformBuffer*>(buffer);

  switch (options) {
    case MAGMA_BUFFER_RANGE_OP_POPULATE_TABLES:
    case MAGMA_BUFFER_RANGE_OP_DEPOPULATE_TABLES: {
      magma::PlatformConnectionClient::cast(connection)
          ->BufferRangeOp(platform_buffer->id(), options, start_offset, length);
      return MAGMA_STATUS_OK;
    }
  }

  if (start_offset % magma::page_size() != 0) {
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Invalid buffer offset %ld", start_offset);
  }
  if (length % magma::page_size() != 0) {
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Invalid buffer length %ld", length);
  }

  switch (options) {
    case MAGMA_BUFFER_RANGE_OP_COMMIT: {
      if (!platform_buffer->CommitPages(start_offset / magma::page_size(),
                                        length / magma::page_size()))
        return DRET(MAGMA_STATUS_MEMORY_ERROR);
      return MAGMA_STATUS_OK;
    }
    case MAGMA_BUFFER_RANGE_OP_DECOMMIT: {
      return platform_buffer
          ->DecommitPages(start_offset / magma::page_size(), length / magma::page_size())
          .get();
    }
    default:
      return DRET(MAGMA_STATUS_INVALID_ARGS);
  }
}

magma_status_t magma_buffer_get_info(magma_buffer_t buffer, magma_buffer_info_t* info_out) {
  auto platform_buffer = reinterpret_cast<magma::PlatformBuffer*>(buffer);
  if (!platform_buffer->GetBufferInfo(info_out))
    return DRET(MAGMA_STATUS_INTERNAL_ERROR);
  return MAGMA_STATUS_OK;
}

magma_status_t magma_connection_execute_command(magma_connection_t connection, uint32_t context_id,
                                                struct magma_command_descriptor* descriptor) {
  TRACE_DURATION("magma", "execute command", "count", descriptor->command_buffer_count);

  if (descriptor->command_buffer_count) {
    auto resource_index = descriptor->command_buffers[0].resource_index;
    DASSERT(resource_index < descriptor->resource_count);

    // Perfetto: the flow id must be unique in a trace.
    uint64_t ATTRIBUTE_UNUSED id = descriptor->resources[resource_index].buffer_id;
    TRACE_FLOW_BEGIN("magma", "command", id);
  }

  return magma::PlatformConnectionClient::cast(connection)->ExecuteCommand(context_id, descriptor);
}

magma_status_t magma_connection_execute_inline_commands(
    magma_connection_t connection, uint32_t context_id, uint64_t command_count,
    magma_inline_command_buffer* command_buffers) {
  uint64_t messages_sent;
  return magma::PlatformConnectionClient::cast(connection)
      ->ExecuteInlineCommands(context_id, command_count, command_buffers, &messages_sent);
}

magma_status_t magma_connection_create_semaphore(magma_connection_t connection,
                                                 magma_semaphore_t* semaphore_out,
                                                 magma_semaphore_id_t* id_out) {
  auto semaphore = magma::PlatformSemaphore::Create();
  if (!semaphore)
    return MAGMA_STATUS_MEMORY_ERROR;

  semaphore->set_local_id(s_semaphore_id_generator.get());

  uint32_t handle;
  if (!semaphore->duplicate_handle(&handle))
    return DRET_MSG(MAGMA_STATUS_ACCESS_DENIED, "failed to duplicate handle");

  magma_status_t result =
      magma::PlatformConnectionClient::cast(connection)
          ->ImportObject(handle, /*flags=*/0, magma::PlatformObject::SEMAPHORE, semaphore->id());
  if (result != MAGMA_STATUS_OK)
    return DRET_MSG(result, "failed to ImportObject");

  *id_out = semaphore->id();
  *semaphore_out = reinterpret_cast<magma_semaphore_t>(semaphore.release());

  return MAGMA_STATUS_OK;
}

void magma_connection_release_semaphore(magma_connection_t connection,
                                        magma_semaphore_t semaphore) {
  auto platform_semaphore = reinterpret_cast<magma::PlatformSemaphore*>(semaphore);
  magma::PlatformConnectionClient::cast(connection)
      ->ReleaseObject(platform_semaphore->id(), magma::PlatformObject::SEMAPHORE);
  delete platform_semaphore;
}

void magma_semaphore_signal(magma_semaphore_t semaphore) {
  reinterpret_cast<magma::PlatformSemaphore*>(semaphore)->Signal();
}

void magma_semaphore_reset(magma_semaphore_t semaphore) {
  reinterpret_cast<magma::PlatformSemaphore*>(semaphore)->Reset();
}

magma_status_t magma_poll(magma_poll_item_t* items, uint32_t count, uint64_t timeout_ns) {
  TRACE_DURATION("magma", "poll");

  // Optimize for simple case
  if (count == 1 && items[0].type == MAGMA_POLL_TYPE_SEMAPHORE &&
      items[0].condition == MAGMA_POLL_CONDITION_SIGNALED) {
    items[0].result = 0;
    // TODO(https://fxbug.dev/42126035): change WaitNoReset to take ns
    if (!reinterpret_cast<magma::PlatformSemaphore*>(items[0].semaphore)
             ->WaitNoReset(magma::ns_to_ms(timeout_ns)))
      return MAGMA_STATUS_TIMED_OUT;

    items[0].result = items[0].condition;
    return MAGMA_STATUS_OK;
  }

  std::unique_ptr<magma::PlatformPort> port = magma::PlatformPort::Create();
  if (!port)
    return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Failed to create port");

  // Map of key to item index
  std::map<uint64_t, uint32_t> map;

  for (uint32_t i = 0; i < count; i++) {
    items[i].result = 0;

    if (!items[i].condition)
      continue;

    switch (items[i].type) {
      case MAGMA_POLL_TYPE_SEMAPHORE: {
        if (items[i].condition != MAGMA_POLL_CONDITION_SIGNALED)
          return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Invalid condition for semaphore: 0x%x",
                          items[i].condition);
        auto semaphore = reinterpret_cast<magma::PlatformSemaphore*>(items[i].semaphore);

        uint64_t key = semaphore->global_id();
        if (!semaphore->WaitAsync(port.get(), key))
          return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "WaitAsync failed");

        DASSERT(map.find(key) == map.end());
        map[key] = i;
        break;
      }

      case MAGMA_POLL_TYPE_HANDLE: {
        if (items[i].condition != MAGMA_POLL_CONDITION_READABLE)
          return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Invalid condition for handle: 0x%x",
                          items[i].condition);

        auto platform_handle = magma::PlatformHandle::Create(items[i].handle);
        if (!platform_handle)
          return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Failed to create platform handle");

        uint64_t key = platform_handle->global_id();
        bool result = platform_handle->WaitAsync(port.get(), key);
        platform_handle->release();

        if (!result)
          return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "WaitAsync failed");

        DASSERT(map.find(key) == map.end());
        map[key] = i;
        break;
      }

      default:
        return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Invalid item type: %u", items[i].type);
    }
  }

  if (map.empty())
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Nothing to do");

  // TODO(https://fxbug.dev/42126035): change PlatformPort::Wait to take ns
  uint64_t key;
  magma::Status status = port->Wait(&key, magma::ns_to_ms(timeout_ns));
  if (!status)
    return status.get();

  while (status) {
    auto iter = map.find(key);
    if (iter == map.end())
      return DRET_MSG(MAGMA_STATUS_INTERNAL_ERROR, "Couldn't find key in map: 0x%lx", key);

    uint32_t index = iter->second;
    DASSERT(index < count);
    items[index].result = items[index].condition;

    // Check for more events
    status = port->Wait(&key, 0);
  }

  return MAGMA_STATUS_OK;
}

magma_status_t magma_semaphore_export(magma_semaphore_t semaphore, uint32_t* semaphore_handle_out) {
  auto platform_semaphore = reinterpret_cast<magma::PlatformSemaphore*>(semaphore);

  if (!platform_semaphore->duplicate_handle(semaphore_handle_out))
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "duplicate_handle failed");

  return MAGMA_STATUS_OK;
}

magma_status_t magma_connection_import_semaphore2(magma_connection_t connection,
                                                  uint32_t semaphore_handle, uint64_t flags,
                                                  magma_semaphore_t* semaphore_out,
                                                  magma_semaphore_id_t* id_out) {
  auto platform_semaphore = magma::PlatformSemaphore::Import(semaphore_handle, flags);
  if (!platform_semaphore)
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "PlatformSemaphore::Import failed");

  platform_semaphore->set_local_id(s_semaphore_id_generator.get());

  uint32_t handle;
  if (!platform_semaphore->duplicate_handle(&handle))
    return DRET_MSG(MAGMA_STATUS_ACCESS_DENIED, "failed to duplicate handle");

  magma_status_t result =
      magma::PlatformConnectionClient::cast(connection)
          ->ImportObject(handle, flags, magma::PlatformObject::SEMAPHORE, platform_semaphore->id());
  if (result != MAGMA_STATUS_OK)
    return DRET_MSG(result, "ImportObject failed: %d", result);

  *id_out = platform_semaphore->id();
  *semaphore_out = reinterpret_cast<magma_semaphore_t>(platform_semaphore.release());

  return MAGMA_STATUS_OK;
}

magma_status_t magma_initialize_tracing(magma_handle_t channel) {
  if (!channel)
    return MAGMA_STATUS_INVALID_ARGS;
  if (magma::PlatformTraceProvider::Get()) {
    if (magma::PlatformTraceProvider::Get()->IsInitialized())
      return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Shouldn't initialize tracing twice");
    if (!magma::PlatformTraceProvider::Get()->Initialize(channel))
      return DRET(MAGMA_STATUS_INTERNAL_ERROR);
  } else {
    // Close channel.
    magma::PlatformHandle::Create(channel);
  }
  return MAGMA_STATUS_OK;
}

magma_status_t magma_initialize_logging(magma_handle_t channel) {
  if (!channel)
    return MAGMA_STATUS_INVALID_ARGS;

  auto handle = magma::PlatformHandle::Create(channel);
  if (magma::PlatformLoggerProvider::IsInitialized())
    return DRET_MSG(MAGMA_STATUS_INVALID_ARGS, "Shouldn't initialize logging twice");

  if (!magma::PlatformLoggerProvider::Initialize(std::move(handle)))
    return MAGMA_STATUS_INTERNAL_ERROR;

  return MAGMA_STATUS_OK;
}

magma_status_t magma_connection_enable_performance_counter_access(magma_connection_t connection,
                                                                  magma_handle_t channel) {
  auto handle = magma::PlatformHandle::Create(channel);
  if (!handle)
    return DRET(MAGMA_STATUS_INVALID_ARGS);
  auto access_token = magma::PlatformConnectionClient::RetrieveAccessToken(handle.get());
  if (!access_token)
    return DRET(MAGMA_STATUS_INTERNAL_ERROR);
  magma_status_t result = magma::PlatformConnectionClient::cast(connection)
                              ->EnablePerformanceCounterAccess(std::move(access_token));
  if (result != MAGMA_STATUS_OK)
    return DRET_MSG(result, "EnablePerformanceCounterAccess failed: %d", result);
  bool enabled = false;
  result = magma::PlatformConnectionClient::cast(connection)
               ->IsPerformanceCounterAccessAllowed(&enabled);
  if (result != MAGMA_STATUS_OK)
    return DRET_MSG(result, "IsPerformanceCounterAccessEnabled failed: %d", result);

  return enabled ? MAGMA_STATUS_OK : MAGMA_STATUS_ACCESS_DENIED;
}

magma_status_t magma_connection_enable_performance_counters(magma_connection_t connection,
                                                            uint64_t* counters,
                                                            uint64_t counters_count) {
  return magma::PlatformConnectionClient::cast(connection)
      ->EnablePerformanceCounters(counters, counters_count)
      .get();
}

magma_status_t magma_connection_create_performance_counter_buffer_pool(
    magma_connection_t connection, magma_perf_count_pool_t* pool_out,
    magma_handle_t* notification_handle_out) {
  std::unique_ptr<magma::PlatformPerfCountPoolClient> client;
  magma::Status status = magma::PlatformConnectionClient::cast(connection)
                             ->CreatePerformanceCounterBufferPool(&client);
  if (!status.ok())
    return status.get();
  *notification_handle_out = client->handle();
  *pool_out = reinterpret_cast<magma_perf_count_pool_t>(client.release());
  return MAGMA_STATUS_OK;
}

magma_status_t magma_connection_release_performance_counter_buffer_pool(
    magma_connection_t connection, magma_perf_count_pool_t pool) {
  auto platform_pool = reinterpret_cast<magma::PlatformPerfCountPoolClient*>(pool);
  magma::Status status = magma::PlatformConnectionClient::cast(connection)
                             ->ReleasePerformanceCounterBufferPool(platform_pool->pool_id());
  delete platform_pool;
  return status.get();
}

magma_status_t magma_connection_add_performance_counter_buffer_offsets_to_pool(
    magma_connection_t connection, magma_perf_count_pool_t pool, const magma_buffer_offset* offsets,
    uint64_t offset_count) {
  auto platform_pool = reinterpret_cast<magma::PlatformPerfCountPoolClient*>(pool);
  return magma::PlatformConnectionClient::cast(connection)
      ->AddPerformanceCounterBufferOffsetsToPool(platform_pool->pool_id(), offsets, offset_count)
      .get();
}

magma_status_t magma_connection_remove_performance_counter_buffer_from_pool(
    magma_connection_t connection, magma_perf_count_pool_t pool, magma_buffer_t buffer) {
  auto platform_buffer = reinterpret_cast<magma::PlatformBuffer*>(buffer);
  auto platform_pool = reinterpret_cast<magma::PlatformPerfCountPoolClient*>(pool);

  return magma::PlatformConnectionClient::cast(connection)
      ->RemovePerformanceCounterBufferFromPool(platform_pool->pool_id(), platform_buffer->id())
      .get();
}

magma_status_t magma_connection_dump_performance_counters(magma_connection_t connection,
                                                          magma_perf_count_pool_t pool,
                                                          uint32_t trigger_id) {
  auto platform_pool = reinterpret_cast<magma::PlatformPerfCountPoolClient*>(pool);
  return magma::PlatformConnectionClient::cast(connection)
      ->DumpPerformanceCounters(platform_pool->pool_id(), trigger_id)
      .get();
}

magma_status_t magma_connection_clear_performance_counters(magma_connection_t connection,
                                                           uint64_t* counters,
                                                           uint64_t counters_count) {
  return magma::PlatformConnectionClient::cast(connection)
      ->ClearPerformanceCounters(counters, counters_count)
      .get();
}

magma_status_t magma_connection_read_performance_counter_completion(
    magma_connection_t connection, magma_perf_count_pool_t pool, uint32_t* trigger_id_out,
    uint64_t* buffer_id_out, uint32_t* buffer_offset_out, uint64_t* time_out,
    uint32_t* result_flags_out) {
  auto platform_pool = reinterpret_cast<magma::PlatformPerfCountPoolClient*>(pool);
  return platform_pool
      ->ReadPerformanceCounterCompletion(trigger_id_out, buffer_id_out, buffer_offset_out, time_out,
                                         result_flags_out)
      .get();
}

magma_status_t magma_buffer_set_name(magma_buffer_t buffer, const char* name) {
  if (!reinterpret_cast<magma::PlatformBuffer*>(buffer)->SetName(name))
    return DRET(MAGMA_STATUS_INTERNAL_ERROR);
  return MAGMA_STATUS_OK;
}

magma_status_t magma_buffer_get_handle(magma_buffer_t buffer, magma_handle_t* handle_out) {
  if (!reinterpret_cast<magma::PlatformBuffer*>(buffer)->duplicate_handle(handle_out))
    return DRET(MAGMA_STATUS_INVALID_ARGS);
  return MAGMA_STATUS_OK;
}
