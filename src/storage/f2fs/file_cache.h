// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STORAGE_F2FS_FILE_CACHE_H_
#define SRC_STORAGE_F2FS_FILE_CACHE_H_

#include <condition_variable>
#include <utility>

#include <fbl/intrusive_double_list.h>
#include <fbl/intrusive_wavl_tree.h>
#include <safemath/checked_math.h>

#include "src/storage/f2fs/common.h"

namespace f2fs {

class F2fs;
class Page;
class VnodeF2fs;
class FileCache;
class LockedPage;
class VmoManager;

constexpr pgoff_t kPgOffMax = std::numeric_limits<pgoff_t>::max() / kBlockSize;
using PageCallback = fit::function<zx_status_t(fbl::RefPtr<Page>)>;
using PageTaggingCallback = fit::function<zx_status_t(fbl::RefPtr<Page>, bool is_last_page)>;
using PageList = fbl::SizedDoublyLinkedList<fbl::RefPtr<Page>>;

enum class PageFlag {
  kPageUptodate =
      0,       // It is uptodate. No need to read blocks from disk if the backing page is present.
  kPageDirty,  // It needs to be written out.
  kPageWriteback,  // It is under writeback.
  kPageVmoLocked,  // Its vmo is locked. It should not be subject to reclaim.
  kPageActive,     // Its reference count > 0
  kPageColdData,   // It is subject to gc.
  kPageCommit,     // It is logged with pre-flush and flush.
  kPageSync,       // It is logged with flush.
  kPageFlagSize,
};

// It defines a writeback operation.
struct WritebackOperation {
  pgoff_t start = 0;  // All dirty Pages within the range of [start, end) are subject to writeback.
  pgoff_t end = kPgOffMax;
  pgoff_t to_write = kPgOffMax;  // The number of dirty Pages to be written.
  bool bSync = false;            // If true, FileCache::Writeback() waits for writeback Pages to be
                                 // written to disk.
  bool bReclaim =
      false;  // If true, it releases inactive Pages while traversing FileCache::page_tree_.
  VnodeCallback if_vnode = nullptr;  // If set, it determines which vnodes are subject to writeback.
  PageCallback if_page = nullptr;    // If set, it determines which Pages are subject to writeback.
  PageTaggingCallback page_cb =
      nullptr;  // If set, it can touch every page subject to writeback before disk I/O. It is used
                // to set flags or update footer for fsync() or checkpoint().
};

template <typename T, bool EnableAdoptionValidator = ZX_DEBUG_ASSERT_IMPLEMENTED>
class PageRefCounted : public fs::VnodeRefCounted<T> {
 public:
  PageRefCounted(const Page &) = delete;
  PageRefCounted &operator=(const PageRefCounted &) = delete;
  PageRefCounted(const PageRefCounted &&) = delete;
  PageRefCounted &operator=(const PageRefCounted &&) = delete;
  using ::fbl::internal::RefCountedBase<EnableAdoptionValidator>::IsLastReference;

