// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_BLOCK_DEVICE_H_
#define SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_BLOCK_DEVICE_H_

#include <fidl/fuchsia.hardware.cqhci/cpp/driver/fidl.h>
#include <fidl/fuchsia.hardware.cqhci/cpp/driver/wire.h>
#include <fidl/fuchsia.hardware.inlineencryption/cpp/wire.h>
#include <fidl/fuchsia.hardware.sdmmc/cpp/wire.h>
#include <fidl/fuchsia.power.broker/cpp/fidl.h>
#include <fidl/fuchsia.storage.block/cpp/wire.h>
#include <lib/driver/component/cpp/driver_base.h>
#include <lib/driver/logging/cpp/logger.h>
#include <lib/driver/power/cpp/power-support.h>
#include <lib/driver/power/cpp/suspend.h>
#include <lib/fzl/vmo-mapper.h>
#include <lib/inspect/component/cpp/component.h>
#include <lib/inspect/cpp/inspect.h>
#include <lib/sdmmc/hw.h>
#include <lib/sync/cpp/completion.h>
#include <lib/trace/event.h>
#include <lib/zircon-internal/thread_annotations.h>
#include <lib/zx/result.h>
#include <threads.h>
#include <zircon/types.h>

#include <array>
#include <atomic>
#include <cinttypes>
#include <deque>
#include <memory>
#include <semaphore>

#include <fbl/auto_lock.h>
#include <fbl/condition_variable.h>

#include "sdmmc-device.h"
#include "sdmmc-partition-device.h"
#include "sdmmc-rpmb-device.h"
#include "sdmmc-types.h"

namespace sdmmc {

class SdmmcRootDevice;

// This struct is used to maintain metadata for IO while the IO is in progress: When using Banjo,
// this is a hard requirement, as only object handles are passed through the call. When using FIDL,
// objects can be transferred through the call. However for performance, we avoid repeatedly
// creating and initializing objects.
struct ReadWriteMetadata {
 public:
  ReadWriteMetadata() = default;

  // This initialization is only required if packed commands are used.
  zx_status_t InitForPackedCommands(uint32_t buffer_region_count, uint32_t block_size) {
    // Skip initialization if we already acquired these resources in a previous call to ProbeMmc().
    if (packed_command_vmo.is_valid() && packed_command_mapper.start()) {
      return ZX_OK;
    }

    buffer_regions = std::make_unique<sdmmc_buffer_region_t[]>(buffer_region_count);
    memset(buffer_regions.get(), 0, sizeof(sdmmc_buffer_region_t) * buffer_region_count);

    // Create a VMO large enough to hold both the packed command header and EXT_CSD.
    zx_status_t status = zx::vmo::create(block_size + MMC_EXT_CSD_SIZE, 0, &packed_command_vmo);
    if (status != ZX_OK) {
      fdf::error("Failed to create packed command header vmo: {}", zx_status_get_string(status));
      return status;
    }

    status = packed_command_mapper.Map(packed_command_vmo);
    if (status != ZX_OK) {
      fdf::error("Failed to map packed command header vmo: {}", zx_status_get_string(status));
      return status;
    }

    packed_command_header_data = static_cast<PackedCommand*>(packed_command_mapper.start());
    ext_csd = {static_cast<uint8_t*>(packed_command_mapper.start()) + block_size, MMC_EXT_CSD_SIZE};

    // Do an initial cache flush of EXT_CSD to put the physical memory in a known state. EXT_CSD
    // will be read-only after this.
    zx_cache_flush(ext_csd.data(), ext_csd.size(), ZX_CACHE_FLUSH_DATA);
    return ZX_OK;
  }

  uint64_t ext_csd_offset() const {
    return ext_csd.data() - static_cast<uint8_t*>(packed_command_mapper.start());
  }

  // For non-packed commands, only this is needed, as initialized here.
  std::unique_ptr<sdmmc_buffer_region_t[]> buffer_regions =
      std::make_unique<sdmmc_buffer_region_t[]>(1);

