// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TRANSFER_REQUEST_PROCESSOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TRANSFER_REQUEST_PROCESSOR_H_

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/function.h>
#include <lib/trace/event.h>

#include "request_processor.h"
#include "src/devices/block/drivers/ufs/registers.h"
#include "src/devices/block/drivers/ufs/transfer_request_descriptor.h"
#include "src/devices/block/drivers/ufs/upiu/scsi_commands.h"

namespace ufs {

constexpr uint8_t kMaxTransferRequestListSize = kMaxRequestListSize;
// Currently, the UFS driver has two threads for submitting commands. One is the ufs driver thread
// that submits the admin command when the driver is initialized, and the other is the I/O thread
// that submits the requested I/O command from the block server.
// These two threads should hold a lock to access the shared resource RequestList slot. However, if
// we separate the Admin slot and the I/O slot, we do not need to lock the slot. Therefore, slots 0
// ~ 30 are used by the I/O thread, and slot 31 is used for the Admin command.
constexpr uint8_t kAdminCommandSlotCount = 1;
constexpr uint8_t kAdminCommandSlotNumber = kMaxTransferRequestListSize - kAdminCommandSlotCount;

// Maximum tight spin-wait retries when waiting for descriptor OCS update after doorbell clearance.
// Descriptors are allocated in uncached memory; this spin-wait loop handles interconnect
// write-posting latency on physical hardware where MMIO doorbell clearance can be observed before
// DMA writes updating OCS reach DRAM.
constexpr uint32_t kOverallCompletionRetries = 100;

// Owns and processes the UTP transfer request list.
class TransferRequestProcessor : public RequestProcessor {
 public:
  static zx::result<std::unique_ptr<TransferRequestProcessor>> Create(Ufs &ufs, zx::unowned_bti bti,
                                                                      const fdf::MmioView mmio,
                                                                      uint8_t entry_count) {
    if (entry_count > kMaxTransferRequestListSize) {
      fdf::error("Request list size exceeded the maximum size of {}.", kMaxTransferRequestListSize);
      return zx::error(ZX_ERR_INVALID_ARGS);
    }

    return RequestProcessor::Create<TransferRequestProcessor, TransferRequestDescriptor>(
        ufs, std::move(bti), mmio, entry_count);
  }
  explicit TransferRequestProcessor(RequestList request_list, Ufs &ufs, zx::unowned_bti bti,
                                    const fdf::MmioView mmio, uint32_t slot_count)
      : RequestProcessor(std::move(request_list), ufs, std::move(bti), mmio, slot_count) {}
  ~TransferRequestProcessor() override = default;

  zx::result<> Init() override;
  using RequestProcessor::ReserveSlot;
  // Allocate a slot to submit an Admin command. Use slot 31 to avoid conflicts with I/O commands.
  zx::result<uint8_t> ReserveAdminSlot() TA_REQ(admin_slot_lock_) TA_EXCL(slot_lock_);

  uint32_t ProcessCompletionOfAdminRequests();
  uint32_t ProcessCompletionOfIoRequests() override;

  bool CheckAdminCommandCompleted(uint32_t doorbell) TA_EXCL(slot_lock_) {
    std::lock_guard<std::mutex> lock(slot_lock_);
    return slots_.GetSlot(kAdminCommandSlotNumber).state == SlotState::kScheduled &&
           !(doorbell & (1u << kAdminCommandSlotNumber));
  }

  // Find the earliest timeout deadline of the in-flight I/O.
  zx_time_t GetEarliestTimeoutDeadline();

  // |SendAdminScsiCmd| allocates the admin slot for a SCSI command and calls SendRequestUsingSlot.
  // Blocks until completion and returns the result.
  zx::result<std::unique_ptr<ResponseUpiu>> SendAdminScsiCmd(
      ScsiCommandUpiu &request, uint8_t lun, zx::unowned_vmo data_vmo = zx::unowned_vmo());

  // |SendIoScsiCmd| sends an asynchronous SCSI command using a previously reserved I/O slot.
  // The |completion_cb| is guaranteed to be invoked, and the |slot| will be automatically
  // released, regardless of whether the request is successfully submitted or fails early.
  void SendIoScsiCmd(ScsiCommandUpiu &request, uint8_t lun, uint8_t slot, zx::unowned_vmo data_vmo,
                     uint64_t dma_offset, uint64_t dma_length,
                     fit::callback<void(zx_status_t)> completion_cb);

  // This function is a wrapper function that sends a query request UPIU.
  zx::result<std::unique_ptr<QueryResponseUpiu>> SendQueryRequestUpiu(QueryRequestUpiu &request);