 protected:
  constexpr PageRefCounted() = default;
  ~PageRefCounted() = default;
};

class Page : public PageRefCounted<Page>,
             public fbl::Recyclable<Page>,
             public fbl::WAVLTreeContainable<Page *>,
             public fbl::DoublyLinkedListable<fbl::RefPtr<Page>> {
 public:
  Page() = delete;
  Page(FileCache *file_cache, pgoff_t index);
  Page(const Page &) = delete;
  Page &operator=(const Page &) = delete;
  Page(const Page &&) = delete;
  Page &operator=(const Page &&) = delete;
  virtual ~Page() = default;

  void fbl_recycle() { RecyclePage(); }

  pgoff_t GetKey() const { return index_; }
  pgoff_t GetIndex() const { return GetKey(); }
  VnodeF2fs &GetVnode() const;
  FileCache &GetFileCache() const;
  VmoManager &GetVmoManager() const;
  // If it runs on a discardable VMO, this method ensures that a associated VMO keeps with its
  // mapping until |this| is evicted from FileCache. If it is backed on a pager's VMO, it does
  // nothing.
  zx_status_t GetVmo();
  zx_status_t VmoOpUnlock(bool evict = false);
  zx::result<bool> VmoOpLock();

  template <typename U = void>
  U *GetAddress() const {
    return static_cast<U *>(addr_);
  }

  bool IsUptodate() const;
  bool IsDirty() const { return TestFlag(PageFlag::kPageDirty); }
  bool IsWriteback() const { return TestFlag(PageFlag::kPageWriteback); }
  bool IsVmoLocked() const { return TestFlag(PageFlag::kPageVmoLocked); }
  bool IsActive() const { return TestFlag(PageFlag::kPageActive); }
  bool IsColdData() const { return TestFlag(PageFlag::kPageColdData); }
  bool IsCommit() const { return TestFlag(PageFlag::kPageCommit); }
  bool IsSync() const { return TestFlag(PageFlag::kPageSync); }

  // Each Setxxx() method atomically sets a flag and returns the previous value.
  // It is called when the first reference is made.
  bool SetActive() { return SetFlag(PageFlag::kPageActive); }
  // It is called after the last reference is destroyed in FileCache::Downgrade().
  void ClearActive() { ClearFlag(PageFlag::kPageActive); }

  void ClearWriteback();
  void WaitOnWriteback();

  bool SetUptodate();
  void ClearUptodate();

  bool SetCommit();
  void ClearCommit();

  bool SetSync();
  void ClearSync();

  void SetColdData();
  bool ClearColdData();

  bool SetDirty();

  static constexpr uint32_t Size() { return kBlockSize; }
  block_t GetBlockAddr() const {
    if (TestFlag(PageFlag::kPageWriteback)) {
      return block_addr_;
    }
    return kNullAddr;
  }

  // Check that |this| Page exists in FileCache.
  bool InTreeContainer() const { return fbl::WAVLTreeContainable<Page *>::InContainer(); }
  // Check that |this| Page exists in any PageList.
  bool InListContainer() const {
    return fbl::DoublyLinkedListable<fbl::RefPtr<Page>>::InContainer();
  }

  zx_status_t Read(void *data, uint64_t offset = 0, size_t len = Size());
  zx_status_t Write(const void *data, uint64_t offset = 0, size_t len = Size());

  F2fs *fs() const;

 protected:
  // It notifies VmoManager that there is no reference to |this|.
  void RecyclePage();

 private:
  zx_status_t Map();
  bool TestFlag(PageFlag flag) const {
    return flags_[static_cast<uint8_t>(flag)].test(std::memory_order_acquire);
  }
  void ClearFlag(PageFlag flag) {
    flags_[static_cast<uint8_t>(flag)].clear(std::memory_order_relaxed);
  }
  bool SetFlag(PageFlag flag) {
    return flags_[static_cast<uint8_t>(flag)].test_and_set(std::memory_order_acquire);
  }

  // It is used to track the status of a page by using PageFlag
  std::array<std::atomic_flag, static_cast<uint8_t>(PageFlag::kPageFlagSize)> flags_ = {
      ATOMIC_FLAG_INIT};
  // It indicates FileCache to which |this| belongs.
  FileCache *file_cache_ = nullptr;
  void *addr_ = nullptr;
  // It is used as the key of |this| in a lookup table (i.e., FileCache::page_tree_).
  // It indicates different information according to the type of FileCache::vnode_ such as file,
  // node, and meta vnodes. For file vnodes, it has file offset. For node vnodes, it indicates the
  // node id. For meta vnode, it points to the block address to which the metadata is written.
  const pgoff_t index_;
  block_t block_addr_ = kNullAddr;
  friend class LockedPage;
  std::mutex mutex_;
};

// LockedPage is a wrapper class for f2fs::Page lock management.
// When LockedPage holds "fbl::RefPtr<Page> page" and the page is not nullptr, it guarantees that
// the page is locked.
//
// The syntax looks something like...
// fbl::RefPtr<Page> unlocked_page;
// {
//   LockedPage locked_page(unlocked_page);
//   do something requiring page lock...
// }
//
// When Page is used as a function parameter, you should use `LockedPage&` type for locked page.
class LockedPage final {
 public:
  LockedPage() : page_(nullptr) {}

  LockedPage(const LockedPage &) = delete;
  LockedPage &operator=(const LockedPage &) = delete;

  LockedPage(LockedPage &&p) noexcept {
    page_ = std::move(p.page_);
    lock_ = std::move(p.lock_);
    p.page_ = nullptr;
  }

  LockedPage &operator=(LockedPage &&p) noexcept {
    reset();
    page_ = std::move(p.page_);
    lock_ = std::move(p.lock_);
    p.page_ = nullptr;
    return *this;
  }

