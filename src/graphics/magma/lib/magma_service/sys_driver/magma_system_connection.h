// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_CONNECTION_H_
#define SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_CONNECTION_H_

#include <lib/magma/util/macros.h>
#include <lib/magma_service/msd.h>

#include <memory>
#include <mutex>
#include <unordered_map>
#include <vector>

#include "fidl/fuchsia.gpu.magma/cpp/wire_types.h"
#include "magma_system_buffer.h"
#include "magma_system_context.h"
#include "primary_fidl_server.h"

namespace msd {
class MagmaSystemDevice;

class MagmaSystemConnection : private MagmaSystemContext::Owner,
                              public msd::internal::PrimaryFidlServer::Delegate,
                              msd::NotificationHandler {
 public:
  class Owner {
   public:
    virtual msd::Driver* driver() = 0;
    virtual uint64_t perf_count_access_token_id() const = 0;
    virtual uint32_t GetDeviceId() = 0;
  };

  // `owner` must outlive the MagmaSystemConnection.
  MagmaSystemConnection(Owner* owner, std::unique_ptr<msd::Connection> msd_connection_t);

  ~MagmaSystemConnection() override;

  magma::Status ImportObject(zx::handle handle, uint64_t flags,
                             fuchsia_gpu_magma::wire::ObjectType object_type,
                             uint64_t client_id) override;
  magma::Status ReleaseObject(uint64_t object_id,
                              fuchsia_gpu_magma::wire::ObjectType object_type) override;
  magma::Status CreateContext(uint32_t context_id) override;
  magma::Status CreateContext2(uint32_t context_id, uint64_t priority) override;
  magma::Status DestroyContext(uint32_t context_id) override;
  magma::Status ExecuteCommandBuffers(uint32_t context_id,
                                      std::vector<magma_exec_command_buffer>& command_buffers,
                                      std::vector<magma_exec_resource>& resources,
                                      std::vector<uint64_t>& wait_semaphores,
                                      std::vector<uint64_t>& signal_semaphores,
                                      uint64_t flags) override;
  magma::Status MapBuffer(uint64_t buffer_id, uint64_t hw_va, uint64_t offset, uint64_t length,
                          uint64_t flags) override;
  magma::Status UnmapBuffer(uint64_t buffer_id, uint64_t hw_va) override;
  magma::Status BufferRangeOp(uint64_t buffer_id, uint32_t op, uint64_t start,
                              uint64_t length) override;
  magma::Status ExecuteInlineCommands(uint32_t context_id,
                                      std::vector<magma_inline_command_buffer> commands) override;
  MagmaSystemContext* LookupContext(uint32_t context_id);
  void SetNotificationCallback(msd::NotificationHandler*) override;
  magma::Status EnablePerformanceCounterAccess(zx::handle access_token) override;
  bool IsPerformanceCounterAccessAllowed() override { return can_access_performance_counters_; }
  magma::Status EnablePerformanceCounters(const uint64_t* counters,
                                          uint64_t counter_count) override;
  magma::Status CreatePerformanceCounterBufferPool(
      std::unique_ptr<msd::PerfCountPoolServer> pool) override;
  magma::Status ReleasePerformanceCounterBufferPool(uint64_t pool_id) override;
  magma::Status AddPerformanceCounterBufferOffsetToPool(uint64_t pool_id, uint64_t buffer_id,
                                                        uint64_t buffer_offset,
                                                        uint64_t buffer_size) override;
  magma::Status RemovePerformanceCounterBufferFromPool(uint64_t pool_id,
                                                       uint64_t buffer_id) override;
  magma::Status DumpPerformanceCounters(uint64_t pool_id, uint32_t trigger_id) override;
  magma::Status ClearPerformanceCounters(const uint64_t* counters, uint64_t counter_count) override;

  // msd::NotificationHandler implementation.
  void NotificationChannelSend(cpp20::span<uint8_t> data) override;
  void ContextKilled() override;
  void PerformanceCounterReadCompleted(const msd::PerfCounterResult& result) override;
  async_dispatcher_t* GetAsyncDispatcher() override;

  // Create a buffer from the handle and add it to the map,
  // on success |id_out| contains the id to be used to query the map
  magma::Status ImportBuffer(zx::handle handle, uint64_t id);
  // This removes the reference to the shared_ptr in the map
  // other instances remain valid until deleted
  // Returns false if no buffer with the given |id| exists in the map
  magma::Status ReleaseBuffer(uint64_t id);

  // Attempts to locate a buffer by |id| in the buffer map and return it.
  // Returns nullptr if the buffer is not found
  std::shared_ptr<MagmaSystemBuffer> LookupBuffer(uint64_t id);

  // Returns the msd_semaphore for the given |id| if present in the semaphore map.
  std::shared_ptr<MagmaSystemSemaphore> LookupSemaphore(uint64_t id);

  uint32_t GetDeviceId();

  msd::Connection* msd_connection() { return msd_connection_.get(); }

  void set_can_access_performance_counters(bool can_access) {
    can_access_performance_counters_ = can_access;
  }

 private:
  struct PoolReference {
    std::unique_ptr<msd::PerfCountPool> msd_pool;
    std::unique_ptr<msd::PerfCountPoolServer> platform_pool;
  };

  // MagmaSystemContext::Owner
  std::shared_ptr<MagmaSystemBuffer> LookupBufferForContext(uint64_t id) override {
    return LookupBuffer(id);
  }
  std::shared_ptr<MagmaSystemSemaphore> LookupSemaphoreForContext(uint64_t id) override {
    return LookupSemaphore(id);
  }

  // The returned value is valid until ReleasePerformanceCounterBufferPool is called on it.
  // Otherwise it will always be valid within the lifetime of a call into MagmaSystemConnection on
  // the connection thread.
  msd::PerfCountPool* LookupPerfCountPool(uint64_t id);

  // The owner (MagmaSystemDevice) will call Shutdown() and join the connection thread before
  // exiting.
  Owner* owner_;
  std::unique_ptr<msd::Connection> msd_connection_;
  std::unordered_map<uint32_t, std::unique_ptr<MagmaSystemContext>> context_map_;
  std::unordered_map<uint64_t, std::shared_ptr<MagmaSystemBuffer>> buffer_map_;
  std::unordered_map<uint64_t, std::shared_ptr<MagmaSystemSemaphore>> semaphore_map_;

  msd::NotificationHandler* notification_handler_ = nullptr;

  // |pool_map_mutex_| should not be held while calling into the driver. It must be held for
  // modifications to pool_map_ and accesses to pool_map_ from a thread that's not the connection
  // thread.
  std::mutex pool_map_mutex_;
  std::unordered_map<uint64_t, PoolReference> pool_map_;
  bool can_access_performance_counters_ = false;
};

}  // namespace msd

#endif  // SRC_GRAPHICS_MAGMA_LIB_MAGMA_SERVICE_SYS_DRIVER_MAGMA_SYSTEM_CONNECTION_H_
