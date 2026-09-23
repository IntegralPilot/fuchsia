// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_VMO_MANAGER_H_
#define SRC_STORAGE_F2FS_VMO_MANAGER_H_

#include <fbl/intrusive_wavl_tree.h>

#include "src/storage/f2fs/common.h"
#include "src/storage/f2fs/layout.h"

namespace f2fs {

constexpr uint64_t kMaxVmoSize = std::numeric_limits<uint64_t>::max();
constexpr size_t kBlocksPerVmoNode = kDefaultBlocksPerSegment;
constexpr size_t kVmoNodeSize = safemath::CheckMul(kBlocksPerVmoNode, kBlockSize).ValueOrDie();

enum class VmoMode {
  kDiscardable = 0,
  kPaged,
};

class VmoMapping : public fbl::WAVLTreeContainable<std::unique_ptr<VmoMapping>> {
 public:
  VmoMapping() = delete;
  explicit VmoMapping(pgoff_t index, size_t size);
  virtual ~VmoMapping();

  virtual zx::result<bool> Lock(pgoff_t offset) = 0;
  virtual zx_status_t Unlock(pgoff_t offset) = 0;
  virtual zx_status_t Zero(pgoff_t start, pgoff_t len) = 0;

  zx::result<zx_vaddr_t> GetAddress(pgoff_t offset) const;
  pgoff_t GetKey() const { return index(); }
  uint64_t GetActivePages() const { return active_pages(); }

 protected:
  zx::vmo &vmo() { return owned_vmo_; }
  zx_vaddr_t address() const { return address_; }
  size_t get_size() const { return size_in_blocks_; }
  pgoff_t index() const { return index_; }
  uint64_t increase_active_pages() { return ++active_pages_; }
  uint64_t decrease_active_pages() { return --active_pages_; }
  uint64_t active_pages() const { return active_pages_.load(std::memory_order_acquire); }

 private:
  // A va mapping to |this|.
  zx_vaddr_t address_ = 0;
  // The number of active pages to |vmo_|.
  std::atomic<uint64_t> active_pages_ = 0;
  const size_t size_in_blocks_ = 0;
  const pgoff_t index_;
  zx::vmo owned_vmo_;
};

// It manages the lifecycle of a discardable Vmo that Pages use in each vnode.
class VmoDiscardable : public VmoMapping {
 public:
  VmoDiscardable() = delete;
  VmoDiscardable(const VmoDiscardable &) = delete;
  VmoDiscardable &operator=(const VmoDiscardable &) = delete;
  VmoDiscardable(const VmoDiscardable &&) = delete;
  VmoDiscardable &operator=(const VmoDiscardable &&) = delete;
  explicit VmoDiscardable(pgoff_t index, size_t size);

  ~VmoDiscardable();

  // It ensures that |vmo_| keeps VMO_OP_LOCK as long as any Pages refer to it
  // by calling Page::GetVmo(). When a valid page gets a new access to a |this|,
  // it increases VmoMapping::active_pages_, and then decreases VmoMapping::active_pages_
  // in Unlock() as a active page is truncated. When VmoMapping::active_pages_ reaches
  // zero, Unlock() does VMO_OP_UNLOCK for page reclaim.
  // When VmoMapping::active_pages is zero with new page access, it tries VMO_OP_TRY_LOCK
  // to check which pages kernel has reclaimed.
  zx::result<bool> Lock(pgoff_t offset) final;
  // It unlocks |vmo_| when there is no Page using it.
  zx_status_t Unlock(pgoff_t offset) final;
  zx_status_t Zero(pgoff_t start, pgoff_t len) final;

 private:
  // It tracks which Page has been decommitted by kernel during |vmo_| unlocked.
  // When a bit is 0, a caller (i.e., Page::GetVmo()) clears the kUptodate flag of the
  // corresponding Page and fill the Page with data read from disk.
  std::vector<bool> page_bitmap_;
};

// It provides vmo service to Filecache of a vnode.
// When the cache is backed with discardable VMO, it divides the range of a vnode into a fixed size
// of vmo nodes and keeps them in |vmo_tree_|. From this way, it covers the full range of a vnode
// and saves va mapping resource. A vmo node represents a range between VmoMapping::index_ and
// VmoMapping::index_ + VmoMapping::size_in_blocks within the range of a vnode. The size of a vmo
// node is set to VmoManager::node_size_in_blocks_, and the va mapping of a vmo node keeps as long
// as the vmo node is kept in |vmo_tree_|.
// When the cache is backed with paged VMO, it does not use the tree and manages its content and vmo
// sizes.
class VmoManager {
 public:
  VmoManager() = delete;
  VmoManager(VmoMode mode, size_t content_size, size_t node_size, zx::vmo vmo = {});
  VmoManager(const VmoManager &) = delete;
  VmoManager &operator=(const VmoManager &) = delete;
  VmoManager(const VmoManager &&) = delete;
  VmoManager &operator=(const VmoManager &&) = delete;
  ~VmoManager() { Reset(true); }