  // If it fails to acquire |page->mutex_|, it doesn't own |page|, and LockedPage::bool returns
  // false.
  explicit LockedPage(fbl::RefPtr<Page> page, std::try_to_lock_t t) {
    lock_ = std::unique_lock<std::mutex>(page->mutex_, t);
    if (lock_.owns_lock()) {
      page_ = std::move(page);
    }
  }

  explicit LockedPage(fbl::RefPtr<Page> page) {
    page_ = std::move(page);
    lock_ = std::unique_lock<std::mutex>(page_->mutex_);
  }

  ~LockedPage() { reset(); }

  void reset() {
    if (page_ != nullptr) {
      lock_.unlock();
      lock_.release();
      page_.reset();
    }
  }

  // It works only for paged vmo. When f2fs needs to update the contents of a page without
  // triggering Vnode::VmoDirty(), it calls this method before the modification. If its page is
  // present, kernel pre-dirties the page. If not, it returns zx::error(ZX_ERR_NOT_FOUND), and the
  // caller have to supply a vmo.
  zx::result<> SetVmoDirty();

  bool ClearDirtyForIo();
  bool SetDirty();
  void Zero(size_t start = 0, size_t end = Page::Size()) const;
  // It invalidates |this| for truncate and punch-a-hole operations. A caller should call
  // WaitOnWriteback() before it.
  void Invalidate();
  // It waits for the writeback flag of |page_| to be cleared. So, it should not be called with
  // FileCache::page_lock_ acquired. It acquires LockedPage::lock_ during waiting.
  // TODO(b/293975446): Consider releasing |lock_| during waiting.
  void WaitOnWriteback();
  bool SetWriteback(block_t addr = kNullAddr);

  // It returns the ownership of unlocked |page_|.
  fbl::RefPtr<Page> release() {
    if (page_) {
      lock_.unlock();
      lock_.release();
    }
    return fbl::RefPtr<Page>(std::move(page_));
  }

  // CopyRefPtr() returns a RefPtr of locked |page_|.
  fbl::RefPtr<Page> CopyRefPtr() const { return page_; }

  template <typename T = Page>
  T &GetPage() const {
    return static_cast<T &>(*page_);
  }

  Page *get() const { return page_.get(); }
  Page &operator*() const { return *page_; }
  Page *operator->() const { return page_.get(); }
  explicit operator bool() const { return page_ != nullptr; }

  // Comparison against nullptr operators (of the form, myptr == nullptr).
  bool operator==(decltype(nullptr)) const { return (page_ == nullptr); }
  bool operator!=(decltype(nullptr)) const { return (page_ != nullptr); }

 private:
  static constexpr std::array<uint8_t, Page::Size()> kZeroBuffer_ = {0};
  fbl::RefPtr<Page> page_ = nullptr;
  std::unique_lock<std::mutex> lock_;
};

class FileCache {
 public:
  FileCache(VnodeF2fs *vnode, VmoManager *vmo_manager);
  FileCache() = delete;
  FileCache(const FileCache &) = delete;
  FileCache &operator=(const FileCache &) = delete;
  FileCache(const FileCache &&) = delete;
  FileCache &operator=(const FileCache &&) = delete;
  ~FileCache();

  // It returns a locked Page corresponding to |index| from |page_tree_|.
  // If there is no Page, it creates and returns a locked Page.
  zx_status_t GetLockedPage(pgoff_t index, LockedPage *out) __TA_EXCLUDES(tree_lock_);
  // It returns locked Pages corresponding to [start - end) from |page_tree_|.
  zx::result<std::vector<LockedPage>> GetLockedPages(pgoff_t start, pgoff_t end)
      __TA_EXCLUDES(tree_lock_);
  // It does the same thing as the above methods except that it returns unlocked Pages.
  zx::result<std::vector<fbl::RefPtr<Page>>> GetPages(const pgoff_t start, const pgoff_t end)
      __TA_EXCLUDES(tree_lock_);
  // It returns locked Pages corresponding to [start - end) from |page_tree_|.
  // If there is no corresponding Page, the returned page will be a null page.
  std::vector<LockedPage> FindLockedPages(pgoff_t start, pgoff_t end) __TA_EXCLUDES(tree_lock_);
  // It returns an unlocked Page corresponding to |index| from |page_tree|.
  // If it fails to find the Page in |page_tree_|, it returns ZX_ERR_NOT_FOUND.
  zx_status_t FindPage(pgoff_t index, fbl::RefPtr<Page> *out) __TA_EXCLUDES(tree_lock_);

