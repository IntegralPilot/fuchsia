// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_REQUEST_LIST_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_REQUEST_LIST_H_

#include <lib/dma-buffer/buffer.h>
#include <lib/fit/function.h>
#include <lib/zx/bti.h>
#include <lib/zx/result.h>

#include <bit>
#include <cstdint>
#include <vector>

#include <safemath/safe_conversions.h>

#include "upiu/scsi_commands.h"

namespace ufs {

// UFS 3.1 only supports up to 32 in-flight requests.
constexpr uint8_t kMaxRequestListSize = 32;

class RequestProcessor;

enum class SlotState {
  kFree = 0,
  kReserved,
  kScheduled,
  kTimeout,
};

class RequestSlot {
 public:
  SlotState state = SlotState::kFree;

  void Reset(SlotState new_state = SlotState::kFree) {
    sync_completion_reset(&complete);
    completion_cb = nullptr;
    data_vmo = {};
    dma_offset = 0;
    dma_length = 0;
    is_read = false;
    is_scsi_command = false;
    response_upiu_offset = 0;
    result = ZX_OK;
    deadline = ZX_TIME_INFINITE;
    state = new_state;
  }

  std::unique_ptr<dma_buffer::ContiguousBuffer> command_descriptor_io;
  sync_completion_t complete{};
  zx::pmt pmt;
  fit::callback<void(zx_status_t)> completion_cb;
  zx::unowned_vmo data_vmo;
  uint64_t dma_offset = 0;
  uint64_t dma_length = 0;
  bool is_read = false;
  bool is_scsi_command = false;
  uint16_t response_upiu_offset = 0;
  zx_status_t result = ZX_OK;
  zx_time_t deadline = 0;

  RequestSlot() = default;
  RequestSlot(RequestSlot &&) noexcept = default;
  RequestSlot &operator=(RequestSlot &&) noexcept = default;
};

using RequestSlotCallback = fit::function<void(uint8_t slot_num, RequestSlot &request_slot)>;

// Implements the UTP 'transfer/task management' request list.
class RequestList {
 public:
  static zx::result<RequestList> Create(zx::unowned_bti bti, size_t entry_size,
                                        uint8_t entry_count);

  // Get 'transfer/task management' request descriptor's physical address
  template <typename T>
  zx_paddr_t GetRequestDescriptorPhysicalAddress(uint8_t slot) const {
    return io_buffer_->phys() + sizeof(T) * slot;
  }
  // Get 'transfer/task management' request descriptor's virtual address
  template <typename T>
  T *GetRequestDescriptor(uint8_t slot) const {
    return static_cast<T *>(io_buffer_->virt()) + slot;
  }

  template <typename Self>
  auto &GetSlot(this Self &&self, uint8_t entry_num) {
    ZX_ASSERT_MSG(entry_num < self.request_slots_.size(), "Invalid entry_num");
    return self.request_slots_[entry_num];
  }
  uint8_t GetSlotCount() const { return safemath::checked_cast<uint8_t>(request_slots_.size()); }
  uint32_t GetSlotMask() const {
    const uint8_t count = GetSlotCount();
    // Handle full 32-slot mask to avoid 1u << 32 bitshift overflow (UB in C++).
    return count == kMaxRequestListSize ? 0xFFFFFFFFu : ((1u << count) - 1u);
  }

  uint8_t GetSlotNum(const RequestSlot &slot) const {
    ZX_ASSERT(&slot >= request_slots_.data() &&
              &slot < request_slots_.data() + request_slots_.size());
    return safemath::checked_cast<uint8_t>(&slot - request_slots_.data());
  }

  void ForEachSlot(RequestSlotCallback callback);

  template <typename T = void>
  T *GetDescriptorBuffer(uint8_t entry_num, uint16_t offset = 0) {
    ZX_ASSERT_MSG(entry_num < request_slots_.size(), "Invalid entry_num");
    return reinterpret_cast<T *>(
        reinterpret_cast<uint8_t *>(request_slots_[entry_num].command_descriptor_io->virt()) +
        offset);
  }

  size_t GetDescriptorBufferSize(uint8_t entry_num) {
    ZX_ASSERT_MSG(entry_num < request_slots_.size(), "Invalid entry_num");
    return request_slots_[entry_num].command_descriptor_io->size();
  }

 private:
  zx::result<> Init(zx::unowned_bti bti, size_t entry_size, uint8_t entry_count);
  zx::result<> IoBufferInit(zx::unowned_bti &bti, std::unique_ptr<dma_buffer::ContiguousBuffer> *io,
                            size_t size);

  std::unique_ptr<dma_buffer::ContiguousBuffer> io_buffer_;

  // Information about the requests that exist in the request list.
  std::vector<RequestSlot> request_slots_;
};

}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_REQUEST_LIST_H_
