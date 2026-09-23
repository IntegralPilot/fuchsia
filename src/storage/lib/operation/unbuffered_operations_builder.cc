// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/lib/operation/unbuffered_operations_builder.h"

#include <vector>

namespace storage {

UnbufferedOperationsBuilder::~UnbufferedOperationsBuilder() = default;

uint64_t UnbufferedOperationsBuilder::BlockCount() const { return tree_.BlockCount(); }

void UnbufferedOperationsBuilder::Add(const UnbufferedOperation& new_operation) {
  ZX_DEBUG_ASSERT_MSG(
      (new_operation.data != nullptr) ^ new_operation.vmo->is_valid(),
      "Exactly one of data pointer or vmo handle should be set. Pointer is valid: %b vmo is valid: %b",
      new_operation.data != nullptr, new_operation.vmo->is_valid());
  tree_.insert(new_operation);
}

std::vector<UnbufferedOperation> UnbufferedOperationsBuilder::TakeOperations() {
  return tree_.TakeOperations();
}

}  // namespace storage