  zx::result<bool> CreateAndLockVmo(pgoff_t index, void **out = nullptr) __TA_EXCLUDES(mutex_);
  zx_status_t UnlockVmo(pgoff_t index) __TA_EXCLUDES(mutex_);
  void ZeroBlocks(fs::PagedVfs &vfs, pgoff_t start, pgoff_t end) __TA_EXCLUDES(mutex_);
  zx::result<> WritebackBegin(fs::PagedVfs &vfs, const size_t start = 0,
                              const size_t end = kMaxVmoSize) __TA_EXCLUDES(mutex_);
  zx_status_t WritebackEnd(fs::PagedVfs &vfs, const size_t start = 0,
                           const size_t end = kMaxVmoSize) __TA_EXCLUDES(mutex_);
  zx::result<> DirtyPages(fs::PagedVfs &vfs, const size_t start = 0, const size_t end = kMaxVmoSize)
      __TA_EXCLUDES(mutex_);
  void AllowEviction(fs::PagedVfs &vfs, const size_t start = 0, const size_t end = kMaxVmoSize)
      __TA_EXCLUDES(mutex_);

  zx_status_t Write(const void *data, uint64_t offset, size_t len) __TA_EXCLUDES(mutex_);
  zx_status_t Read(void *data, uint64_t offset, size_t len) __TA_EXCLUDES(mutex_);

  void UpdateSizeUnsafe() __TA_REQUIRES(mutex_);
  void SetContentSize(const size_t nbytes) __TA_EXCLUDES(mutex_);
  uint64_t GetContentSize(bool update_checkpointed_size = false, bool round_up = false)
      __TA_EXCLUDES(mutex_);
  uint64_t GetContentSizeUnsafe(bool round_up) __TA_REQUIRES_SHARED(mutex_);
  bool SizeIsChanged() __TA_EXCLUDES(mutex_);

  void Reset(bool shutdown = false) __TA_EXCLUDES(mutex_);
  bool IsPaged() const { return mode_ == VmoMode::kPaged; }

 private:
  zx::result<> DirtyPagesUnsafe(fs::PagedVfs &vfs, const size_t start = 0,
                                const size_t end = kMaxVmoSize) __TA_REQUIRES_SHARED(mutex_);
  zx::result<> WritebackBeginUnsafe(fs::PagedVfs &vfs, const size_t start = 0,
                                    const size_t end = kMaxVmoSize) __TA_REQUIRES_SHARED(mutex_);
  zx_status_t WritebackEndUnsafe(fs::PagedVfs &vfs, const size_t start = 0,
                                 const size_t end = kMaxVmoSize) __TA_REQUIRES_SHARED(mutex_);

  pgoff_t GetOffsetInVmoNode(pgoff_t page_index) const;
  pgoff_t GetVmoNodeKey(pgoff_t page_index) const;

  using VmoTreeTraits = fbl::DefaultKeyedObjectTraits<pgoff_t, VmoMapping>;
  using VmoTree = fbl::WAVLTree<pgoff_t, std::unique_ptr<VmoMapping>, VmoTreeTraits>;
  zx::result<VmoMapping *> FindVmoNodeUnsafe(pgoff_t index) __TA_REQUIRES_SHARED(mutex_);
  zx::result<VmoMapping *> GetVmoNodeUnsafe(pgoff_t index) __TA_REQUIRES(mutex_);

  std::shared_mutex mutex_;
  VmoTree vmo_tree_ __TA_GUARDED(mutex_);
  const VmoMode mode_;

  size_t size_in_blocks_ __TA_GUARDED(mutex_) = 0;
  uint64_t content_size_ __TA_GUARDED(mutex_) = 0;
  uint64_t checkpointed_size_ __TA_GUARDED(mutex_) = 0;
  const size_t node_size_in_blocks_ = 0;
  const size_t node_size_ = 0;
  // a copy of paged vmo
  zx::vmo vmo_ __TA_GUARDED(mutex_);
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_VMO_MANAGER_H_
