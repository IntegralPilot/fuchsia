// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/bcache.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/node_page.h"
#include "src/storage/f2fs/superblock_info.h"
#include "src/storage/f2fs/vmo_manager.h"
#include "src/storage/f2fs/vnode.h"
#include "src/storage/f2fs/writeback.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {

Page::Page(FileCache *file_cache, pgoff_t index) : file_cache_(file_cache), index_(index) {}

VnodeF2fs &Page::GetVnode() const { return file_cache_->GetVnode(); }

VmoManager &Page::GetVmoManager() const { return file_cache_->GetVmoManager(); }

FileCache &Page::GetFileCache() const { return *file_cache_; }

void Page::RecyclePage() {
  // Since a page is evicted only when it has any references or is a weak pointer.
  // InTreeContainer() can be called without lock.
  if (InTreeContainer()) {
    ZX_ASSERT(VmoOpUnlock() == ZX_OK);
    file_cache_->Downgrade(this);
  } else {
    delete this;
  }
}

F2fs *Page::fs() const { return file_cache_->fs(); }

bool LockedPage::SetDirty() {
  if (page_) {
    return page_->SetDirty();
  }
  return true;
}

bool Page::SetDirty() {
  SetUptodate();
  if (!flags_[static_cast<uint8_t>(PageFlag::kPageDirty)].test_and_set(std::memory_order_acquire)) {
    VnodeF2fs &vnode = GetVnode();
    SuperblockInfo &superblock_info = fs()->GetSuperblockInfo();
    vnode.SetDirty();
    vnode.IncreaseDirtyPageCount();
    if (vnode.IsNode()) {
      superblock_info.IncreasePageCount(CountType::kDirtyNodes);
    } else if (vnode.IsDir()) {
      superblock_info.IncreasePageCount(CountType::kDirtyDents);
      superblock_info.IncreaseDirtyDir();
    } else if (vnode.IsMeta()) {
      superblock_info.IncreasePageCount(CountType::kDirtyMeta);
      superblock_info.SetDirty();
    } else {
      superblock_info.IncreasePageCount(CountType::kDirtyData);
    }
    return false;
  }
  return true;
}

bool LockedPage::ClearDirtyForIo() {
  if (page_ && page_->IsDirty()) {
    VnodeF2fs &vnode = page_->GetVnode();
    vnode.DecreaseDirtyPageCount();
    page_->ClearFlag(PageFlag::kPageDirty);
    SuperblockInfo &superblock_info = page_->fs()->GetSuperblockInfo();
    if (vnode.IsNode()) {
      superblock_info.DecreasePageCount(CountType::kDirtyNodes);
    } else if (vnode.IsDir()) {
      superblock_info.DecreasePageCount(CountType::kDirtyDents);
      superblock_info.DecreaseDirtyDir();
    } else if (vnode.IsMeta()) {
      superblock_info.DecreasePageCount(CountType::kDirtyMeta);
    } else {
      superblock_info.DecreasePageCount(CountType::kDirtyData);
    }
    return true;
  }
  return false;
}

zx_status_t Page::GetVmo() {
  zx::result committed = VmoOpLock();
  ZX_ASSERT(committed.is_ok());
  if (!*committed) {
    ZX_DEBUG_ASSERT(!IsDirty());
    ZX_DEBUG_ASSERT(!IsWriteback());
    ClearUptodate();
  }
  return committed.status_value();
}

void LockedPage::Invalidate() {
  if (page_) {
    ClearDirtyForIo();
    page_->block_addr_ = kNullAddr;
    page_->ClearColdData();
    page_->ClearUptodate();
  }
}

bool Page::IsUptodate() const {
  if (!TestFlag(PageFlag::kPageUptodate)) {
    return false;
  }
  return !GetVmoManager().IsPaged() || IsDirty() || IsWriteback();
}

bool Page::SetUptodate() { return SetFlag(PageFlag::kPageUptodate); }

void Page::ClearUptodate() { ClearFlag(PageFlag::kPageUptodate); }

void Page::WaitOnWriteback() { GetFileCache().WaitOnWriteback(*this); }

void LockedPage::WaitOnWriteback() {
  if (!page_) {
    return;
  }
  page_->WaitOnWriteback();
}

