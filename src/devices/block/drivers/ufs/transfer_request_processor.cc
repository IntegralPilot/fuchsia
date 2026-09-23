// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "transfer_request_processor.h"

#include <lib/driver/logging/cpp/logger.h>
#include <lib/fit/defer.h>
#include <lib/trace/event.h>

#include <optional>

#include <safemath/checked_math.h>
#include <safemath/safe_conversions.h>

#include "src/devices/block/drivers/ufs/ufs.h"
#include "src/devices/block/drivers/ufs/upiu/upiu_transactions.h"

namespace ufs {

namespace {
void FillPrdt(PhysicalRegionDescriptionTableEntry *prdt,
              const std::vector<zx_paddr_t> &buffer_physical_addresses, uint32_t prdt_count,
              uint32_t data_length) {
  for (uint32_t i = 0; i < prdt_count; ++i) {
    // It only supports 4KB data buffers for each entry in the scatter-gather.
    ZX_ASSERT(buffer_physical_addresses[i] != 0);
    uint32_t byte_count = data_length < kPrdtEntryDataLength ? data_length : kPrdtEntryDataLength;
    prdt->set_data_base_address(static_cast<uint32_t>(buffer_physical_addresses[i] & 0xffffffff));
    prdt->set_data_base_address_upper(static_cast<uint32_t>(buffer_physical_addresses[i] >> 32));
    prdt->set_data_byte_count(byte_count - 1);

    ++prdt;
    data_length -= byte_count;
  }
  ZX_DEBUG_ASSERT(data_length == 0);
}
}  // namespace

template <>
std::tuple<uint16_t, uint32_t> TransferRequestProcessor::PreparePrdt<ScsiCommandUpiu>(
    ScsiCommandUpiu &request, const uint8_t lun, const uint8_t slot,
    const std::vector<zx_paddr_t> &buffer_phys, const uint16_t response_offset,
    const uint16_t response_length) {
  const uint32_t data_transfer_length = std::min(request.GetTransferBytes(), kMaxPrdtDataLength);

  request.GetHeader().lun = lun;
  request.SetExpectedDataTransferLength(data_transfer_length);

  // Prepare PRDT(physical region description table).
  const uint32_t prdt_entry_count =
      fbl::round_up(data_transfer_length, kPrdtEntryDataLength) / kPrdtEntryDataLength;
  ZX_DEBUG_ASSERT(prdt_entry_count <= kMaxPrdtEntryCount);

  uint16_t prdt_offset = response_offset + response_length;
  uint32_t prdt_length_in_bytes = prdt_entry_count * sizeof(PhysicalRegionDescriptionTableEntry);
  const size_t total_length = static_cast<size_t>(prdt_offset) + prdt_length_in_bytes;

  ZX_DEBUG_ASSERT_MSG(total_length <= slots_.GetDescriptorBufferSize(slot),
                      "Invalid UPIU size for prdt");
  auto prdt = slots_.GetDescriptorBuffer<PhysicalRegionDescriptionTableEntry>(slot, prdt_offset);
  CustomMemSet(prdt, 0, prdt_length_in_bytes);

  FillPrdt(prdt, buffer_phys, prdt_entry_count, data_transfer_length);

  // TODO(https://fxbug.dev/42075643): Enable unmmap and write buffer command. Umap and writebuffer
  // must set the xfer->count value differently.

  return {prdt_offset, prdt_entry_count};
}

zx::result<> TransferRequestProcessor::Init() {
  std::lock_guard<std::mutex> lock(slot_lock_);
  zx_paddr_t paddr = slots_.GetRequestDescriptorPhysicalAddress<TransferRequestDescriptor>(0);
  UtrListBaseAddressReg::Get().FromValue(paddr & 0xffffffff).WriteTo(&register_);
  UtrListBaseAddressUpperReg::Get().FromValue(paddr >> 32).WriteTo(&register_);

  if (!HostControllerStatusReg::Get().ReadFrom(&register_).utp_transfer_request_list_ready()) {
    fdf::error("UTP transfer request list is not ready\n");
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (UtrListDoorBellReg::Get().ReadFrom(&register_).door_bell() != 0) {
    fdf::error("UTP transfer request list door bell is not ready\n");
    return zx::error(ZX_ERR_INTERNAL);
  }

  if (UtrListCompletionNotificationReg::Get().ReadFrom(&register_).notification() != 0) {
    fdf::error("UTP transfer request list notification is not ready\n");
    return zx::error(ZX_ERR_INTERNAL);
  }

  // Start Utp Transfer Request list.
  UtrListRunStopReg::Get().FromValue(0).set_value(true).WriteTo(&register_);

  return zx::ok();
}

zx::result<uint8_t> TransferRequestProcessor::ReserveAdminSlot() {
  std::lock_guard<std::mutex> lock(slot_lock_);
  ReclaimTimedOutSlotsLocked();
  if ((allocated_slots_mask_ & (1u << kAdminCommandSlotNumber)) != 0) {
    fdf::debug("Failed to reserve an admin request slot");
    return zx::error(ZX_ERR_NO_RESOURCES);
  }
  allocated_slots_mask_ |= (1u << kAdminCommandSlotNumber);
  slots_.GetSlot(kAdminCommandSlotNumber).Reset(SlotState::kReserved);
  return zx::ok(kAdminCommandSlotNumber);
}

zx::result<std::unique_ptr<ResponseUpiu>> TransferRequestProcessor::SendAdminScsiCmd(
    ScsiCommandUpiu &request, uint8_t lun, zx::unowned_vmo data_vmo) {
  if (request.GetTransferBytes() > 0 && !data_vmo->is_valid()) {
    return zx::error(ZX_ERR_BAD_HANDLE);
  }

  uint64_t dma_length = data_vmo->is_valid()
                            ? fbl::round_up(request.GetTransferBytes(), zx_system_get_page_size())
                            : 0;
  return SendRequestUpiu<ScsiCommandUpiu, ResponseUpiu>(request, lun, std::move(data_vmo), 0,
                                                        dma_length);
}

void TransferRequestProcessor::SendIoScsiCmd(ScsiCommandUpiu &request, uint8_t lun, uint8_t slot,
                                             zx::unowned_vmo data_vmo, uint64_t dma_offset,
                                             uint64_t dma_length,
                                             fit::callback<void(zx_status_t)> completion_cb) {
  SendRequestUsingSlot<ScsiCommandUpiu>(request, lun, slot, std::move(data_vmo), dma_offset,
                                        dma_length, std::move(completion_cb));
}

template <class RequestType, class ResponseType>
zx::result<std::unique_ptr<ResponseType>> TransferRequestProcessor::SendRequestUpiu(
    RequestType &request, uint8_t lun, zx::unowned_vmo data_vmo, uint64_t dma_offset,
    uint64_t dma_length) {
  // TODO(https://fxbug.dev/42075643): Needs to be changed to be compatible with DFv2's dispatcher
  // Since completions are handled by the worker dispatchers, submitting a synchronous command from
  // either the I/O or admin worker thread will cause a deadlock.
  fdf_dispatcher_t *current = fdf::Dispatcher::GetCurrent()->get();
  fdf_dispatcher_t *io_disp = controller_.io_worker_dispatcher()->get();
  fdf_dispatcher_t *admin_disp = controller_.admin_worker_dispatcher()->get();
  if (current && ((io_disp && current == io_disp) || (admin_disp && current == admin_disp))) {
    ZX_PANIC(
        "Synchronous UFS request cannot be issued from inside IO or Admin worker dispatcher thread!");
  }

  std::lock_guard<std::mutex> lock(admin_slot_lock_);
  zx::result<uint8_t> slot = ReserveAdminSlot();
  if (slot.is_error()) {
    return zx::error(ZX_ERR_NO_RESOURCES);
  }

  const uint8_t slot_num = slot.value();
  struct SyncWaitContext {
    sync_completion_t complete;
    zx_status_t completion_status = ZX_OK;
  };
  auto wait_context = std::make_shared<SyncWaitContext>();
  SendRequestUsingSlot<RequestType>(request, lun, slot_num, std::move(data_vmo), dma_offset,
                                    dma_length, [wait_context](zx_status_t status) {
                                      wait_context->completion_status = status;
                                      sync_completion_signal(&wait_context->complete);
                                    });

  zx_time_t deadline = zx_deadline_after(GetTimeout().get());
  zx_status_t status = sync_completion_wait_deadline(&wait_context->complete, deadline);
  if (status != ZX_OK) {
    std::lock_guard<std::mutex> lock(slot_lock_);
    RequestSlot &request_slot = slots_.GetSlot(slot_num);
    request_slot.completion_cb = nullptr;
    uint32_t doorbell = UtrListDoorBellReg::Get().ReadFrom(&register_).door_bell();
    if (doorbell & (1u << slot_num)) {
      SetSlotStateLocked(slot_num, SlotState::kTimeout);
    } else {
      if (zx::result<> result = ClearSlotLocked(request_slot); result.is_error()) {
        return result.take_error();
      }
    }
    return zx::error(status);
  }
  if (wait_context->completion_status != ZX_OK) {
    return zx::error(wait_context->completion_status);
  }

  const uint16_t response_offset = request.GetResponseOffset();
  void *response_buf = nullptr;
  {
    std::lock_guard<std::mutex> lock(slot_lock_);
    response_buf = slots_.GetDescriptorBuffer(slot_num, response_offset);
  }
  return zx::ok(std::make_unique<ResponseType>(response_buf));
}

zx::result<std::unique_ptr<QueryResponseUpiu>> TransferRequestProcessor::SendQueryRequestUpiu(
    QueryRequestUpiu &request) {
  auto response = SendRequestUpiu<QueryRequestUpiu, QueryResponseUpiu>(request);
  if (response.is_error()) {
    QueryOpcode query_opcode =
        static_cast<QueryOpcode>(request.GetData<QueryRequestUpiuData>()->opcode);
    uint8_t type = request.GetData<QueryRequestUpiuData>()->idn;
    fdf::debug("Failed {}(type:0x{:x}) query request UPIU: {}", QueryOpcodeToString(query_opcode),
               type, response.status_string());
  }
  return response;
}

template <class RequestType>
void TransferRequestProcessor::SendRequestUsingSlot(
    RequestType &request, uint8_t lun, uint8_t slot, zx::unowned_vmo data_vmo, uint64_t dma_offset,
    uint64_t dma_length, fit::callback<void(zx_status_t)> completion_cb) {
  zx_status_t status = ZX_OK;
  {
    std::lock_guard<std::mutex> lock(slot_lock_);
    RequestSlot &request_slot = slots_.GetSlot(slot);
    ZX_DEBUG_ASSERT_MSG(request_slot.state == SlotState::kReserved, "Invalid slot state");

    const uint16_t response_offset = request.GetResponseOffset();
    const uint16_t response_length = request.GetResponseLength();

    request_slot.completion_cb = std::move(completion_cb);
    request_slot.data_vmo = data_vmo;
    request_slot.dma_offset = dma_offset;
    request_slot.dma_length = dma_length;
    request_slot.is_read = (request.GetDataDirection() == DataDirection::kDeviceToHost);
    request_slot.is_scsi_command = std::is_base_of<ScsiCommandUpiu, RequestType>::value;
    request_slot.response_upiu_offset = response_offset;

    std::vector<zx_paddr_t> data_paddrs;

    if (dma_length > 0) {
      if (!request_slot.data_vmo->is_valid()) {
        fdf::error("Invalid data VMO for transfer of length {}", dma_length);
        status = ZX_ERR_BAD_HANDLE;
      } else {
        // Assign physical addresses(pin) to data vmo. The return value is the physical address of
        // the pinned memory.
        const uint32_t kPageSize = zx_system_get_page_size();
        uint32_t option = request_slot.is_read ? ZX_BTI_PERM_WRITE : ZX_BTI_PERM_READ;

        ZX_DEBUG_ASSERT(dma_length % kPageSize == 0);

        data_paddrs.resize(dma_length / kPageSize, 0);
        if (zx_status_t pin_status =
                GetBti()->pin(option, *request_slot.data_vmo, dma_offset, dma_length,
                              data_paddrs.data(), dma_length / kPageSize, &request_slot.pmt);
            pin_status != ZX_OK) {
          fdf::error("Failed to pin IO buffer: {}", zx_status_get_string(pin_status));
          status = pin_status;
        } else {
          // Ensure that any cached writes are written out to RAM before we issue the request.
          // For writes, CLEAN is sufficient and cheaper. For reads, CLEAN_INVALIDATE ensures
          // pending writes are flushed and cache lines are cleared before DMA.
          uint32_t op =
              request_slot.is_read ? ZX_VMO_OP_CACHE_CLEAN_INVALIDATE : ZX_VMO_OP_CACHE_CLEAN;
          if (zx_status_t cache_status =
                  request_slot.data_vmo->op_range(op, dma_offset, dma_length, nullptr, 0);
              cache_status != ZX_OK) {
            fdf::error("Failed to flush/invalidate cache for data VMO: {}",
                       zx_status_get_string(cache_status));
            status = cache_status;
          }
        }
      }
    }

    if (status == ZX_OK) {
      uint16_t prdt_offset = 0;
      uint32_t prdt_entry_count = 0;
      std::tie(prdt_offset, prdt_entry_count) = PreparePrdt<RequestType>(
          request, lun, slot, data_paddrs, response_offset, response_length);

      // Record the slot number to |task_tag| for debugging.
      request.GetHeader().task_tag = slot;

      // Copy request and prepare response.
      const size_t length = static_cast<size_t>(response_offset) + response_length;
      ZX_DEBUG_ASSERT_MSG(length <= slots_.GetDescriptorBufferSize(slot), "Invalid UPIU size");

      CustomMemCpy(slots_.GetDescriptorBuffer(slot), request.GetData(), response_offset);
      CustomMemSet(slots_.GetDescriptorBuffer<uint8_t>(slot) + response_offset, 0, response_length);

      const bool reliable_write = (lun == static_cast<uint8_t>(WellKnownLuns::kRpmb) &&
                                   request.GetDataDirection() == DataDirection::kHostToDevice);

      if (zx::result<> result = FillDescriptorAndSendRequest(
              slot, request.GetDataDirection(), response_offset, response_length, prdt_offset,
              prdt_entry_count, reliable_write);
          result.is_error()) {
        fdf::error("Failed to send upiu: {}", result);
        status = result.status_value();
      }
    }

    if (status != ZX_OK) {
      completion_cb = std::move(request_slot.completion_cb);
      if (zx::result<> clear_res = ClearSlotLocked(request_slot); clear_res.is_error()) {
        fdf::error("Failed to clear slot[{}]: {}", slot, clear_res);
      }
    }
  }

  // |completion_cb| is consumed once we've submitted the request; this is just used for the
  // failure path to ensure the callback is invoked.
  if (completion_cb) {
    ZX_DEBUG_ASSERT(status != ZX_OK);
    completion_cb(status);
  }
}

template void TransferRequestProcessor::SendRequestUsingSlot<QueryRequestUpiu>(
    QueryRequestUpiu &request, uint8_t lun, uint8_t slot, zx::unowned_vmo data_vmo,
    uint64_t dma_offset, uint64_t dma_length, fit::callback<void(zx_status_t)> completion_cb);
template void TransferRequestProcessor::SendRequestUsingSlot<ScsiCommandUpiu>(
    ScsiCommandUpiu &request, uint8_t lun, uint8_t slot, zx::unowned_vmo data_vmo,
    uint64_t dma_offset, uint64_t dma_length, fit::callback<void(zx_status_t)> completion_cb);
template void TransferRequestProcessor::SendRequestUsingSlot<NopOutUpiu>(
    NopOutUpiu &request, uint8_t lun, uint8_t slot, zx::unowned_vmo data_vmo, uint64_t dma_offset,
    uint64_t dma_length, fit::callback<void(zx_status_t)> completion_cb);

template zx::result<std::unique_ptr<QueryResponseUpiu>>
TransferRequestProcessor::SendRequestUpiu<QueryRequestUpiu, QueryResponseUpiu>(
    QueryRequestUpiu &request, uint8_t lun, zx::unowned_vmo data_vmo, uint64_t dma_offset,
    uint64_t dma_length);
template zx::result<std::unique_ptr<NopInUpiu>>
TransferRequestProcessor::SendRequestUpiu<NopOutUpiu, NopInUpiu>(NopOutUpiu &request, uint8_t lun,
                                                                 zx::unowned_vmo data_vmo,
                                                                 uint64_t dma_offset,
                                                                 uint64_t dma_length);
template zx::result<std::unique_ptr<ResponseUpiu>>
TransferRequestProcessor::SendRequestUpiu<ScsiCommandUpiu, ResponseUpiu>(ScsiCommandUpiu &request,
                                                                         uint8_t lun,
                                                                         zx::unowned_vmo data_vmo,
                                                                         uint64_t dma_offset,
                                                                         uint64_t dma_length);

zx_status_t TransferRequestProcessor::UpiuCompletion(uint8_t slot_num, RequestSlot &request_slot,
                                                     zx_status_t completion_status) {
  TRACE_DURATION("ufs", "UpiuCompletion", "slot", slot_num);

  scsi::StatusMessage status_message;
  std::optional<std::reference_wrapper<scsi::FixedFormatSenseDataHeader>> sense_data = std::nullopt;

  ResponseUpiu response(
      slots_.GetDescriptorBuffer<ResponseUpiu>(slot_num, request_slot.response_upiu_offset));

  zx::result<> request_result = zx::ok();
  if (completion_status == ZX_ERR_TIMED_OUT) {
    status_message.host_status_code = scsi::HostStatusCode::kTimeout;
    status_message.scsi_status_code = scsi::StatusCode::GOOD;
    request_result = zx::error(ZX_ERR_TIMED_OUT);
  } else if (completion_status != ZX_OK) {
    request_result = zx::error(completion_status);
  } else {
    request_result = CheckResponse(slot_num, response);

    if (request_slot.is_scsi_command) {
      status_message = CheckScsiAndGetStatusMessage(slot_num, response);
      uint16_t sense_data_len = betoh16(response.GetData<ResponseUpiuData>()->sense_data_len);
      // Per the UFS specification (JESD220, Table 10.19 / Section 11.3.17), UFS-compliant devices
      // return a fixed format data record of exactly 18 bytes when sense data is present (with
      // additional_sense_length set to 10 / 0x0A), or 0 when no sense data is returned.
      if (sense_data_len == sizeof(scsi::FixedFormatSenseDataHeader)) {
        auto *header =
            reinterpret_cast<scsi::FixedFormatSenseDataHeader *>(response.GetSenseData());
        constexpr uint8_t kUfsFixedSenseDataAdditionalLength = 10;
        if (header->additional_sense_length == kUfsFixedSenseDataAdditionalLength) {
          sense_data = *header;
        } else {
          fdf::warn("UFS response returned invalid additional_sense_length: {} (expected 10)",
                    header->additional_sense_length);
          request_result = zx::error(ZX_ERR_BAD_STATE);
        }
      } else if (sense_data_len != 0) {
        fdf::warn("UFS response returned invalid sense data length: {} (expected 18 or 0)",
                  sense_data_len);
        request_result = zx::error(ZX_ERR_BAD_STATE);
      }
    }
  }

  ZX_DEBUG_ASSERT_MSG(request_slot.completion_cb, "Slot has no completion callback");
  if (slot_num != kAdminCommandSlotNumber && request_slot.is_scsi_command &&
      request_result.is_ok()) {
    // Until native UFS IO commands are defined by the UFS specification, we assume that only SCSI
    // commands can be IO commands.
    request_result = controller_.ScsiComplete(status_message, sense_data);
  }

  uint32_t doorbell = UtrListDoorBellReg::Get().ReadFrom(&register_).door_bell();
  if (completion_status == ZX_ERR_TIMED_OUT && (doorbell & (1u << slot_num))) {
    // Hardware still owns the slot; do not unpin PMT memory until hardware doorbell clears
    // or controller reset / task abort finishes.
    SetSlotStateLocked(slot_num, SlotState::kTimeout);
    return ZX_ERR_TIMED_OUT;
  }

  // Unpin data buffer before signalling request completion to the upper layer. This is
  // necessary because the filesystem is allowed to transfer pages directly out of this
  // buffer.
  if (request_slot.pmt.is_valid()) {
    if (zx_status_t unpin_status = request_slot.pmt.unpin(); unpin_status != ZX_OK) {
      fdf::error("Failed to unpin IO buffer: {}", zx_status_get_string(unpin_status));
      if (request_result.is_ok()) {
        request_result = zx::error(unpin_status);
      }
    }
  }

  if (response.GetHeader().event_alert()) {
    if (zx::result result = controller_.GetDeviceManager().PostExceptionEventsTask();
        result.is_error()) {
      fdf::error("Failed to handle Exception Event slot[{}]: {}", slot_num, result.status_string());
    }
  }

  return request_result.status_value();
}

void TransferRequestProcessor::RequestCompletion(uint8_t slot_num, RequestSlot &request_slot,
                                                 bool is_timeout,
                                                 fit::callback<void(zx_status_t)> &cb,
                                                 zx_status_t &status) {
  zx_status_t completion_status = ZX_OK;
  if (is_timeout) {
    // UTRLDBR bit is still set: tell the host controller to abandon the slot
    // *before* UpiuCompletion() unpins the client VMO and completes the block
    // op, otherwise a late DATA-IN UPIU will DMA into freed pages.
    // UFSHCI 3.0 5.4.4: UTRLCLR is W0C - write 0 to the slot bit to clear it.
    UtrListClearReg::Get().FromValue(~(1u << slot_num)).WriteTo(&register_);
    SetSlotStateLocked(slot_num, SlotState::kTimeout);
    completion_status = ZX_ERR_TIMED_OUT;
  } else if (request_slot.data_vmo->is_valid() && request_slot.is_read &&
             request_slot.dma_length > 0) {
    // Clean and invalidate the cache so that the next CPU read picks up data written to main memory
    // by the controller (discarding any speculative CPU prefetches during DMA).
    completion_status =
        request_slot.data_vmo->op_range(ZX_VMO_OP_CACHE_CLEAN_INVALIDATE, request_slot.dma_offset,
                                        request_slot.dma_length, nullptr, 0);
    if (completion_status != ZX_OK) {
      fdf::error("Failed to invalidate cache for data VMO: {}",
                 zx_status_get_string(completion_status));
    }
  }
  // Check request response.
  status = UpiuCompletion(slot_num, request_slot, completion_status);
  if (status == ZX_ERR_UNAVAILABLE) {
    fdf::warn(
        "Unavailability reported for request, slot[{}] "
        "(Possibly a UNIT_ATTENTION condition)",
        slot_num);
  } else if (status != ZX_OK) {
    fdf::debug("Failed to complete request, slot[{}]: {}", slot_num, zx_status_get_string(status));
  }
  request_slot.result = status;

  cb = std::move(request_slot.completion_cb);

  if (!is_timeout) {
    UtrListCompletionNotificationReg::Get()
        .FromValue(0)
        .set_notification(1u << slot_num)
        .WriteTo(&register_);

    if (zx::result result = ClearSlotLocked(request_slot); result.is_error()) {
      fdf::error("Failed to clear slot[{}]: {}", slot_num, result);
    }
  }
}

bool TransferRequestProcessor::ProcessSlotCompletion(uint8_t slot_num, uint32_t doorbell,
                                                     fit::callback<void(zx_status_t)> &cb,
                                                     zx_status_t &status) {
  bool is_completed = false;
  RequestSlot &request_slot = slots_.GetSlot(slot_num);
  if (request_slot.state == SlotState::kScheduled) {
    auto descriptor = slots_.GetRequestDescriptor<TransferRequestDescriptor>(slot_num);
    if (!(doorbell & (1u << slot_num))) {
      // Hardware cleared doorbell; ensure descriptor DMA writes are visible before checking status.
      // Tight spin-wait for DMA write-posting interconnect buffer settling (typically < 100ns).
      for (uint32_t retry = 0;
           retry < kOverallCompletionRetries &&
           descriptor->overall_command_status() == OverallCommandStatus::kInvalid;
           ++retry) {
        std::atomic_thread_fence(std::memory_order_acquire);
      }
      if (descriptor->overall_command_status() == OverallCommandStatus::kInvalid) {
        if (zx_clock_get_monotonic() >= request_slot.deadline) {
          fdf::warn("Doorbell cleared for slot[{}] but OCS remained invalid and deadline passed",
                    slot_num);
          RequestCompletion(slot_num, request_slot, /*is_timeout=*/true, cb, status);
          return true;
        }
        return false;
      }
      RequestCompletion(slot_num, request_slot, /*is_timeout=*/false, cb, status);
      is_completed = true;
    } else if (request_slot.deadline < zx_clock_get_monotonic()) {
      RequestCompletion(slot_num, request_slot, /*is_timeout=*/true, cb, status);
      is_completed = true;
    }
  }
  return is_completed;
}

uint32_t TransferRequestProcessor::ProcessCompletionOfAdminRequests() {
  if (disable_completion_.load(std::memory_order_acquire)) {
    return 0;
  }
  const uint32_t doorbell = ReadDoorBellRegister();
  fit::callback<void(zx_status_t)> cb;
  zx_status_t status = ZX_OK;
  bool completed = false;
  {
    std::lock_guard<std::mutex> lock(slot_lock_);
    completed = ProcessSlotCompletion(kAdminCommandSlotNumber, doorbell, cb, status);
  }
  if (cb) {
    cb(status);
  }
  return completed ? 1 : 0;
}

uint32_t TransferRequestProcessor::ProcessCompletionOfIoRequests() {
  if (disable_completion_.load(std::memory_order_acquire)) {
    return 0;
  }

  uint32_t active_slots;
  {
    std::lock_guard<std::mutex> lock(slot_lock_);
    active_slots = allocated_slots_mask_ & ~(1u << kAdminCommandSlotNumber);
  }

  if (active_slots == 0) {
    return 0;
  }

  const uint32_t doorbell = ReadDoorBellRegister();
  uint32_t completion_count = 0;
  while (active_slots != 0) {
    const uint8_t slot_num = static_cast<uint8_t>(std::countr_zero(active_slots));
    active_slots &= active_slots - 1;
    fit::callback<void(zx_status_t)> cb;
    zx_status_t status = ZX_OK;
    {
      std::lock_guard<std::mutex> lock(slot_lock_);
      if (ProcessSlotCompletion(slot_num, doorbell, cb, status)) {
        completion_count++;
      }
    }
    if (cb) {
      cb(status);
    }
  }
  return completion_count;
}

zx_time_t TransferRequestProcessor::GetEarliestTimeoutDeadline() {
  std::lock_guard<std::mutex> lock(slot_lock_);
  zx_time_t deadline = ZX_TIME_INFINITE;
  ForEachAllocatedSlotLocked(
      [&](uint8_t slot_num, RequestSlot &request_slot) {
        if (request_slot.state == SlotState::kScheduled) {
          deadline = std::min(deadline, request_slot.deadline);
        }
      },
      /*excluded_mask=*/(1u << kAdminCommandSlotNumber));
  return deadline;
}

zx::result<> TransferRequestProcessor::FillDescriptorAndSendRequest(
    uint8_t slot, const DataDirection data_dir, const uint16_t response_offset,
    const uint16_t response_length, const uint16_t prdt_offset, const uint32_t prdt_entry_count,
    const bool reliable_write) {
  auto descriptor = slots_.GetRequestDescriptor<TransferRequestDescriptor>(slot);
  RequestSlot &request_slot = slots_.GetSlot(slot);
  constexpr uint16_t kDwordSize = 4;
  request_slot.response_upiu_offset =
      static_cast<uint16_t>(fbl::round_down(response_offset, kDwordSize));
  zx_paddr_t paddr = request_slot.command_descriptor_io->phys();

  // Fill up UTP Transfer Request Descriptor.
  CustomMemSet(descriptor, 0, sizeof(TransferRequestDescriptor));
  descriptor->set_interrupt(true);
  descriptor->set_ru(reliable_write);
  descriptor->set_data_direction(data_dir);
  descriptor->set_command_type(kCommandTypeUfsStorage);
  // If the command was successful, overwrite |overall_command_status| field with |kSuccess|.
  descriptor->set_overall_command_status(OverallCommandStatus::kInvalid);
  descriptor->set_utp_command_descriptor_base_address(static_cast<uint32_t>(paddr & 0xffffffff));
  descriptor->set_utp_command_descriptor_base_address_upper(static_cast<uint32_t>(paddr >> 32));

  descriptor->set_response_upiu_offset(response_offset / kDwordSize);
  descriptor->set_response_upiu_length(response_length / kDwordSize);
  descriptor->set_prdt_offset(prdt_entry_count > 0 ? (prdt_offset / kDwordSize) : 0);
  descriptor->set_prdt_length(prdt_entry_count);

  TRACE_DURATION("ufs", "RingRequestDoorbell", "slot", slot);
  if (zx::result<> result = controller_.Notify(NotifyEvent::kSetupTransferRequestList, slot);
      result.is_error()) {
    return result.take_error();
  }
  RingRequestDoorbellLocked(slot);
  return zx::ok();
}

scsi::HostStatusCode TransferRequestProcessor::ScsiStatusToHostStatus(
    scsi::StatusCode scsi_status) {
  scsi::HostStatusCode host_status;
  switch (scsi_status) {
    case scsi::StatusCode::GOOD:
    case scsi::StatusCode::CHECK_CONDITION:
      host_status = scsi::HostStatusCode::kOk;
      break;
    case scsi::StatusCode::BUSY:
    case scsi::StatusCode::TASK_SET_FULL:
      host_status = scsi::HostStatusCode::kRequeue;
      break;
    case scsi::StatusCode::RESERVATION_CONFILCT:  // optional
      host_status = scsi::HostStatusCode::kOk;
      break;
    default:
      host_status = scsi::HostStatusCode::kError;
      break;
  }
  return host_status;
}

scsi::HostStatusCode TransferRequestProcessor::GetScsiCommandHostStatus(
    OverallCommandStatus ocs, UpiuHeaderResponseCode header_response,
    scsi::StatusCode scsi_status) {
  scsi::HostStatusCode host_status;
  switch (ocs) {
    case kSuccess:
      if (header_response == UpiuHeaderResponseCode::kTargetSuccess) {
        host_status = ScsiStatusToHostStatus(static_cast<scsi::StatusCode>(scsi_status));
      } else {
        host_status = scsi::HostStatusCode::kError;
      }
      break;
    case kAborted:
      host_status = scsi::HostStatusCode::kAbort;
      break;
    case kInvalid:
      host_status = scsi::HostStatusCode::kRequeue;
      break;
    default:
      host_status = scsi::HostStatusCode::kError;
      break;
  }
  return host_status;
}

scsi::StatusMessage TransferRequestProcessor::CheckScsiAndGetStatusMessage(
    uint8_t slot_num, AbstractResponseUpiu &response) {
  auto descriptor = slots_.GetRequestDescriptor<TransferRequestDescriptor>(slot_num);
  OverallCommandStatus ocs = descriptor->overall_command_status();
  auto header_response = static_cast<UpiuHeaderResponseCode>(response.GetHeader().response);
  auto scsi_status = static_cast<scsi::StatusCode>(response.GetHeader().status);

  scsi::StatusMessage message;
  message.host_status_code = GetScsiCommandHostStatus(ocs, header_response, scsi_status);
  message.scsi_status_code = scsi_status;
  return message;
}

zx::result<> TransferRequestProcessor::CheckResponse(uint8_t slot_num,
                                                     AbstractResponseUpiu &response) {
  auto transaction_type = static_cast<UpiuTransactionCodes>(response.GetHeader().trans_type);
  auto descriptor = slots_.GetRequestDescriptor<TransferRequestDescriptor>(slot_num);
  OverallCommandStatus ocs = descriptor->overall_command_status();
  auto header_response = static_cast<UpiuHeaderResponseCode>(response.GetHeader().response);

  switch (transaction_type) {
    case UpiuTransactionCodes::kResponse:
      if (response.GetHeader().command_set_type() != UpiuCommandSetType::kScsi) {
        fdf::error(
            "Unknown command(set type = 0x{:x}) response: ocs=0x{:x}, header_response=0x{:x}",
            static_cast<uint32_t>(response.GetHeader().command_set_type()),
            static_cast<uint32_t>(ocs), static_cast<uint32_t>(header_response));
        return zx::error(ZX_ERR_BAD_STATE);
      }
      // For SCSI commands, check ocs and header_response in CheckScsiAndGetStatusMessage().
      break;
    case UpiuTransactionCodes::kQueryResponse:
      if (ocs != OverallCommandStatus::kSuccess ||
          header_response != static_cast<uint8_t>(QueryResponseCode::kSuccess)) {
        fdf::error("Query request failure: ocs=0x{:x}, header_response=0x{:x}",
                   static_cast<uint32_t>(ocs), static_cast<uint32_t>(header_response));
        return zx::error(ZX_ERR_BAD_STATE);
      }
      break;
    default:
      if (ocs != OverallCommandStatus::kSuccess ||
          header_response != UpiuHeaderResponseCode::kTargetSuccess) {
        fdf::error("Generic request(transaction type = 0x{:x}) failure: ocs=0x{:x}",
                   static_cast<uint32_t>(transaction_type), static_cast<uint32_t>(ocs));
        return zx::error(ZX_ERR_BAD_STATE);
      }
      break;
  }
  return zx::ok();
}

}  // namespace ufs
