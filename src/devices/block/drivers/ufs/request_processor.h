// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_REQUEST_PROCESSOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_REQUEST_PROCESSOR_H_

#include <lib/driver/mmio/cpp/mmio.h>
#include <lib/zircon-internal/thread_annotations.h>

#include "request_list.h"

namespace ufs {

constexpr zx::duration kCommandTimeout = zx::sec(10);

class Ufs;

class RequestProcessor {
 public:
  explicit RequestProcessor(RequestList request_list, Ufs &ufs, zx::unowned_bti bti,
                            const fdf::MmioView mmio, uint32_t slot_count)
      : slots_(std::move(request_list)),
        controller_(ufs),
        register_(mmio),
        bti_(std::move(bti)),
        slot_count_(safemath::checked_cast<uint8_t>(slot_count)) {}
  virtual ~RequestProcessor() = default;

  template <typename Processor, typename Descriptor>
  static zx::result<std::unique_ptr<Processor>> Create(Ufs &ufs, zx::unowned_bti bti,
                                                       const fdf::MmioView mmio,
                                                       uint8_t entry_count);

  // Write the address of the list to the list base address register and set the run-stop register.
  virtual zx::result<> Init() = 0;

  // Check all slots to process completed requests. This function returns the number of completed
  // requests. This function is called by the ISR.
  virtual uint32_t ProcessCompletionOfIoRequests() = 0;

  uint8_t GetSlotCount() const { return slot_count_; }

  // For testing
  RequestList &GetRequestListLocked() TA_REQ(slot_lock_) { return slots_; }
  const RequestList &GetRequestListLocked() const TA_REQ(slot_lock_) { return slots_; }
  std::mutex &GetSlotLock() const TA_RET_CAP(slot_lock_) { return slot_lock_; }
  void SetTimeout(zx::duration timeout) { timeout_ = timeout; }
  zx::duration GetTimeout() const { return timeout_; }
  void DisableCompletion() { disable_completion_.store(true, std::memory_order_release); }
  void EnableCompletion() { disable_completion_.store(false, std::memory_order_release); }

 protected:
  zx::unowned_bti &GetBti() { return bti_; }

  void SetSlotStateLocked(uint8_t slot_num, SlotState state) TA_REQ(slot_lock_) {
    slots_.GetSlot(slot_num).state = state;
  }

  // Get the number of the free slot and mark it as |SlotState::kReserved|.
  zx::result<uint8_t> ReserveSlot() TA_EXCL(slot_lock_);
  zx::result<> ClearSlot(RequestSlot &request_slot) TA_EXCL(slot_lock_);
  zx::result<> ClearSlotLocked(RequestSlot &request_slot) TA_REQ(slot_lock_);
  void ReclaimTimedOutSlotsLocked() TA_REQ(slot_lock_);

  template <typename Callback>
  void ForEachAllocatedSlotLocked(Callback &&callback, uint32_t excluded_mask = 0)
      TA_REQ(slot_lock_) {
    uint32_t mask = allocated_slots_mask_ & ~excluded_mask;
    while (mask != 0) {
      const uint8_t slot_num = static_cast<uint8_t>(std::countr_zero(mask));
      mask &= mask - 1;
      callback(slot_num, slots_.GetSlot(slot_num));
    }
  }

  // Ring the door bell.
  void RingRequestDoorbell(uint8_t slot_num) TA_EXCL(slot_lock_);
  void RingRequestDoorbellLocked(uint8_t slot_num) TA_REQ(slot_lock_);

  // Protects slots, slot allocation, and state transitions.
  mutable std::mutex slot_lock_;
  uint32_t allocated_slots_mask_ TA_GUARDED(slot_lock_) = 0;
  RequestList slots_ TA_GUARDED(slot_lock_);

  Ufs &controller_;
  const fdf::MmioView register_;

  zx::duration timeout_ = kCommandTimeout;

  std::atomic<bool> disable_completion_{false};

 private:
  virtual std::optional<uint8_t> GetAdminCommandSlotNumber() const { return std::nullopt; }

  virtual void SetDoorBellRegister(uint8_t slot_num) = 0;
  virtual uint32_t ReadDoorBellRegister() = 0;

  zx::unowned_bti bti_;
  const uint8_t slot_count_;
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_REQUEST_PROCESSOR_H_