bool LockedPage::SetWriteback(block_t addr) {
  bool ret = !page_ || page_->SetFlag(PageFlag::kPageWriteback);
  if (!ret) {
    page_->fs()->GetSuperblockInfo().IncreasePageCount(CountType::kWriteback);
    page_->block_addr_ = addr;
    size_t offset = page_->GetKey() * kBlockSize;
    ZX_ASSERT(page_->GetVmoManager()
                  .WritebackBegin(*page_->fs()->vfs(), offset, offset + kBlockSize)
                  .is_ok());
  }
  return ret;
}

void Page::ClearWriteback() {
  if (IsWriteback()) {
    fs()->GetSuperblockInfo().DecreasePageCount(CountType::kWriteback);
    size_t offset = GetKey() * kBlockSize;
    ZX_ASSERT(GetVmoManager().WritebackEnd(*fs()->vfs(), offset, offset + kBlockSize) == ZX_OK);
    ClearFlag(PageFlag::kPageWriteback);
  }
}

bool Page::SetCommit() { return SetFlag(PageFlag::kPageCommit); }

void Page::ClearCommit() { ClearFlag(PageFlag::kPageCommit); }

bool Page::SetSync() { return SetFlag(PageFlag::kPageSync); }

void Page::ClearSync() { ClearFlag(PageFlag::kPageSync); }

void Page::SetColdData() {
  ZX_DEBUG_ASSERT(!IsWriteback());
  SetFlag(PageFlag::kPageColdData);
}

bool Page::ClearColdData() {
  if (IsColdData()) {
    ClearFlag(PageFlag::kPageColdData);
    return true;
  }
  return false;
}

zx_status_t Page::VmoOpUnlock(bool evict) {
  if (evict && IsVmoLocked()) {
    ClearFlag(PageFlag::kPageVmoLocked);
    return GetVmoManager().UnlockVmo(index_);
  }
  return ZX_OK;
}

zx::result<bool> Page::VmoOpLock() {
  ZX_DEBUG_ASSERT(InTreeContainer());
  if (!SetFlag(PageFlag::kPageVmoLocked)) {
    return GetVmoManager().CreateAndLockVmo(index_, addr_ ? nullptr : &addr_);
  }
  return zx::ok(true);
}

zx_status_t Page::Read(void *data, uint64_t offset, size_t len) {
  if (unlikely(offset + len > Size())) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  if (uint8_t *addr = GetAddress<uint8_t>(); addr) {
    std::memcpy(data, &addr[offset], len);
    return ZX_OK;
  }
  return GetVmoManager().Read(data, index_ * Size() + offset, len);
}

zx_status_t Page::Write(const void *data, uint64_t offset, size_t len) {
  if (unlikely(offset + len > Size())) {
    return ZX_ERR_OUT_OF_RANGE;
  }
  if (uint8_t *addr = GetAddress<uint8_t>(); addr) {
    std::memcpy(&addr[offset], data, len);
    return ZX_OK;
  }
  return GetVmoManager().Write(data, index_ * Size() + offset, len);
}

void LockedPage::Zero(size_t start, size_t end) const {
  if (start < end && end <= Page::Size() && page_) {
    page_->Write(kZeroBuffer_.data(), start, end - start);
  }
}

zx::result<> LockedPage::SetVmoDirty() {
  if (!page_ || !page_->GetVmoManager().IsPaged()) {
    return zx::error(ZX_ERR_NOT_SUPPORTED);
  }
  WaitOnWriteback();
  if (page_->IsDirty()) {
    return zx::ok();
  }
  size_t start_offset = safemath::CheckMul(page_->GetKey(), kBlockSize).ValueOrDie();
  if (zx::result dirty = page_->GetVmoManager().DirtyPages(*page_->fs()->vfs(), start_offset,
                                                           start_offset + Page::Size());
      dirty.is_error()) {
    ZX_DEBUG_ASSERT(dirty.error_value() == ZX_ERR_NOT_FOUND);
    return dirty.take_error();
  }
  return zx::ok();
}

FileCache::FileCache(VnodeF2fs *vnode, VmoManager *vmo_manager)
    : vnode_(vnode), vmo_manager_(vmo_manager) {}

FileCache::~FileCache() {
  std::vector<fbl::RefPtr<Page>> pages = FindPagesUnsafe();
  for (auto &page : pages) {
    EvictUnsafe(page.get());
  }
  vmo_manager_->Reset();
}

