// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_EXTENT_CACHE_H_
#define SRC_STORAGE_F2FS_EXTENT_CACHE_H_

#include <condition_variable>
#include <utility>

#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_wavl_tree.h>
#include <safemath/checked_math.h>

#include "src/storage/f2fs/common.h"

namespace f2fs {

constexpr uint32_t kMinExtentLen = 64;

struct ExtentInfo {
  pgoff_t fofs = 0;      // start offset in a file
  block_t blk_addr = 0;  // start block address of the extent
  uint32_t len = 0;      // length of the extent
};

class ExtentTree;
class ExtentNode : public fbl::WAVLTreeContainable<std::unique_ptr<ExtentNode>> {
 public:
  explicit ExtentNode(const ExtentInfo &extent_info) : extent_info_(extent_info) {}
  ExtentNode(const ExtentNode &) = delete;
  ExtentNode &operator=(const ExtentNode &) = delete;
  ExtentNode(ExtentNode &&) = delete;
  ExtentNode &operator=(ExtentNode &&) = delete;
  virtual ~ExtentNode() { ZX_DEBUG_ASSERT(!InContainer()); }

  pgoff_t GetKey() const { return extent_info_.fofs; }

  ExtentInfo &GetExtentInfo() { return extent_info_; }

 private:
  ExtentInfo extent_info_;
};

class ExtentTree : public fbl::WAVLTreeContainable<ExtentTree *> {
 public:
  ExtentTree() = default;
  ExtentTree(const ExtentTree &) = delete;
  ExtentTree &operator=(const ExtentTree &) = delete;
  ExtentTree(ExtentTree &&) = delete;
  ExtentTree &operator=(ExtentTree &&) = delete;
  virtual ~ExtentTree() { Reset(); }

  zx::result<> InsertExtent(ExtentInfo extent_info) __TA_EXCLUDES(tree_lock_);
  zx::result<ExtentInfo> LookupExtent(pgoff_t file_offset) __TA_EXCLUDES(tree_lock_);
  ExtentInfo GetLargestExtent() __TA_EXCLUDES(tree_lock_);

  void Reset() __TA_EXCLUDES(tree_lock_);

 private:
  // Since extents do not overlap, we simply construct a tree with the start address of the extent
  // as the key.
  using ExtentNodeTreeTraits = fbl::DefaultKeyedObjectTraits<pgoff_t, ExtentNode>;
  using ExtentNodeTree = fbl::WAVLTree<pgoff_t, std::unique_ptr<ExtentNode>, ExtentNodeTreeTraits>;
  std::shared_mutex tree_lock_;
  ExtentNodeTree extent_node_tree_ __TA_GUARDED(tree_lock_);
  std::optional<ExtentInfo> largest_extent_info_ __TA_GUARDED(tree_lock_);
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_EXTENT_CACHE_H_