  // For packed commands, the following are also needed.
  zx::vmo packed_command_vmo;
  std::span<const uint8_t> ext_csd;
  fzl::VmoMapper packed_command_mapper;
  PackedCommand* packed_command_header_data;
};

class SdmmcBlockDevice : public fdf::WireServer<fuchsia_hardware_cqhci::Cqhci>,
                         public fidl::WireServer<fuchsia_hardware_inlineencryption::Device> {
 public:
  static constexpr char kHardwarePowerElementName[] = "sdmmc-hardware";

  // Power levels for the sdmmc-hardware element.
  static constexpr uint8_t kPowerLevelOff = 0;
  static constexpr uint8_t kPowerLevelOn = 1;

  // Implement fuchsia.power.broker.ElementRunner, allowing Power Broker
  // to set this device's power level.
  void SetLevel(uint8_t level);

  void Suspend(fdf_power::SuspendCompleter completer);
  void Resume(fdf_power::ResumeCompleter completer);
  // We always return true here, if our config says that we don't support power
  // management, suspend and resume will be no-ops.
  bool SuspendEnabled() { return true; }

  // fuchsia.hardware.cqhci.Cqhci
  void HostInfo(fdf::Arena& arena, HostInfoCompleter::Sync& completer) override;
  void InitializeCommandQueueing(InitializeCommandQueueingRequestView request, fdf::Arena& arena,
                                 InitializeCommandQueueingCompleter::Sync& completer) override;
  void EnableCqhci(fdf::Arena& arena, EnableCqhciCompleter::Sync& completer) override;
  void DisableCqhci(fdf::Arena& arena, DisableCqhciCompleter::Sync& completer) override;

  SdmmcBlockDevice(SdmmcRootDevice* parent, std::unique_ptr<SdmmcDevice> sdmmc)
      : parent_(parent), sdmmc_(std::move(sdmmc)) {
    block_info_.max_transfer_size = static_cast<uint32_t>(sdmmc_->host_info().max_transfer_size);
  }

  static zx_status_t Create(SdmmcRootDevice* parent, std::unique_ptr<SdmmcDevice> sdmmc,
                            std::unique_ptr<SdmmcBlockDevice>* out_dev);
  // Returns the SdmmcDevice. Used if this SdmmcBlockDevice fails to probe (i.e., no eligible device
  // present).
  std::unique_ptr<SdmmcDevice> TakeSdmmcDevice() { return std::move(sdmmc_); }

  // Probe for SD first, then MMC.
  zx_status_t Probe(const fuchsia_hardware_sdmmc::SdmmcMetadata& metadata) TA_EXCL(worker_lock_) {
    metadata_ = metadata;
    fbl::AutoLock lock(&worker_lock_);
    return ProbeSdLocked() == ZX_OK ? ZX_OK : ProbeMmcLocked();
  }
  zx_status_t ProbeSd() TA_EXCL(worker_lock_) {
    fbl::AutoLock lock(&worker_lock_);
    return ProbeSdLocked();
  }
  zx_status_t ProbeMmc() TA_EXCL(worker_lock_) {
    fbl::AutoLock lock(&worker_lock_);
    return ProbeMmcLocked();
  }

  zx_status_t AddDevice() TA_EXCL(queue_lock_);

  void StopWorkerDispatcher(std::optional<fdf::StopCompleter> completer = std::nullopt)
      TA_EXCL(queue_lock_);
  void SendPowerOffNotification();

  void SetPowerSuspendedForTest(bool suspended) TA_EXCL(worker_lock_) {
    fbl::AutoLock lock(&worker_lock_);
    power_suspended_ = suspended;
    worker_condition_.Broadcast();
  }

  zx_status_t SuspendPower() TA_REQ(worker_lock_);
  zx_status_t ResumePower() TA_REQ(worker_lock_);

  // Called by children of this device.
  void RpmbQueue(RpmbRequestInfo info) TA_EXCL(queue_lock_);
  fidl::WireSyncClient<fuchsia_driver_framework::Node>& block_node() { return block_node_; }
  const char* block_name() const { return is_sd_ ? "sdmmc-sd" : "sdmmc-mmc"; }
  SdmmcRootDevice* parent() { return parent_; }
  void OnRequests(PartitionDevice& partition, cpp20::span<block_server::Request> requests);
  bool SupportsInlineEncryption() const { return inline_encryption_client_.is_valid(); }

  // Visible for testing.
  void SetBlockInfo(uint32_t block_size, uint64_t block_count);
  void SetMetadata(const fuchsia_hardware_sdmmc::SdmmcMetadata& metadata) { metadata_ = metadata; }
  const inspect::Inspector& inspect() const;
  const std::vector<std::unique_ptr<PartitionDevice>>& child_partition_devices() const {
    return child_partition_devices_;
  }
  const std::unique_ptr<RpmbDevice>& child_rpmb_device() const { return child_rpmb_device_; }

  fdf::Logger& logger() const;

  // fuchsia.hardware.inlineencryption.Device
  void ProgramKey(ProgramKeyRequestView request, ProgramKeyCompleter::Sync& completer) override;
  void DeriveRawSecret(DeriveRawSecretRequestView request,
                       DeriveRawSecretCompleter::Sync& completer) override;

 private:
  static constexpr fdf_arena_tag_t kArenaTag = 'SBLK';
  // An arbitrary limit to prevent RPMB clients from flooding us with requests.
  static constexpr size_t kMaxOutstandingRpmbRequests = 16;

  // The worker thread will handle this many block ops then this many RPMB requests, and will repeat
  // until both queues are empty.
  static constexpr size_t kRoundRobinRequestCount = 16;

  static constexpr uint16_t kRpmbRequestProgramKey = 1;
  static constexpr uint16_t kRpmbRequestReadWriteCounter = 2;
  static constexpr uint16_t kRpmbRequestWriteData = 3;
  static constexpr uint16_t kRpmbRequestReadData = 4;
  static constexpr uint16_t kRpmbRequestReadResult = 5;
  static constexpr uint16_t kRpmbRequestWriteConfiguration = 6;
  static constexpr uint16_t kRpmbRequestReadConfiguration = 7;

  static constexpr uint32_t kPackedCommandVmoId = 1;

  zx_status_t ProbeSdLocked() TA_REQ(worker_lock_);
  zx_status_t ProbeMmcLocked() TA_REQ(worker_lock_);
  zx_status_t MmcConfigureBus() TA_REQ(worker_lock_);
  zx_status_t MmcTryHs() TA_REQ(worker_lock_);

  zx_status_t AddCqhciDevice() TA_REQ(worker_lock_, queue_lock_);

  zx_status_t ReadWriteWithRetries(std::vector<block_server::Request>& requests,
                                   EmmcPartition partition) TA_REQ(worker_lock_);
  zx_status_t ReadWriteAttempt(std::vector<block_server::Request>& requests,
                               bool suppress_error_messages) TA_REQ(worker_lock_);
  zx_status_t Flush() TA_REQ(worker_lock_);
  zx_status_t Barrier() TA_REQ(worker_lock_);
  zx_status_t Trim(uint64_t offset_dev, uint64_t length, const EmmcPartition partition)
      TA_REQ(worker_lock_);
  zx_status_t SetPartition(const EmmcPartition partition) TA_REQ(worker_lock_);
  zx::result<uint16_t> GetRpmbRequestType(const RpmbRequestInfo& request) const;
  zx_status_t RpmbRequest(const RpmbRequestInfo& request) TA_REQ(worker_lock_);

  void HandleRpmbRequests(std::deque<RpmbRequestInfo>& rpmb_list) TA_REQ(worker_lock_);

  void WorkerLoop();

  zx_status_t WaitForIdle();
  zx_status_t WaitForTran();
  zx_status_t WaitForState(uint32_t state);

  zx_status_t MmcDoSwitch(uint8_t index, uint8_t value) TA_REQ(worker_lock_);
  zx_status_t MmcWaitForSwitch(uint8_t index, uint8_t value) TA_REQ(worker_lock_);
  zx_status_t MmcSetBusWidth(sdmmc_bus_width_t bus_width, uint8_t mmc_ext_csd_bus_width)
      TA_REQ(worker_lock_);
  sdmmc_bus_width_t MmcSelectBusWidth() TA_REQ(worker_lock_);
  // The host is expected to switch the timing from HS200 to HS as part of HS400 initialization.
  // Checking the status of the switch requires special handling to avoid a temporary mismatch
  // between the host and device timings.
  zx_status_t MmcSwitchTiming(sdmmc_timing_t new_timing) TA_REQ(worker_lock_);
  zx_status_t MmcSwitchTimingHs200ToHs() TA_REQ(worker_lock_);
  zx_status_t MmcSwitchFreq(uint32_t new_freq);
  zx_status_t MmcDecodeExtCsd() TA_REQ(worker_lock_);
  bool MmcSupportsHs() TA_REQ(worker_lock_);
  bool MmcSupportsHsDdr() TA_REQ(worker_lock_);
  bool MmcSupportsHs200() TA_REQ(worker_lock_);
  bool MmcSupportsHs400() TA_REQ(worker_lock_);
  bool MmcSupportsHs400EnhancedStrobe() TA_REQ(worker_lock_);
  void MmcSetInspectProperties() TA_REQ(worker_lock_) TA_REQ(queue_lock_);

  // TODO(b/309152899): Once fuchsia.power.SuspendEnabled config cap is available, have this method
  // return failure if power management could not be configured. Use fuchsia.power.SuspendEnabled to
  // ignore this failure when expected.
  // Register power configs with Power Broker, and begin the continuous power level adjustment of
  // hardware. For products that don't support the Power Framework, this method simply returns
  // success.
  zx::result<> ConfigurePowerManagement();

  // Acquires a lease on a power element via the supplied |lessor_client|, returning the resulting
  // lease control client end.
  // This method is planned for use in a future change.
  zx::result<fidl::ClientEnd<fuchsia_power_broker::LeaseControl>> AcquireInitLease(
      const fidl::WireSyncClient<fuchsia_power_broker::Lessor>& lessor_client);

  // Watches the required hardware power level and adjusts it accordingly. Also serves requests that
  // were delayed because they were received during suspended state. Communicates power level
  // transitions to the Power Broker.
  void WatchHardwareRequiredLevel();

  SdmmcRootDevice* const parent_;
  // Only accessed by ProbeSd, ProbeMmc, SuspendPower, ResumePower, and WorkerLoop.
  std::unique_ptr<SdmmcDevice> sdmmc_;

  sdmmc_bus_width_t bus_width_;
  sdmmc_timing_t timing_;

  uint32_t clock_rate_;  // Bus clock rate

  // mmc
  std::array<uint8_t, SDMMC_CID_SIZE> raw_cid_;
  std::array<uint8_t, MMC_EXT_CSD_SIZE> raw_ext_csd_ TA_GUARDED(worker_lock_);

  // `worker_lock_` must be held before interacting with the device.
  fbl::Mutex worker_lock_ TA_ACQ_BEFORE(queue_lock_);
  fbl::ConditionVariable worker_condition_;
  fbl::Mutex queue_lock_ TA_ACQ_AFTER(worker_lock_);

  // Signals the worker loop to process incoming commands (or driver shutdown).
  libsync::Completion worker_event_;

  // blockio requests
  std::deque<RpmbRequestInfo> rpmb_list_ TA_GUARDED(queue_lock_);

  // Dispatcher for processing queued block requests.
  fdf::Dispatcher worker_dispatcher_;
  // Signaled when worker_dispatcher_ is shut down.
  libsync::Completion worker_shutdown_completion_;

  bool power_suspended_ TA_GUARDED(worker_lock_) = false;
  bool shutdown_ TA_GUARDED(worker_lock_) = false;
  trace_async_id_t trace_async_id_;

  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> cqhci_controller_;
  fdf::ServerBindingGroup<fuchsia_hardware_cqhci::Cqhci> cqhci_bindings_;
  // Only used when CQHCI is enabled.
  std::unique_ptr<DriverRpmbDevice> driver_rpmb_device_;
  // NB: This must not outlive `driver_rpmb_device_`.
  fdf::ServerBindingGroup<fuchsia_hardware_rpmb::DriverRpmb> driver_rpmb_bindings_;

  fuchsia_storage_block::wire::BlockInfo block_info_{};
  std::optional<fuchsia_hardware_cqhci::CqhciHostInfo> cqhci_host_info_;

  bool is_sd_ = false;
  bool cache_enabled_ = false;
  bool cache_flush_fifo_ = false;

  uint32_t max_packed_reads_effective_ = 0;   // Use command packing up to this many reads.
  uint32_t max_packed_writes_effective_ = 0;  // Use command packing up to this many writes.
  ReadWriteMetadata readwrite_metadata_ TA_GUARDED(worker_lock_);

  inspect::Node root_;
  struct InspectProperties {
    inspect::UintProperty io_errors_;                    // Only updated from the worker thread.
    inspect::UintProperty io_retries_;                   // Only updated from the worker thread.
    inspect::UintProperty serial_number_;                // Set once by the init thread.
    inspect::UintProperty clock_rate_;                   // Set once by the init thread.
    inspect::UintProperty bus_width_bits_;               // Set once by the init thread.
    inspect::StringProperty timing_;                     // Set once by the init thread.
    inspect::UintProperty type_a_lifetime_used_;         // Set once by the init thread.
    inspect::UintProperty type_b_lifetime_used_;         // Set once by the init thread.
    inspect::UintProperty max_lifetime_used_;            // Set once by the init thread.
    inspect::UintProperty cache_size_bits_;              // Set once by the init thread.
    inspect::BoolProperty cache_enabled_;                // Set once by the init thread.
    inspect::BoolProperty cache_flush_fifo_;             // Set once by the init thread.
    inspect::BoolProperty barrier_supported_;            // Set once by the init thread.
    inspect::BoolProperty trim_enabled_;                 // Set once by the init thread.
    inspect::UintProperty max_packed_reads_;             // Set once by the init thread.
    inspect::UintProperty max_packed_writes_;            // Set once by the init thread.
    inspect::UintProperty max_packed_reads_effective_;   // Set once by the init thread.
    inspect::UintProperty max_packed_writes_effective_;  // Set once by the init thread.
    inspect::BoolProperty using_fidl_;                   // Set once by the init thread.
    inspect::BoolProperty power_suspended_;              // Updated whenever power state changes.
  } properties_;

  fidl::WireSyncClient<fuchsia_driver_framework::Node> block_node_;
  fidl::WireSyncClient<fuchsia_driver_framework::NodeController> controller_;

  // Only used when CQHCI is disabled.
  std::vector<std::unique_ptr<PartitionDevice>> child_partition_devices_;
  // Only used when CQHCI is disabled.
  std::unique_ptr<RpmbDevice> child_rpmb_device_;
  EmmcPartition current_partition_ TA_GUARDED(worker_lock_) = EmmcPartition::USER_DATA_PARTITION;

  // This value comes from metadata. If true, the device must be re-initialized after leaving the
  // OFF state, and we cannot use the MMC sleep state.
  bool vccq_off_with_controller_off_ = false;

  fuchsia_hardware_sdmmc::SdmmcMetadata metadata_;
  fdf::WireSyncClient<fuchsia_hardware_inlineencryption::DriverDevice> inline_encryption_client_;
  bool suspend_supported_ = false;
};

}  // namespace sdmmc

#endif  // SRC_DEVICES_BLOCK_DRIVERS_SDMMC_SDMMC_BLOCK_DEVICE_H_
