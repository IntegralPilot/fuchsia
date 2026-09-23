// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_VFS_CPP_TRANSACTION_DEVICE_TRANSACTION_HANDLER_H_
#define SRC_STORAGE_LIB_VFS_CPP_TRANSACTION_DEVICE_TRANSACTION_HANDLER_H_

#include <zircon/types.h>

#include <vector>

#include "src/storage/lib/block_client/cpp/block_device.h"
#include "src/storage/lib/operation/operation.h"
#include "src/storage/lib/vfs/cpp/transaction/transaction_handler.h"

namespace fs {

// Provides a reasonable implementation of RunRequests that issues requests to a BlockDevice.
class DeviceTransactionHandler : public TransactionHandler {
 public:
  struct DecompressionInfo {
    uint32_t uncompressed_bytes = 0;
    uint32_t compressed_bytes = 0;
    uint16_t compressed_prefix_bytes = 0;
  };

  zx_status_t RunRequests(const std::vector<storage::BufferedOperation>& operations,
                          const DecompressionInfo* decompression_info);

  zx_status_t RunRequests(const std::vector<storage::BufferedOperation>& operations) override {
    return RunRequests(operations, nullptr);
  }

  // Returns the backing block device that is associated with this TransactionHandler.
  virtual block_client::BlockDevice* GetDevice() = 0;

  zx_status_t Flush() override;
};

}  // namespace fs

#endif  // SRC_STORAGE_LIB_VFS_CPP_TRANSACTION_DEVICE_TRANSACTION_HANDLER_H_