void FileCache::Downgrade(Page *raw_page) {
  fs::SharedLock tree_lock(tree_lock_);
  raw_page->ResurrectRef();
  fbl::RefPtr<Page> page = fbl::ImportFromRawPtr(raw_page);
  // Leak it to keep alive in FileCache.
  [[maybe_unused]] auto leak = fbl::ExportToRawPtr(&page);
  raw_page->ClearActive();
  recycle_cvar_.notify_all();
}

F2fs *FileCache::fs() const { return GetVnode().fs(); }

zx::result<std::vector<LockedPage>> FileCache::GetLockedPagesUnsafe(const pgoff_t start,
                                                                    const pgoff_t end) {
  std::vector<LockedPage> locked_pages(end - start);
  auto exist_pages = FindLockedPagesUnsafe(start, end);
  uint32_t exist_pages_index = 0, count = 0;
  for (pgoff_t index = start; index < end; ++index, ++count) {
    if (exist_pages_index < exist_pages.size() &&
        exist_pages[exist_pages_index]->GetKey() == index) {
      locked_pages[count] = std::move(exist_pages[exist_pages_index]);
      ++exist_pages_index;
    } else {
      locked_pages[count] = LockedPage(AddNewPageUnsafe(index));
      if (auto ret = locked_pages[count]->GetVmo(); ret != ZX_OK) {
        return zx::error(ret);
      }
    }
  }

  return zx::ok(std::move(locked_pages));
}

zx::result<std::vector<fbl::RefPtr<Page>>> FileCache::GetPagesUnsafe(const pgoff_t start,
                                                                     const pgoff_t end) {
  std::vector<fbl::RefPtr<Page>> pages(end - start);
  auto exist_pages = FindPagesUnsafe(start, end);
  uint32_t exist_pages_index = 0, count = 0;
  for (pgoff_t index = start; index < end; ++index, ++count) {
    if (exist_pages_index < exist_pages.size() &&
        exist_pages[exist_pages_index]->GetKey() == index) {
      pages[count] = std::move(exist_pages[exist_pages_index]);
      ++exist_pages_index;
    } else {
      pages[count] = AddNewPageUnsafe(index);
      if (auto ret = pages[count]->GetVmo(); ret != ZX_OK) {
        return zx::error(ret);
      }
    }
  }

  return zx::ok(std::move(pages));
}

zx::result<std::vector<LockedPage>> FileCache::GetLockedPages(const pgoff_t start,
                                                              const pgoff_t end) {
  std::lock_guard tree_lock(tree_lock_);
  return GetLockedPagesUnsafe(start, end);
}

std::vector<LockedPage> FileCache::FindLockedPages(const pgoff_t start, const pgoff_t end) {
  std::lock_guard tree_lock(tree_lock_);
  return FindLockedPagesUnsafe(start, end);
}

zx::result<std::vector<fbl::RefPtr<Page>>> FileCache::GetPages(const pgoff_t start,
                                                               const pgoff_t end) {
  std::lock_guard tree_lock(tree_lock_);
  return GetPagesUnsafe(start, end);
}

fbl::RefPtr<Page> FileCache::AddNewPageUnsafe(const pgoff_t index) {
  fbl::RefPtr<Page> page;
  if (GetVnode().IsNode()) {
    page = fbl::MakeRefCounted<NodePage>(this, index);
  } else {
    page = fbl::MakeRefCounted<Page>(this, index);
  }
  page_tree_.insert(page.get());
  page->SetActive();
  return page;
}

zx_status_t FileCache::GetLockedPage(const pgoff_t index, LockedPage *out) {
  std::lock_guard tree_lock(tree_lock_);
  zx::result locked_page = GetLockedPagesUnsafe(index, index + 1);
  if (locked_page.is_error()) {
    return locked_page.error_value();
  }
  *out = std::move(locked_page->front());
  return ZX_OK;
}

zx_status_t FileCache::FindPage(const pgoff_t index, fbl::RefPtr<Page> *out) {
  std::lock_guard tree_lock(tree_lock_);
  auto pages = FindPagesUnsafe(index, index + 1);
  if (pages.empty()) {
    return ZX_ERR_NOT_FOUND;
  }
  *out = std::move(pages.front());
  return ZX_OK;
}

