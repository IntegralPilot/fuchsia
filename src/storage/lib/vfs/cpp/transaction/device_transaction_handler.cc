// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/transaction/device_transaction_handler.h"

#include <fuchsia/hardware/block/driver/c/banjo.h>
#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <cstddef>
#include <cstdint>
#include <limits>
#include <vector>

#include "src/storage/lib/block_protocol/block-fifo.h"
#include "src/storage/lib/operation/operation.h"
#include "src/storage/lib/trace/trace.h"

namespace fs {

zx_status_t DeviceTransactionHandler::RunRequests(
    const std::vector<storage::BufferedOperation>& operations,
    const DecompressionInfo* decompression_info) {
  if (operations.empty()) {
    return ZX_OK;
  }

  TRACE_DURATION("storage", "DeviceTransactionHandler::RunRequests", "num", operations.size());
  std::vector<uint64_t> trace_flow_ids;

  // Update all the outgoing transactions to be in disk blocks.
  std::vector<BlockFifoRequest> block_requests(operations.size());
  {
    // This duration mainly exists to give the below TRACE_FLOW_BEGIN calls a context which ends
    // before the actual blocking call to |FifoTransaction|. Flow events originate from the end of
    // the duration that they were defined in.
    TRACE_DURATION("storage", "DeviceTransactionHandler::RunRequests::Enqueue");
    for (size_t i = 0; i < operations.size(); ++i) {
      auto& request = block_requests[i];
      request.vmoid = operations[i].vmoid;

      const auto& operation = operations[i].op;
      switch (operation.type) {
        case storage::OperationType::kRead:
          request.command = {.opcode = BLOCK_OPCODE_READ, .flags = 0};
          break;
        case storage::OperationType::kWrite:
          request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = 0};
          break;
        case storage::OperationType::kTrim:
          request.command = {.opcode = BLOCK_OPCODE_TRIM, .flags = 0};
          break;
        case storage::OperationType::kWriteFua:
          request.command = {.opcode = BLOCK_OPCODE_WRITE, .flags = BLOCK_IO_FLAG_FORCE_ACCESS};
          break;
        default:
          ZX_DEBUG_ASSERT_MSG(false, "Unsupported operation");
      }
      // For the time being, restrict a transaction to operations of the same type. This probably
      // can be relaxed, as the concept of a transaction implies the operations take place logically
      // at the same time, so even if there's a mix of reads and writes, it doesn't make sense to
      // depend on the relative order of the operations, which is what could break with the merging
      // done by the request builder.
      ZX_DEBUG_ASSERT(request.command.opcode == block_requests[0].command.opcode);

      if (decompression_info) {
        if (i == 0) {
          request.vmo_offset = BlockNumberToDevice(operation.vmo_offset);
          request.uncompressed_bytes = decompression_info->uncompressed_bytes;
          request.total_compressed_bytes = decompression_info->compressed_bytes;
          request.compressed_prefix_bytes = decompression_info->compressed_prefix_bytes;
        }
        request.command.flags |= BLOCK_IO_FLAG_DECOMPRESS_WITH_ZSTD;
      } else {
        request.vmo_offset = BlockNumberToDevice(operation.vmo_offset);
      }
      request.dev_offset = BlockNumberToDevice(operation.dev_offset);
      uint64_t length = BlockNumberToDevice(operation.length);
      if (length > std::numeric_limits<decltype(request.length)>::max()) {
        return ZX_ERR_OUT_OF_RANGE;
      }
      request.length = static_cast<decltype(request.length)>(length);
      ZX_ASSERT(request.length != 0);
      if (operation.trace_flow_id) {
        // Client provided an explicit flow ID, no need to begin a new flow.
        request.trace_flow_id = operation.trace_flow_id;
      } else {
        request.trace_flow_id = storage::trace::GenerateTraceId();
        TRACE_FLOW_BEGIN("storage", "BlockOp", request.trace_flow_id);
        trace_flow_ids.push_back(request.trace_flow_id);
      }
    }
  }

  zx_status_t status = GetDevice()->FifoTransaction(block_requests.data(), operations.size());

  TRACE_DURATION("storage", "DeviceTransactionHandler::RunRequests::Finish");
  for (const auto& id : trace_flow_ids) {
    TRACE_FLOW_END("storage", "BlockOp", id);
  }

  return status;
}

zx_status_t DeviceTransactionHandler::Flush() {
  BlockFifoRequest request = {
      .command = {.opcode = BLOCK_OPCODE_FLUSH, .flags = 0},
  };
  return GetDevice()->FifoTransaction(&request, 1);
}

}  // namespace fs
