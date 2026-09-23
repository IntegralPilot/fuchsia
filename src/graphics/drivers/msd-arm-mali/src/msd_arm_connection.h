// Copyright 2017 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef MSD_ARM_CONNECTION_H
#define MSD_ARM_CONNECTION_H

#include <lib/fit/function.h>
#include <lib/fit/thread_safety.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/magma/util/short_macros.h>
#include <lib/magma_service/msd.h>
#include <lib/magma_service/msd_defs.h>
#include <lib/magma_service/util/address_space_allocator.h>
#include <zircon/compiler.h>

#include <deque>
#include <list>
#include <map>
#include <memory>
#include <mutex>
#include <thread>
#include <unordered_map>
#include <vector>

#include "src/graphics/drivers/msd-arm-mali/include/magma_arm_mali_types.h"
#include "src/graphics/drivers/msd-arm-mali/src/address_space.h"
#include "src/graphics/drivers/msd-arm-mali/src/device_request.h"
#include "src/graphics/drivers/msd-arm-mali/src/gpu_mapping.h"
#include "src/graphics/drivers/msd-arm-mali/src/msd_arm_atom.h"
#include "src/graphics/drivers/msd-arm-mali/src/msd_arm_buffer.h"
#include "src/graphics/drivers/msd-arm-mali/src/msd_arm_semaphore.h"
#include "src/graphics/drivers/msd-arm-mali/src/performance_counters_manager.h"

struct magma_arm_mali_atom;

class MsdArmPerfCountPool;
class PerformanceCounters;

