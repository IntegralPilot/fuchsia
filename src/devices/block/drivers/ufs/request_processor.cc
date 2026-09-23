// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "request_processor.h"

#include <lib/driver/logging/cpp/logger.h>

#include <bit>

#include "src/devices/block/drivers/ufs/ufs.h"

namespace ufs {

template <typename Processor, typename Descriptor>
zx::result<std::unique_ptr<Processor>> RequestProcessor::Create(Ufs &ufs, zx::unowned_bti bti,
                                                                const fdf::MmioView mmio,
                                                                uint8_t entry_count) {
  zx::result<RequestList> request_list =
      RequestList::Create(bti->borrow(), sizeof(Descriptor), entry_count);
  if (request_list.is_error()) {
    return request_list.take_error();
  }

  fbl::AllocChecker ac;
  auto request_processor = fbl::make_unique_checked<Processor>(
      &ac, std::move(request_list.value()), ufs, std::move(bti), mmio, entry_count);
  if (!ac.check()) {
    fdf::error("Failed to allocate request processor.");
    return zx::error(ZX_ERR_NO_MEMORY);
  }
  return zx::ok(std::move(request_processor));
}

template zx::result<std::unique_ptr<TransferRequestProcessor>>
RequestProcessor::Create<TransferRequestProcessor, TransferRequestDescriptor>(
    Ufs &ufs, zx::unowned_bti bti, const fdf::MmioView mmio, uint8_t entry_count);

template zx::result<std::unique_ptr<TaskManagementRequestProcessor>>
RequestProcessor::Create<TaskManagementRequestProcessor, TaskManagementRequestDescriptor>(
    Ufs &ufs, zx::unowned_bti bti, const fdf::MmioView mmio, uint8_t entry_count);

void RequestProcessor::ReclaimTimedOutSlotsLocked() {
  const uint32_t doorbell = ReadDoorBellRegister();
  ForEachAllocatedSlotLocked(
      [this, doorbell](uint8_t slot_num, RequestSlot &slot) TA_REQ(slot_lock_) {
        if (slot.state == SlotState::kTimeout && (doorbell & (1u << slot_num)) == 0) {
          if (zx::result<> result = ClearSlotLocked(slot); result.is_error()) {
            fdf::error("Failed to clear timed out slot[{}]: {}", slot_num, result);
          }
        }
      });
}

zx::result<uint8_t> RequestProcessor::ReserveSlot() {
  std::lock_guard<std::mutex> lock(slot_lock_);
  ReclaimTimedOutSlotsLocked();

  std::optional<uint8_t> admin_slot_num = GetAdminCommandSlotNumber();
  uint32_t excluded_mask = admin_slot_num.has_value() ? (1u << *admin_slot_num) : 0;

  uint32_t free_slots_mask = ~allocated_slots_mask_ & slots_.GetSlotMask() & ~excluded_mask;
  if (free_slots_mask != 0) {
    const uint8_t slot_num = static_cast<uint8_t>(std::countr_zero(free_slots_mask));
    allocated_slots_mask_ |= (1u << slot_num);
    slots_.GetSlot(slot_num).Reset(SlotState::kReserved);
    return zx::ok(slot_num);
  }
  fdf::debug("Failed to reserve a request slot");
  return zx::error(ZX_ERR_NO_RESOURCES);
}

zx::result<> RequestProcessor::ClearSlotLocked(RequestSlot &request_slot) {
  if (request_slot.pmt.is_valid()) {
    zx_status_t status = request_slot.pmt.unpin();
    request_slot.pmt = zx::pmt();
    if (status != ZX_OK) {
      fdf::error("Failed to unpin IO buffer: {}", zx_status_get_string(status));
      request_slot.result = status;
      return zx::error(status);
    }
  }

  uint8_t slot_num = slots_.GetSlotNum(request_slot);
  request_slot.Reset(SlotState::kFree);
  allocated_slots_mask_ &= ~(1u << slot_num);
  return zx::ok();
}

zx::result<> RequestProcessor::ClearSlot(RequestSlot &request_slot) {
  std::lock_guard<std::mutex> lock(slot_lock_);
  return ClearSlotLocked(request_slot);
}

void RequestProcessor::RingRequestDoorbellLocked(uint8_t slot_num) {
  RequestSlot &request_slot = slots_.GetSlot(slot_num);
  sync_completion_t *complete = &request_slot.complete;
  sync_completion_reset(complete);
  request_slot.deadline = zx_deadline_after(GetTimeout().get());
  ZX_DEBUG_ASSERT(request_slot.state == SlotState::kReserved);
  request_slot.state = SlotState::kScheduled;
  SetDoorBellRegister(slot_num);

  // TODO(https://fxbug.dev/42075643): Set the UtrInterruptAggregationControlReg.
}

void RequestProcessor::RingRequestDoorbell(uint8_t slot_num) {
  std::lock_guard<std::mutex> lock(slot_lock_);
  RingRequestDoorbellLocked(slot_num);
}

}  // namespace ufs
