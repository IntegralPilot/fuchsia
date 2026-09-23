// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_TRANSFER_REQUEST_PROCESSOR_H_
#define SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_TRANSFER_REQUEST_PROCESSOR_H_

#include <lib/driver/mmio/cpp/mmio-buffer.h>
#include <lib/mmio-ptr/fake.h>

#include <functional>
#include <vector>

#include "handler.h"
#include "src/devices/block/drivers/ufs/ufs.h"

namespace ufs {
namespace ufs_mock_device {

class UfsMockDevice;

struct CommandDescriptorData {
  zx_vaddr_t command_upiu_base_addr;
  zx_vaddr_t response_upiu_base_addr;
  uint32_t response_upiu_length;
  zx_vaddr_t prdt_base_addr;
  uint32_t prdt_entry_count;
};

class TransferRequestProcessor {
 public:
  using TransferRequestHandler = std::function<zx_status_t(UfsMockDevice &, CommandDescriptorData)>;

  TransferRequestProcessor(const TransferRequestProcessor &) = delete;
  TransferRequestProcessor &operator=(const TransferRequestProcessor &) = delete;
  TransferRequestProcessor(const TransferRequestProcessor &&) = delete;
  TransferRequestProcessor &operator=(const TransferRequestProcessor &&) = delete;
  ~TransferRequestProcessor() = default;
  explicit TransferRequestProcessor(UfsMockDevice &mock_device) : mock_device_(mock_device) {}
  zx_status_t HandleTransferRequest(TransferRequestDescriptor &descriptor);

  static zx_status_t DefaultNopOutHandler(UfsMockDevice &mock_device,
                                          CommandDescriptorData command_descriptor_data);

  static zx_status_t DefaultQueryHandler(UfsMockDevice &mock_device,
                                         CommandDescriptorData command_descriptor_data);

  static zx_status_t DefaultCommandHandler(UfsMockDevice &mock_device,
                                           CommandDescriptorData command_descriptor_data);

  DEF_DEFAULT_HANDLER_BEGIN(UpiuTransactionCodes, TransferRequestHandler)
  DEF_DEFAULT_HANDLER(UpiuTransactionCodes::kNopOut, DefaultNopOutHandler)
  DEF_DEFAULT_HANDLER(UpiuTransactionCodes::kQueryRequest, DefaultQueryHandler)
  DEF_DEFAULT_HANDLER(UpiuTransactionCodes::kCommand, DefaultCommandHandler)
  DEF_DEFAULT_HANDLER_END()

  void ResetExtra() { pending_slots_.reset(); }

  std::bitset<kMaxRequestListSize> &GetPendingSlots() { return pending_slots_; }
  void SetPendingSlots(std::bitset<kMaxRequestListSize> value) { pending_slots_ = value; }

 private:
  UfsMockDevice &mock_device_;

  // Since the doorbell information is not stored on the mock device, we record it in pending_slots_
  // to indicate which slots are being processed.
  std::bitset<kMaxRequestListSize> pending_slots_ = 0;
};

}  // namespace ufs_mock_device
}  // namespace ufs

#endif  // SRC_DEVICES_BLOCK_DRIVERS_UFS_TEST_MOCK_DEVICE_TRANSFER_REQUEST_PROCESSOR_H_