// This can only be accessed on the connection thread.
class MsdArmConnection : public std::enable_shared_from_this<MsdArmConnection>,
                         public GpuMapping::Owner,
                         public AddressSpace::Owner {
 public:
  class Owner {
   public:
    // Method names should be Ndt (Not Device Thread) prefixed to indicate that these
    // are called from a connection thread and their implementation must be threadsafe.
    virtual void NdtPostScheduleAtom(std::shared_ptr<MsdArmAtom> atom) = 0;
    virtual void NdtPostCancelAtoms(std::shared_ptr<MsdArmConnection> connection) = 0;
    virtual AddressSpaceObserver* NdtGetAddressSpaceObserver() = 0;
    virtual ArmMaliCacheCoherencyStatus NdtGetCacheCoherencyStatus() = 0;
    virtual magma::PlatformBusMapper* NdtGetBusMapper() = 0;
    virtual bool NdtIsProtectedModeSupported() = 0;
    // Called after the connection's destructor has been called, so the
    // refcount should be 0.
    virtual void NdtDeregisterConnection() = 0;
    virtual void NdtSetCurrentThreadToDefaultPriority() = 0;
    virtual std::shared_ptr<DeviceRequest::Reply> NdtPostTask(FitCallbackTask task) = 0;
    virtual std::thread::id NdtGetDeviceThreadId() = 0;
    virtual msd::MagmaMemoryPressureLevel NdtGetCurrentMemoryPressureLevel() = 0;
  };

  static std::shared_ptr<MsdArmConnection> Create(msd::msd_client_id_t client_id, Owner* owner);

  virtual ~MsdArmConnection();

  msd::msd_client_id_t client_id() { return client_id_; }

  void InitializeInspectNode(inspect::Node* parent);

  AddressSpace* address_space_for_testing() __TA_NO_THREAD_SAFETY_ANALYSIS {
    return address_space_.get();
  }
  const AddressSpace* const_address_space() const __TA_NO_THREAD_SAFETY_ANALYSIS {
    return address_space_.get();
  }

  // GpuMapping::Owner implementation.
  bool RemoveMapping(uint64_t gpu_va) override;
  bool UpdateCommittedMemory(GpuMapping* mapping) override;

  bool AddMapping(std::unique_ptr<GpuMapping> mapping);
  bool ExecuteAtom(size_t* remaining_data_size, magma_arm_mali_atom* atom,
                   std::vector<std::shared_ptr<magma::PlatformSemaphore>>& semaphores,
                   std::deque<std::shared_ptr<magma::PlatformSemaphore>>* deprecated_semaphores);

  void SetNotificationCallback(msd::NotificationHandler* handler);
  void SendNotificationData(MsdArmAtom* atom);
  void MarkDestroyed();

  // Returns the number of bytes freed due to the change.
  size_t PeriodicMemoryPressureCallback();

  // Tries to allocate JIT memory for an atom.  Returns a status if allocation
  // finished (successfully or not) or no status if the allocation needs to be
  // retried after a free is completed.
  [[nodiscard]] std::optional<ArmMaliResultCode> AllocateJitMemory(
      const std::shared_ptr<MsdArmSoftAtom>& atom);
  // Process a JIT memory free operation. Doesn't modify the result code of the atom.
  void ReleaseJitMemory(const std::shared_ptr<MsdArmSoftAtom>& atom);

  // Called only on device thread.
  void set_address_space_lost() { address_space_lost_ = true; }
  bool address_space_lost() const { return address_space_lost_; }

  AddressSpaceObserver* GetAddressSpaceObserver() override {
    return owner_->NdtGetAddressSpaceObserver();
  }
  std::shared_ptr<AddressSpace::Owner> GetSharedPtr() override { return shared_from_this(); }

  bool PageInMemory(uint64_t address);
  bool SetCommittedPagesForBuffer(MsdArmBuffer* buffer, uint64_t page_offset, uint64_t page_count);
  bool CommitMemoryForBuffer(MsdArmBuffer* buffer, uint64_t page_offset, uint64_t page_count);
  bool DecommitMemoryForBuffer(MsdArmBuffer* buffer, uint64_t page_offset, uint64_t page_count);

  // This is slow because it iterates over all pages for all mappings. It should be used only
  // rarely.
  bool GetVirtualAddressFromPhysical(uint64_t address, uint64_t* virtual_address_out);

  void IncrementContextCount() { context_count_++; }
  void DecrementContextCount() { context_count_--; }
  uint64_t context_count() const { return context_count_; }

  void SendPerfCounterNotification(const msd::PerfCounterResult& results);

  magma_status_t EnablePerformanceCounters(std::vector<uint64_t> flags);
  magma_status_t DumpPerformanceCounters(std::shared_ptr<MsdArmPerfCountPool> pool,
                                         uint32_t trigger_id);
  magma_status_t ReleasePerformanceCounterBufferPool(std::shared_ptr<MsdArmPerfCountPool> pool);
  magma_status_t AddPerformanceCounterBufferOffsetToPool(std::shared_ptr<MsdArmPerfCountPool> pool,
                                                         std::shared_ptr<MsdArmBuffer> buffer,
                                                         uint64_t buffer_id, uint64_t buffer_offset,
                                                         uint64_t buffer_size);
  magma_status_t RemovePerformanceCounterBufferFromPool(std::shared_ptr<MsdArmPerfCountPool> pool,
                                                        std::shared_ptr<MsdArmBuffer> buffer);

  std::thread::id GetDeviceThreadId() { return owner_->NdtGetDeviceThreadId(); }

 private:
  static const uint32_t kMagic = 0x636f6e6e;  // "conn" (Connection)
  friend class TestConnection;

  struct JitMemoryRegion {
    // ID the client uses to refer to this region while it's allocated. If 0, the region is not
    // currently in use.
    uint8_t id;
    // Bin ID of the region. Bin IDs must match for the region to be reused.
    uint8_t bin_id;
    // Usage ID of the region. Usage ID preferably matches.
    uint16_t usage_id;
    uint64_t gpu_address;
    // Number of initial committed pages requested. The region may grow in size
    // while in use, and may be shrunk when freed.
    uint64_t committed_pages;
    std::shared_ptr<MsdArmBuffer> buffer;
    inspect::Node node;
    inspect::UintProperty id_property;
    inspect::UintProperty comitted_page_count_property;
    inspect::UintProperty requested_comitted_pages_property;
  };

  struct JitProperties {
    uint8_t trim_level;
    uint8_t max_allocations;
  };

  struct ConnectionPerfCountManager final : public PerformanceCountersManager {
    // PerformanceCountersManager implementation. Only called on device thread.
    std::vector<uint64_t> EnabledPerfCountFlags() override { return enabled_performance_counters_; }

    // Only modified on device thread.
    std::vector<uint64_t> enabled_performance_counters_;
  };

  MsdArmConnection(msd::msd_client_id_t client_id, Owner* owner);

  bool Init();

  JitMemoryRegion* FindBestJitRegionAddressWithUsage(const magma_arm_jit_memory_allocate_info& info,
                                                     bool check_usage) FIT_REQUIRES(address_lock_);
  uint64_t FindBestJitRegionAddress(const magma_arm_jit_memory_allocate_info& info);
  std::optional<ArmMaliResultCode> AllocateNewJitMemoryRegion(
      const magma_arm_jit_memory_allocate_info& info, uint64_t* address_out);
  ArmMaliResultCode WriteJitRegionAdddress(const magma_arm_jit_memory_allocate_info& info,
                                           uint64_t address);

  // Returns a result code on success or failure. Returns no value if allocation is delayed.
  std::optional<ArmMaliResultCode> AllocateOneJitMemoryRegion(
      const magma_arm_jit_memory_allocate_info& info);
  void ReleaseOneJitMemory(const magma_arm_jit_memory_free_info& info);

  // Release all unused JIT regions to save memory. Returns the number of bytes freed.
  size_t FreeUnusedJitRegionsIfNeeded() FIT_REQUIRES(address_lock_);
  bool RemoveMappingLocked(uint64_t gpu_va) FIT_REQUIRES(address_lock_);

  magma::PlatformBusMapper* GetBusMapper() override { return owner_->NdtGetBusMapper(); }

  msd::msd_client_id_t client_id_;

  inspect::Node node_;
  inspect::Node jit_regions_;
  inspect::UintProperty client_id_property_;
  inspect::UintProperty slow_atom_count_property_;
  inspect::UintProperty slow_semaphore_set_atom_count_property_;

  std::mutex address_lock_;
  __THREAD_ANNOTATION(__pt_guarded_by__(address_lock_))
  std::unique_ptr<AddressSpace> address_space_;
  // Map GPU va to a mapping.
  FIT_GUARDED(address_lock_) std::map<uint64_t, std::unique_ptr<GpuMapping>> gpu_mappings_;
  FIT_GUARDED(address_lock_) JitProperties jit_properties_;
  FIT_GUARDED(address_lock_) std::list<JitMemoryRegion> jit_memory_regions_;
  FIT_GUARDED(address_lock_) std::unique_ptr<magma::AddressSpaceAllocator> jit_allocator_;

  // Store a list of a small number of mappings to help debug issues when references to freed
  // memory.
  static constexpr uint32_t kMaxStoredRemovedMappings = 64;
  std::deque<std::pair</*gpu_va=*/uint64_t, /*len=*/uint64_t>> recently_removed_mappings_;

  Owner* owner_;

  // Modified and accessed only from device thread.
  bool address_space_lost_ = false;

  std::mutex callback_lock_;
  msd::NotificationHandler* notification_handler_{};
  std::shared_ptr<MsdArmAtom> outstanding_atoms_[256];
  std::atomic<uint32_t> terminated_atoms_{0};
  std::atomic<uint32_t> context_count_{0};
  std::atomic<uint64_t> received_atom_count_{0};
  std::atomic<uint64_t> notified_atom_count_{0};
  std::vector<magma_arm_mali_status> coalescing_notifications_ FIT_GUARDED(callback_lock_);

  std::shared_ptr<ConnectionPerfCountManager> perf_count_manager_;
};