zx::result<LockedPage> FileCache::GetLockedPageFromRawUnsafe(Page *raw_page) {
  auto page = fbl::MakeRefPtrUpgradeFromRaw(raw_page, tree_lock_);
  if (page == nullptr) {
    recycle_cvar_.wait(tree_lock_);
    // It is being recycled. It is unavailable now.
    return zx::error(ZX_ERR_UNAVAILABLE);
  }
  // Try to make LockedPage from |page|.
  zx::result locked_page = GetLockedPageUnsafe(std::move(page));
  if (locked_page.is_error()) {
    return locked_page.take_error();
  }
  return zx::ok(*std::move(locked_page));
}

zx::result<LockedPage> FileCache::GetLockedPageUnsafe(fbl::RefPtr<Page> page) {
  LockedPage locked_page = LockedPage(page, std::try_to_lock);
  if (!locked_page) {
    tree_lock_.unlock();
    // If |page| is already locked, it releases |tree_lock_|, which allows FileCache to serve
    // other requests while waiting for the locked page.
    locked_page = LockedPage(std::move(page));
    tree_lock_.lock();
  }
  // |page| can be evicted while it releases |tree_lock_| while waiting for the page lock.
  if (!locked_page->InTreeContainer()) {
    return zx::error(ZX_ERR_NOT_FOUND);
  }
  return zx::ok(std::move(locked_page));
}

zx_status_t FileCache::EvictUnsafe(Page *page) {
  if (!page->InTreeContainer()) {
    return ZX_ERR_NOT_FOUND;
  }
  // Before eviction, check if it requires VMO_OP_UNLOCK
  // since Page::RecyclePage() tries VMO_OP_UNLOCK only when |page| keeps in FileCache.
  ZX_ASSERT(page->VmoOpUnlock(true) == ZX_OK);
  page_tree_.erase(*page);
  return ZX_OK;
}

std::vector<fbl::RefPtr<Page>> FileCache::FindPagesUnsafe(pgoff_t start, pgoff_t end) {
  std::vector<fbl::RefPtr<Page>> pages;
  auto current = page_tree_.lower_bound(start);
  while (current != page_tree_.end() && current->GetKey() < end) {
    if (!current->IsActive()) {
      auto page = fbl::ImportFromRawPtr(current.CopyPointer());
      page->SetActive();
      pages.push_back(std::move(page));
    } else {
      auto prev_key = current->GetKey();
      auto page = fbl::MakeRefPtrUpgradeFromRaw(current.CopyPointer(), tree_lock_);
      if (page == nullptr) {
        // It is being recycled. It is unavailable now.
        recycle_cvar_.wait(tree_lock_);
        current = page_tree_.lower_bound(prev_key);
        continue;
      }
      pages.push_back(std::move(page));
    }
    ++current;
  }
  return pages;
}

std::vector<LockedPage> FileCache::FindLockedPagesUnsafe(pgoff_t start, pgoff_t end) {
  std::vector<LockedPage> pages;
  auto current = page_tree_.lower_bound(start);
  while (current != page_tree_.end() && current->GetKey() < end) {
    if (!current->IsActive()) {
      LockedPage locked_page(fbl::ImportFromRawPtr(current.CopyPointer()));
      locked_page->SetActive();
      pages.push_back(std::move(locked_page));
    } else {
      auto prev_key = current->GetKey();
      zx::result locked_page = GetLockedPageFromRawUnsafe(current.CopyPointer());
      if (locked_page.is_error()) {
        current = page_tree_.lower_bound(prev_key);
        continue;
      }
      pages.push_back(std::move(*locked_page));
    }
    ++current;
  }
  return pages;
}

std::vector<LockedPage> FileCache::InvalidatePages(pgoff_t start, pgoff_t end, bool zero) {
  std::vector<LockedPage> pages = FindLockedPages(start, end);
  // Invalidate pages after waiting for their writeback.
  for (auto &page : pages) {
    WaitOnWriteback(*page);
    page.Invalidate();
  }
  if (zero) {
    // Make sure that all pages in the range are zeroed.
    vmo_manager_->ZeroBlocks(*vnode_->fs()->vfs(), start, end);
  }
  std::lock_guard tree_lock(tree_lock_);
  for (auto &page : pages) {
    EvictUnsafe(page.get());
  }
  return pages;
}

void FileCache::ClearDirtyPages() {
  std::vector<LockedPage> pages;
  {
    std::lock_guard tree_lock(tree_lock_);
    pages = FindLockedPagesUnsafe();
    // Let kernel evict the pages if |this| is running on paged vmo.
    vmo_manager_->AllowEviction(*vnode_->fs()->vfs());
  }
  // Clear the dirty flag of all Pages.
  for (auto &page : pages) {
    page.ClearDirtyForIo();
  }
}