  // It invalidates Pages within the range of |start| to |end| in |page_tree_|. If |zero| is set,
  // the data of the corresponding pages are zeored. Then, it evicts all Pages within the range and
  // returns them locked.
  std::vector<LockedPage> InvalidatePages(pgoff_t start = 0, pgoff_t end = kPgOffMax,
                                          bool zero = true) __TA_EXCLUDES(tree_lock_);
  // It invalidates all Pages from |page_tree_|.
  void Reset() __TA_EXCLUDES(tree_lock_);
  // Clear all dirty pages.
  void ClearDirtyPages() __TA_EXCLUDES(tree_lock_);

  VnodeF2fs &GetVnode() const { return *vnode_; }
  // Only Page::RecyclePage() is allowed to call it.
  void Downgrade(Page *raw_page) __TA_EXCLUDES(tree_lock_);
  // It returns a proper read size within the range of [size, size + max_size) according to the
  // status of recently used pages and memory pressure level.
  size_t GetReadHint(pgoff_t start, size_t size, size_t max_size, bool high_memory_pressure = false)
      __TA_EXCLUDES(tree_lock_);
  F2fs *fs() const;
  VmoManager &GetVmoManager() { return *vmo_manager_; }

  // It returns a set of dirty Pages that meet |operation|.
  std::vector<fbl::RefPtr<Page>> FindDirtyPages(const WritebackOperation &operation)
      __TA_EXCLUDES(tree_lock_);
  // It evicts every clean, inactive  page.
  void EvictCleanPages() __TA_EXCLUDES(tree_lock_);

  // It provides wait() and notify() for kPageWriteback flag of Page.
  void WaitOnWriteback(Page &page) __TA_EXCLUDES(flag_lock_, tree_lock_);
  void NotifyWriteback(PageList pages) __TA_EXCLUDES(flag_lock_, tree_lock_);

  size_t GetSize() __TA_EXCLUDES(tree_lock_);

 private:
  // Unless |page| is locked, it returns a locked |page|. If |page| is already locked,
  // it waits for |page| to be unlocked. While waiting, |tree_lock_| keeps unlocked to avoid
  // possible deadlock problems and to allow other page requests. When it gets the locked |page|, it
  // acquires |tree_lock_| again and returns the locked |page| if |page| still belongs to
  // |page_tree_|;
  zx::result<LockedPage> GetLockedPageUnsafe(fbl::RefPtr<Page> page) __TA_REQUIRES(tree_lock_);
  zx::result<LockedPage> GetLockedPageFromRawUnsafe(Page *raw_page) __TA_REQUIRES(tree_lock_);
  fbl::RefPtr<Page> AddNewPageUnsafe(pgoff_t index) __TA_REQUIRES(tree_lock_);
  zx_status_t EvictUnsafe(Page *page) __TA_REQUIRES(tree_lock_);
  // It returns all Pages from |page_tree_| within the range of |start| to |end|.
  // If there is no corresponding Page in page_tree_, the Page will not be included in the returned
  // vector. Therefore, returned vector's size could be smaller than |end - start|.
  std::vector<LockedPage> FindLockedPagesUnsafe(pgoff_t start = 0, pgoff_t end = kPgOffMax)
      __TA_REQUIRES(tree_lock_);
  std::vector<fbl::RefPtr<Page>> FindPagesUnsafe(pgoff_t start = 0, pgoff_t end = kPgOffMax)
      __TA_REQUIRES(tree_lock_);

  zx::result<std::vector<LockedPage>> GetLockedPagesUnsafe(pgoff_t start, pgoff_t end)
      __TA_REQUIRES(tree_lock_);
  zx::result<std::vector<fbl::RefPtr<Page>>> GetPagesUnsafe(pgoff_t start, pgoff_t end)
      __TA_REQUIRES(tree_lock_);

  using PageTreeTraits = fbl::DefaultKeyedObjectTraits<pgoff_t, Page>;
  using PageTree = fbl::WAVLTree<pgoff_t, Page *, PageTreeTraits>;

  // |tree_lock_| should not be acquired with |flag_lock_| to avoid deadlock.
  std::shared_mutex tree_lock_;

  std::condition_variable_any recycle_cvar_;

  // |flag_lock_| is used to protect writeback flags of Page.
  std::shared_mutex flag_lock_;

  std::condition_variable_any flag_cvar_;
  PageTree page_tree_ __TA_GUARDED(tree_lock_);
  VnodeF2fs *vnode_;
  VmoManager *vmo_manager_;
};

}  // namespace f2fs

#endif  // SRC_STORAGE_F2FS_FILE_CACHE_H_