class MsdArmAbiConnection : public msd::Connection {
 public:
  MsdArmAbiConnection(std::shared_ptr<MsdArmConnection> ptr) : ptr_(std::move(ptr)) {
    magic_ = kMagic;
  }

  magma_status_t MsdMapBuffer(msd::Buffer& buffer, uint64_t gpu_va, uint64_t offset,
                              uint64_t length, uint64_t flags) override;
  magma_status_t MsdUnmapBuffer(msd::Buffer& buffer, uint64_t gpu_va) override;
  magma_status_t MsdBufferRangeOp(msd::Buffer& buffer, uint32_t options, uint64_t start_offset,
                                  uint64_t length) override;
  void MsdReleaseBuffer(msd::Buffer& buffer, bool shutting_down) override;

  void MsdSetNotificationCallback(msd::NotificationHandler* handler) override;
  std::unique_ptr<msd::Context> MsdCreateContext() override;

  magma_status_t MsdEnablePerformanceCounters(cpp20::span<const uint64_t> counters) override;

  magma_status_t MsdCreatePerformanceCounterBufferPool(
      uint64_t pool_id, std::unique_ptr<msd::PerfCountPool>* pool_out) override;
  magma_status_t MsdReleasePerformanceCounterBufferPool(
      std::unique_ptr<msd::PerfCountPool> pool) override;

  magma_status_t MsdAddPerformanceCounterBufferOffsetToPool(msd::PerfCountPool& pool,
                                                            msd::Buffer& buffer, uint64_t buffer_id,
                                                            uint64_t buffer_offset,
                                                            uint64_t buffer_size) override;

  magma_status_t MsdRemovePerformanceCounterBufferFromPool(msd::PerfCountPool& pool,
                                                           msd::Buffer& buffer) override;

  magma_status_t MsdDumpPerformanceCounters(msd::PerfCountPool& pool, uint32_t trigger_id) override;

  magma_status_t MsdClearPerformanceCounters(cpp20::span<const uint64_t> counters) override;

  std::shared_ptr<MsdArmConnection> ptr() { return ptr_; }

 private:
  std::shared_ptr<MsdArmConnection> ptr_;
  static const uint32_t kMagic = 0x636f6e6e;  // "conn" (Connection)
  uint32_t magic_;
};

#endif  // MSD_ARM_CONNECTION_H