  // |SendRequestUpiu| allocates a slot for request UPIU and calls SendRequestUsingSlot.
  // This function is only ever used for admin commands.
  template <class RequestType, class ResponseType>
  zx::result<std::unique_ptr<ResponseType>> SendRequestUpiu(
      RequestType &request, uint8_t lun = 0, zx::unowned_vmo data_vmo = zx::unowned_vmo(),
      uint64_t dma_offset = 0, uint64_t dma_length = 0);

  template <class RequestType>
  std::tuple<uint16_t, uint32_t> PreparePrdt(RequestType &request, uint8_t lun, uint8_t slot,
                                             const std::vector<zx_paddr_t> &buffer_phys,
                                             uint16_t response_offset, uint16_t response_length)
      TA_REQ(slot_lock_) {
    return {0, 0};
  }

  template <>
  std::tuple<uint16_t, uint32_t> PreparePrdt<ScsiCommandUpiu>(
      ScsiCommandUpiu &request, uint8_t lun, uint8_t slot,
      const std::vector<zx_paddr_t> &buffer_phys, uint16_t response_offset,
      uint16_t response_length) TA_REQ(slot_lock_);

  // Low-level helper that sets up descriptor and PRDT buffers for a reserved slot, pins data VMO,
  // and rings the doorbell. Invokes |completion_cb| when the hardware request completes, or
  // immediately with an error status if slot preparation fails.
  template <class RequestType>
  void SendRequestUsingSlot(RequestType &request, uint8_t lun, uint8_t slot,
                            zx::unowned_vmo data_vmo, uint64_t dma_offset, uint64_t dma_length,
                            fit::callback<void(zx_status_t)> completion_cb);

  uint32_t GetInflightIoCount() const {
    std::lock_guard<std::mutex> lock(slot_lock_);
    constexpr uint32_t kIoSlotsMask = (1u << kAdminCommandSlotNumber) - 1;
    return static_cast<uint32_t>(std::popcount(allocated_slots_mask_ & kIoSlotsMask));
  }

 private:
  friend class UfsTest;

  zx::result<> FillDescriptorAndSendRequest(uint8_t slot, DataDirection data_dir,
                                            uint16_t response_offset, uint16_t response_length,
                                            uint16_t prdt_offset, uint32_t prdt_entry_count,
                                            bool reliable_write = false) TA_REQ(slot_lock_);

  zx::result<> CheckResponse(uint8_t slot_num, AbstractResponseUpiu &response) TA_REQ(slot_lock_);
  // Check for errors in the following order: OCS -> header_response -> scsi_status
  scsi::StatusMessage CheckScsiAndGetStatusMessage(uint8_t slot_num, AbstractResponseUpiu &response)
      TA_REQ(slot_lock_);
  scsi::HostStatusCode GetScsiCommandHostStatus(OverallCommandStatus ocs,
                                                UpiuHeaderResponseCode header_response,
                                                scsi::StatusCode response_status);
  scsi::HostStatusCode ScsiStatusToHostStatus(scsi::StatusCode command_status);

  void RequestCompletion(uint8_t slot_num, RequestSlot &request_slot, bool is_timeout,
                         fit::callback<void(zx_status_t)> &cb, zx_status_t &status)
      TA_REQ(slot_lock_);
  zx_status_t UpiuCompletion(uint8_t slot_num, RequestSlot &request_slot,
                             zx_status_t completion_status = ZX_OK) TA_REQ(slot_lock_);

  std::optional<uint8_t> GetAdminCommandSlotNumber() const override {
    return kAdminCommandSlotNumber;
  }

  void SetDoorBellRegister(uint8_t slot_num) override {
    UtrListDoorBellReg::Get().FromValue(1u << slot_num).WriteTo(&register_);
  }
  uint32_t ReadDoorBellRegister() override {
    return UtrListDoorBellReg::Get().ReadFrom(&register_).door_bell();
  }
  // Checks if the request at |slot_num| has completed (either via hardware completion or timeout).
  // If the request completed, returns true, sets |cb| to the completion callback that was stored in
  // the request slot, and sets |status| to the completion result status.
  // The caller must invoke |cb(status)| after releasing |slot_lock_| to avoid lock re-entrancy and
  // inversion deadlocks.
  bool ProcessSlotCompletion(uint8_t slot_num, uint32_t doorbell,
                             fit::callback<void(zx_status_t)> &cb, zx_status_t &status)
      TA_REQ(slot_lock_);

  // TODO(b/42075643): Background Operation uses the admin slot, causing a race condition for admin
  // commands running on the main thread. To fix this, per-slot locking is required, but I added
  // admin_slot_lock_ as a temporary solution.
  std::mutex admin_slot_lock_;
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TRANSFER_REQUEST_PROCESSOR_H_
