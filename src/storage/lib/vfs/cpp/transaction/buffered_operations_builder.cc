// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/vfs/cpp/transaction/buffered_operations_builder.h"

#include <algorithm>
#include <utility>
#include <vector>

#include "src/storage/lib/buffer/block_buffer.h"
#include "src/storage/lib/operation/operation.h"

namespace fs {
namespace {
// Check if |operation| is a mergeable type. Write requests requiring flush or fua should not be
// merged with other operations.
bool IsMergeableIoType(const storage::Operation& operation) {
  return operation.type == storage::OperationType::kWrite ||
         operation.type == storage::OperationType::kRead;
}

}  // namespace

BufferedOperationsBuilder& BufferedOperationsBuilder::Add(const storage::Operation& new_operation,
                                                          storage::BlockBuffer* buffer) {
  // TODO(rvargas): consider unifying the logic with UnbufferedOperationsBuilder.
  for (auto& old_operation : operations_) {
    storage::Operation& operation = old_operation.op;
    if (operation.type != new_operation.type || !IsMergeableIoType(operation) ||
#ifdef __Fuchsia__
        old_operation.vmoid != buffer->vmoid()) {
#else
        old_operation.data != buffer->Data(0)) {
#endif
      continue;
    }

    if (operation.vmo_offset == new_operation.vmo_offset &&
        operation.dev_offset == new_operation.dev_offset) {
      // Take the longer of the operations (if operating on the same blocks).
      operation.length = std::max(operation.length, new_operation.length);
      return *this;
    }
    if ((operation.vmo_offset + operation.length == new_operation.vmo_offset) &&
        (operation.dev_offset + operation.length == new_operation.dev_offset)) {
      // Combine with the previous request, if immediately following.
      operation.length += new_operation.length;
      return *this;
    }
  }

  storage::BufferedOperation buffered_operation;
  buffered_operation.op = new_operation;
#ifdef __Fuchsia__
  buffered_operation.vmoid = buffer->vmoid();
#else
  buffered_operation.data = buffer->Data(0);
#endif

  operations_.push_back(buffered_operation);
  return *this;
}

std::vector<storage::BufferedOperation> BufferedOperationsBuilder::TakeOperations() {
  return std::move(operations_);
}

}  // namespace fs