void FileCache::Reset() {
  std::vector<LockedPage> pages = InvalidatePages(0, kPgOffMax, false);
  vmo_manager_->Reset();
}

size_t FileCache::GetReadHint(pgoff_t start, size_t size, size_t max_size,
                              bool high_memory_pressure) {
  // Consider using dirty or writeback pages on high memory pressure.
  if (high_memory_pressure) {
    return size;
  }
  fs::SharedLock lock(tree_lock_);
  pgoff_t end = start + max_size;
  auto last_page = page_tree_.lower_bound(start + size);
  for (; last_page != page_tree_.end() && last_page->GetKey() < end; ++last_page) {
    // Do not readahead recently used pages
    if (last_page->GetKey() >= start + size) {
      break;
    }
  }
  if (last_page != page_tree_.end()) {
    end = std::min(end, last_page->GetKey());
  }
  return end - start;
}

std::vector<fbl::RefPtr<Page>> FileCache::FindDirtyPages(const WritebackOperation &operation) {
  std::vector<fbl::RefPtr<Page>> pages;
  pgoff_t nwritten = 0;

  std::lock_guard tree_lock(tree_lock_);
  auto current = page_tree_.lower_bound(operation.start);
  // Get Pages from |operation.start| to |operation.end|.
  while (nwritten < operation.to_write && current != page_tree_.end() &&
         current->GetKey() < operation.end) {
    fbl::RefPtr<Page> page;
    auto raw_page = current.CopyPointer();
    if (raw_page->IsActive()) {
      if (raw_page->IsDirty()) {
        auto prev_key = raw_page->GetKey();
        page = fbl::MakeRefPtrUpgradeFromRaw(raw_page, tree_lock_);
        if (page == nullptr) {
          recycle_cvar_.wait(tree_lock_);
          // It is being recycled. It is unavailable now.
          current = page_tree_.lower_bound(prev_key);
          continue;
        }
      }
      ++current;
    } else {
      ++current;
      ZX_DEBUG_ASSERT(!raw_page->IsWriteback());
      if (raw_page->IsDirty()) {
        ZX_DEBUG_ASSERT(raw_page->IsLastReference());
        page = fbl::ImportFromRawPtr(raw_page);
      } else if (operation.bReclaim || GetVmoManager().IsPaged()) {
        // Evict clean Pages if it is invoked due to memory reclaim or its vnode is pager backed.
        fbl::RefPtr<Page> evicted = fbl::ImportFromRawPtr(raw_page);
        EvictUnsafe(raw_page);
      }
    }
    if (!page) {
      continue;
    }
    if (!operation.if_page || operation.if_page(page) == ZX_OK) {
      page->SetActive();
      pages.push_back(std::move(page));
      ++nwritten;
    } else {
      // It prevents |page| from entering RecyclePage() and
      // keeps |page| alive in FileCache.
      [[maybe_unused]] auto leak = fbl::ExportToRawPtr(&page);
    }
  }
  return pages;
}

void FileCache::EvictCleanPages() {
  std::lock_guard tree_lock(tree_lock_);
  auto current = page_tree_.lower_bound(0);
  while (current != page_tree_.end()) {
    auto raw_page = current.CopyPointer();
    ++current;
    if (!raw_page->IsActive() && !raw_page->IsDirty()) {
      ZX_DEBUG_ASSERT(!raw_page->IsWriteback());
      fbl::RefPtr<Page> evicted = fbl::ImportFromRawPtr(raw_page);
      EvictUnsafe(raw_page);
    }
  }
}

void FileCache::WaitOnWriteback(Page &page) {
  fs::SharedLock lock(flag_lock_);
  if (page.IsWriteback()) {
    fs()->GetWriter().ScheduleWriteBlocks();
  }
  flag_cvar_.wait(lock, [&]() { return !page.IsWriteback(); });
}

void FileCache::NotifyWriteback(PageList pages) {
  std::lock_guard lock(flag_lock_);
  while (!pages.is_empty()) {
    auto page = pages.pop_front();
    page->ClearWriteback();
  }
  flag_cvar_.notify_all();
}

size_t FileCache::GetSize() {
  fs::SharedLock lock(tree_lock_);
  return page_tree_.size();
}

}  // namespace f2fs
