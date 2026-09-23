// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_LIB_OPERATION_OPERATION_TREE_H_
#define SRC_STORAGE_LIB_OPERATION_OPERATION_TREE_H_

#include <zircon/assert.h>
#include <zircon/errors.h>
#include <zircon/types.h>

#include <algorithm>
#include <cstdint>
#include <utility>
#include <vector>

#include <safemath/checked_math.h>

#include "src/storage/lib/operation/operation.h"
#include "src/storage/lib/range/interval-tree.h"
#include "src/storage/lib/range/range.h"

#ifdef __Fuchsia__
#include "src/storage/lib/operation/unbuffered_operation.h"
#endif

namespace storage {
namespace internal {

inline bool EqualVmoDeviceOffsetSkew(const Operation& a, const Operation& b) {
  return (a.vmo_offset - b.vmo_offset) == (a.dev_offset - b.dev_offset);
}

inline bool IsMergeableIoType(OperationType type) {
  return type == OperationType::kWrite || type == OperationType::kRead;
}

inline bool EqualSource(const BufferedOperation& a, const BufferedOperation& b) {
#ifdef __Fuchsia__
  return a.vmoid == b.vmoid;
#else
  return a.data == b.data;
#endif
}

#ifdef __Fuchsia__
inline bool EqualSource(const UnbufferedOperation& a, const UnbufferedOperation& b) {
  zx_handle_t a_handle = a.vmo->is_valid() ? a.vmo->get() : ZX_HANDLE_INVALID;
  zx_handle_t b_handle = b.vmo->is_valid() ? b.vmo->get() : ZX_HANDLE_INVALID;
  return a_handle == b_handle && a.data == b.data;
}
#endif

template <typename Op>
bool CanMergeOperations(const Op& a, const Op& b) {
  if (!EqualSource(a, b)) {
    return false;
  }
  if (a.op.type != b.op.type) {
    return false;
  }
  if (!IsMergeableIoType(a.op.type)) {
    return false;
  }
  return EqualVmoDeviceOffsetSkew(a.op, b.op);
}

// Container for operations stored in an OperationTree.
//
// The "dev_offset" is used as a key for determining overlap.
template <typename Op>
class OperationRangeContainer {
 public:
  explicit OperationRangeContainer(Op op) : operation_(std::move(op)) {}
  uint64_t Start() const { return operation_.op.dev_offset; }
  uint64_t End() const { return operation_.op.dev_offset + operation_.op.length; }
  const Op& operation() const { return operation_; }
  Op& operation() { return operation_; }
  void Update(uint64_t start, uint64_t end) {
    // Update is called during range merges and splits. During these operations, vmo_offset stays a
    // constant distance away from dev_offset. Calculate the movement of dev_offset in this
    // operation and move vmo_offset accordingly.
    int64_t diff = static_cast<int64_t>(start) - static_cast<int64_t>(operation_.op.dev_offset);
    operation_.op.vmo_offset += diff;
    operation_.op.dev_offset = start;
    operation_.op.length = end - start;
  }

 private:
  Op operation_;
};

// Traits which enable an operation to exist in an interval tree.
template <typename Op>
struct OperationRangeTraits {
  static uint64_t Start(const OperationRangeContainer<Op>& obj) { return obj.Start(); }
  static uint64_t End(const OperationRangeContainer<Op>& obj) { return obj.End(); }
  static zx_status_t Update(const OperationRangeContainer<Op>* other, uint64_t start, uint64_t end,
                            OperationRangeContainer<Op>* obj) {
    if (other) {
      if (!CanMergeOperations(other->operation(), obj->operation())) {
        return ZX_ERR_INVALID_ARGS;
      }
    }
    obj->Update(start, end);
    return ZX_OK;
  }
};

template <typename Op>
using OperationRange =
    range::Range<uint64_t, OperationRangeContainer<Op>, OperationRangeTraits<Op>>;

template <typename Op>
using OperationIntervalTree = range::IntervalTree<OperationRange<Op>>;

}  // namespace internal

// An interval tree which collects and coalesces storage operations targeting device offsets.
//
// On insertion, the tree ensures that the latest operation touching a particular destination block
// overwrites any prior operations. If possible, contiguous operations with the same source are
// merged. If an operation partially overlaps or overwrites with a different source, overlapping
// ranges are split and updated, guaranteeing there are no duplicate destination offsets.
template <typename Op>
class OperationTree {
 public:
  using RangeType = internal::OperationRange<Op>;
  using TreeType = internal::OperationIntervalTree<Op>;
  using IterType = typename TreeType::IterType;
  using ConstIterType = typename TreeType::ConstIterType;

  OperationTree() = default;

  // Inserts an operation into the tree.
  //
  // First, removes all overlapping prior operations which target the same device offset, and then
  // inserts |operation|. This ensures that the "latest operation touching block B" will be the only
  // operation targeting that block.
  void insert(Op operation) {
    if (operation.op.length == 0) {
      return;
    }
    RangeType range((internal::OperationRangeContainer<Op>(std::move(operation))));

    // Erase all prior operations which touch the same dev_offset.
    tree_.erase(range);

    // Utilize the newest operations touching dev_offset.
    tree_.insert(range);
  }

  // Returns the total number of blocks in all requests.
  [[nodiscard]] uint64_t BlockCount() const {
    safemath::CheckedNumeric<uint64_t> count = 0;
    for (const auto& [_, range] : tree_) {
      count += range.Length();
    }
    // ValueOrDie may kill this process, but it practically should never happen and the failure here
    // would likely mean that the filesystem is not going to be able to continue anyways.
    return count.ValueOrDie();
  }

  // Removes the operations from the tree, and returns them to the caller sorted by dev_offset.
  std::vector<Op> TakeOperations() {
    std::vector<Op> operations;
    operations.reserve(tree_.size());
    for (auto& [_, range] : tree_) {
      operations.push_back(std::move(range.container().operation()));
    }
    tree_.clear();
    return operations;
  }

  void clear() { tree_.clear(); }

  [[nodiscard]] bool empty() const { return tree_.empty(); }

  [[nodiscard]] size_t size() const { return tree_.size(); }

  IterType begin() { return tree_.begin(); }

  ConstIterType begin() const { return tree_.begin(); }

  IterType end() { return tree_.end(); }

  ConstIterType end() const { return tree_.end(); }

 private:
  TreeType tree_;
};

}  // namespace storage

#endif  // SRC_STORAGE_LIB_OPERATION_OPERATION_TREE_H_
