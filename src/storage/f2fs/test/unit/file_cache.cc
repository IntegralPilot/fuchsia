// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <numeric>

#include <gtest/gtest.h>
#include <safemath/checked_math.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/test/unit/unit_lib.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {
namespace {

using FileCacheTest = SingleFileTest;

TEST_F(FileCacheTest, WaitOnWriteback) {
  char buf[kBlockSize];
  FileTester::AppendToFile(&vnode<File>(), buf, kBlockSize);

  vnode<File>().Writeback(false, true);
  LockedPage page = GetLockedPage(0);
  ASSERT_TRUE(page->IsWriteback());
  page.WaitOnWriteback();
  ASSERT_FALSE(page->IsWriteback());
}

TEST_F(FileCacheTest, Map) {
  constexpr size_t kNum = 0xAA00AA00AA00AA00;
  size_t *base = nullptr;
  {
    // Get discardable backed page
    LockedPage page;
    ASSERT_EQ(fs_->GetMetaPage(0, &page), ZX_OK);
    // Set dirty to keep |page| and its mapping in FileCache.
    page.SetDirty();
    base = page->GetAddress<size_t>();
    *base = kNum;
  }
  // Even after LockedPage is destructed, the mapping should be maintained.
  ASSERT_EQ(*base, kNum);
}

TEST_F(FileCacheTest, EvictActivePages) {
  char buf[kBlockSize];
  auto &file = vnode<File>();

  // Make two dirty Pages.
  FileTester::AppendToFile(&file, buf, kBlockSize);
  FileTester::AppendToFile(&file, buf, kBlockSize);

  constexpr uint64_t kPageNum = 2;
  for (auto i = 0ULL; i < kPageNum; ++i) {
    LockedPage page = GetLockedPage(i);
    ASSERT_TRUE(page->IsDirty());
    ASSERT_FALSE(page->IsWriteback());
  }

  // Clean every dirty Pages
  ASSERT_EQ(file.Writeback(true, false), kPageNum);

  for (auto i = 0ULL; i < kPageNum; ++i) {
    LockedPage page = GetLockedPage(i);
    ASSERT_FALSE(page->IsDirty());
  }

  zx::nanosleep(zx::deadline_after(zx::sec(1)));
  // Evict every inactive Page.
  ASSERT_EQ(file.Writeback(false, true), 0ULL);

  for (auto i = 0ULL; i < kPageNum; ++i) {
    fbl::RefPtr<Page> page;
    ASSERT_EQ(file.FindPage(i, &page), ZX_ERR_NOT_FOUND);
  }
}

TEST_F(FileCacheTest, WritebackOperation) {
  auto &file = vnode<File>();
  char buf[kBlockSize];
  pgoff_t key;
  WritebackOperation op = {.start = 0,
                           .end = 2,
                           .to_write = 2,
                           .bSync = true,
                           .bReclaim = false,
                           .if_page = [&key](const fbl::RefPtr<Page> &page) {
                             if (page->GetKey() <= key) {
                               return ZX_OK;
                             }
                             return ZX_ERR_NEXT;
                           }};

  // |vn| should not have any dirty Pages.
  ASSERT_EQ(file.GetDirtyPageCount(), 0U);
  FileTester::AppendToFile(&file, buf, kBlockSize);
  FileTester::AppendToFile(&file, buf, kBlockSize);
  // Flush the Page of 1st block.
  std::lock_guard gc_lock(f2fs::GetGlobalLock());
  {
    LockedPage page = GetLockedPage(0);
    ASSERT_EQ(file.GetDirtyPageCount(), 2U);
    key = page->GetKey();
    auto unlocked_page = page.release();
    // Request writeback for dirty Pages. |unlocked_page| should be written out.
    key = 0;
    ASSERT_EQ(file.Writeback(op), 1UL);
    // Writeback() should be able to flush |unlocked_page|.
    ASSERT_EQ(file.GetDirtyPageCount(), 1U);
    ASSERT_EQ(fs_->GetSuperblockInfo().GetPageCount(CountType::kWriteback), 0);
    ASSERT_EQ(fs_->GetSuperblockInfo().GetPageCount(CountType::kDirtyData), 1);
    ASSERT_EQ(unlocked_page->IsWriteback(), false);
    ASSERT_EQ(unlocked_page->IsDirty(), false);
  }
  // Set sync. writeback.
  op.bSync = true;
  // Request writeback for dirty Pages, but there is no Page meeting op.if_page.
  ASSERT_EQ(file.Writeback(op), 0UL);
  // Every writeback Page has been already flushed since op.bSync is set.
  ASSERT_EQ(fs_->GetSuperblockInfo().GetPageCount(CountType::kWriteback), 0);
  ASSERT_EQ(fs_->GetSuperblockInfo().GetPageCount(CountType::kDirtyData), 1);

  key = 1;
  // Set async. writeback.
  op.bSync = false;
  // Now, 2nd Page meets op.if_page.
  ASSERT_EQ(file.Writeback(op), 1UL);
  ASSERT_EQ(file.GetDirtyPageCount(), 0U);
  ASSERT_EQ(fs_->GetSuperblockInfo().GetPageCount(CountType::kDirtyData), 0);
  // Set sync. writeback.
  op.bSync = true;
  // No dirty Pages to be written.
  // All writeback Pages should be clean.
  ASSERT_EQ(file.Writeback(op), 0UL);
  ASSERT_EQ(fs_->GetSuperblockInfo().GetPageCount(CountType::kWriteback), 0);

  // The Pages should not be kept on vnodes using pager after writeback.
  {
    ASSERT_EQ(file.Writeback(op), 0UL);
    fbl::RefPtr<Page> page1, page2;
    ASSERT_EQ(file.FindPage(0, &page1), ZX_ERR_NOT_FOUND);
    ASSERT_EQ(file.FindPage(1, &page2), ZX_ERR_NOT_FOUND);
  }

  // Invalidate pages
  {
    LockedPage page = GetLockedPage(0);
    page = GetLockedPage(1);
  }
  file.InvalidatePages();
  {
    fbl::RefPtr<Page> page1, page2;
    ASSERT_EQ(file.FindPage(0, &page1), ZX_ERR_NOT_FOUND);
    ASSERT_EQ(file.FindPage(1, &page2), ZX_ERR_NOT_FOUND);
  }
}

TEST_F(FileCacheTest, Recycle) {
  char buf[kBlockSize];
  auto &file = vnode<File>();
  FileTester::AppendToFile(&file, buf, kBlockSize);

  LockedPage locked_page = GetLockedPage(0);
  ASSERT_EQ(locked_page->IsDirty(), true);
  fbl::RefPtr<Page> page = locked_page.release();

  // Remove |raw_page| from writeback list
  file.Writeback(true);
  ASSERT_FALSE(page->IsDirty());
  ASSERT_TRUE(page->InTreeContainer());
  zx::nanosleep(zx::deadline_after(zx::msec(100)));
  ASSERT_EQ(page->IsLastReference(), true);
  Page *raw_page = page.get();
  page.reset();

  FileCache &cache = raw_page->GetFileCache();
  locked_page = GetLockedPage(0);
  // Test FileCache::GetLockedPage() and FileCache::Downgrade() with multiple threads
  std::thread thread1([&]() {
    int i = 1000;
    while (--i) {
      LockedPage page = GetLockedPage(0);
      ASSERT_EQ(page.get(), raw_page);
    }
  });

  std::thread thread2([&]() {
    int i = 1000;
    while (--i) {
      LockedPage page(GetPage(0));
      ASSERT_EQ(page.get(), raw_page);
    }
  });

  // Start the execution of threads.
  locked_page.reset();

  thread1.join();
  thread2.join();

  cache.InvalidatePages();

  // Run FileCache::Downgrade() and FileCache::Reset() on different threads.
  std::thread thread_get_page([&]() {
    bool bStop = false;
    std::thread thread_reset([&]() {
      while (!bStop) {
        cache.Reset();
      }
    });

    int i = 1000;
    while (--i) {
      LockedPage page = GetLockedPage(0);
      ASSERT_EQ(page->IsUptodate(), false);
    }
    bStop = true;
    thread_reset.join();
  });

  thread_get_page.join();
}

TEST_F(FileCacheTest, GetPages) {
  constexpr uint32_t kTestNum = 2;
  auto &file = vnode<File>();

  // Keep multiple references to a unlocked Page by calling VnodeF2fs::GrabPages() without blocking
  std::vector<fbl::RefPtr<Page>> pages;
  for (size_t loop = 0; loop < kTestNum; ++loop) {
    zx::result pages_or = file.GrabPages(0, kTestNum);
    ASSERT_TRUE(pages_or.is_ok());
    for (auto &page : *pages_or) {
      pages.push_back(page);
    }
  }
  size_t i = 0;
  for (auto &page : pages) {
    ASSERT_EQ(page->GetKey(), i++ % kTestNum);
  }
}

TEST_F(FileCacheTest, GetLockedPages) {
  constexpr uint32_t kTestNum = 10;
  constexpr uint32_t kTestEndNum = kTestNum * 2;
  char buf[kBlockSize * kTestNum];
  auto &file = vnode<File>();

  FileTester::AppendToFile(&file, buf, Page::Size() * kTestNum);

  std::vector<LockedPage> locked_pages;
  zx::result pages_or = file.GrabLockedPages(0, kTestEndNum);
  ASSERT_TRUE(pages_or.is_ok());
  for (size_t i = 0; i < kTestNum; ++i) {
    ASSERT_EQ(pages_or.value()[i]->IsDirty(), true);
  }
  for (size_t i = kTestNum; i < kTestEndNum; ++i) {
    ASSERT_EQ(pages_or.value()[i]->IsDirty(), false);
  }
  locked_pages = std::move(pages_or.value());

  auto task = [&]() {
    int i = 1000;
    while (--i) {
      auto pages_or = file.GrabLockedPages(0, kTestEndNum);
      ASSERT_TRUE(pages_or.is_ok());
      for (size_t i = 0; i < kTestNum; ++i) {
        ASSERT_EQ(pages_or.value()[i]->IsDirty(), true);
      }
      for (size_t i = kTestNum; i < kTestEndNum; ++i) {
        ASSERT_EQ(pages_or.value()[i]->IsDirty(), false);
      }
    }
  };
  // Test FileCache::GetLockedPages() with multiple threads
  std::thread thread1(task);
  std::thread thread2(task);
  // Start threads.
  for (auto &locked_page : locked_pages) {
    locked_page.reset();
  }
  thread1.join();
  thread2.join();
}

TEST_F(FileCacheTest, Basic) {
  uint8_t buf[kBlockSize];
  const uint16_t nblocks = 256;
  auto &file = vnode<File>();

  // All pages should not be uptodated.
  for (uint16_t i = 0; i < nblocks; ++i) {
    LockedPage page = GetLockedPage(i);
    // A newly created page should have kPageUptodate/kPageDirty/kPageWriteback flags clear.
    ASSERT_EQ(page->IsUptodate(), false);
    ASSERT_EQ(page->IsDirty(), false);
    ASSERT_EQ(page->IsWriteback(), false);
  }

  // Append |nblocks| * |kBlockSize|.
  // Each block is filled with its block offset.
  for (uint16_t i = 0; i < nblocks; ++i) {
    memset(buf, i, kBlockSize);
    FileTester::AppendToFile(&file, buf, kBlockSize);
  }

  // All pages should be uptodated and dirty.
  for (uint16_t i = 0; i < nblocks; ++i) {
    memset(buf, i, kBlockSize);
    LockedPage page = GetLockedPage(i);
    ASSERT_EQ(page->IsUptodate(), true);
    ASSERT_EQ(page->IsDirty(), true);
    BlockBuffer read_buffer;
    page->Read(read_buffer.get());
    ASSERT_EQ(memcmp(buf, read_buffer.get(), kBlockSize), 0);
  }

  // Write out some dirty pages
  WritebackOperation op = {.end = nblocks / 2, .bSync = true};
  fs::SharedLock lock(f2fs::GetGlobalLock());
  file.Writeback(op);

  // Check if each page has a correct dirty flag.
  for (size_t i = 0; i < nblocks; ++i) {
    LockedPage page = GetLockedPage(i);
    if (i < nblocks / 2) {
      ASSERT_EQ(page->IsUptodate(), false);
      ASSERT_EQ(page->IsDirty(), false);
    } else {
      ASSERT_EQ(page->IsUptodate(), true);
      ASSERT_EQ(page->IsDirty(), true);
    }
  }
}

TEST_F(FileCacheTest, Truncate) TA_NO_THREAD_SAFETY_ANALYSIS {
  uint8_t buf[kBlockSize];
  const uint16_t nblocks = 256;
  auto &file = vnode<File>();

  // Append |nblocks| * |kBlockSize|.
  // Each block is filled with its block offset.
  for (uint16_t i = 0; i < nblocks; ++i) {
    memset(buf, i, kBlockSize);
    FileTester::AppendToFile(&file, buf, kBlockSize);
  }

  // All pages should be uptodated and dirty.
  for (uint16_t i = 0; i < nblocks; ++i) {
    LockedPage page = GetLockedPage(i);
    ASSERT_EQ(page->IsUptodate(), true);
    ASSERT_EQ(page->IsDirty(), true);
  }

  // Truncate test_vnode to the half.
  pgoff_t start = static_cast<pgoff_t>(nblocks) / 2 * kBlockSize;
  file.Truncate(start);

  // Check if each page has correct flags.
  for (size_t i = 0; i < nblocks; ++i) {
    LockedPage page = GetLockedPage(i);
    zx::result data_blkaddr = file.FindAddresses(i, 1);
    ASSERT_TRUE(data_blkaddr.is_ok());
    if (i >= start / kBlockSize) {
      ASSERT_EQ(page->IsDirty(), false);
      ASSERT_EQ(page->IsUptodate(), false);
      ASSERT_EQ(data_blkaddr->front(), kNullAddr);
    } else {
      ASSERT_EQ(page->IsDirty(), true);
      ASSERT_EQ(page->IsUptodate(), true);
      ASSERT_EQ(data_blkaddr->front(), kNewAddr);
    }
  }

  --start;
  // Punch a hole at start
  file.TruncateHole(start, start + 1);

  {
    LockedPage page = GetLockedPage(start);
    zx::result data_blkaddr = file.FindAddresses(start, 1);
    ASSERT_TRUE(data_blkaddr.is_ok());
    // |page| for the hole should be invalidated.
    ASSERT_EQ(page->IsDirty(), false);
    ASSERT_EQ(page->IsUptodate(), false);
    ASSERT_EQ(data_blkaddr->front(), kNullAddr);
  }
}

TEST_F(FileCacheTest, LockedPageRelease) {
  auto &file = vnode<File>();
  GetLockedPage(0);

  fbl::RefPtr<Page> page;
  ASSERT_EQ(file.FindPage(0, &page), ZX_OK);

  // Lock and unlock page
  LockedPage locked_page(page);
  fbl::RefPtr<Page> released_page = locked_page.release();
  ASSERT_FALSE(locked_page);
  // Check its lock is free
  locked_page = LockedPage(released_page);
  ASSERT_EQ(page, released_page);
}

using WritebackTest = SingleFileTest;

TEST_F(WritebackTest, DataWriteFailure) {
  char buf[kBlockSize];
  FileTester::AppendToFile(&vnode<File>(), buf, kBlockSize);
  fbl::RefPtr<Page> page;
  ASSERT_EQ(vnode<File>().FindPage(0, &page), ZX_OK);
  ASSERT_TRUE(page->IsDirty());

  vnode<File>().Writeback(true, true);
  ASSERT_FALSE(page->IsDirty());

  // I/O failure
  auto hook = [](const BlockFifoRequest &_req, const zx::vmo *_vmo) { return ZX_ERR_IO; };
  LockedPage(page).SetDirty();
  ASSERT_TRUE(page->IsDirty());
  DeviceTester::SetHook(fs_.get(), hook);
  vnode<File>().Writeback(true, true);
  ASSERT_TRUE(page->IsDirty());
  {
    vnode<File>().Writeback(false, true);
    LockedPage locked_page(page);
    locked_page.WaitOnWriteback();
    ASSERT_TRUE(page->IsDirty());
  }

  // Retry
  DeviceTester::SetHook(fs_.get(), nullptr);
  vnode<File>().Writeback(false, true);
  ASSERT_FALSE(page->IsDirty());
  ASSERT_FALSE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));
}

TEST_F(WritebackTest, InvalidPage) {
  char buf[kBlockSize];
  FileTester::AppendToFile(&vnode<File>(), buf, kBlockSize);
  /// Schedule |page| for writeback
  vnode<File>().Writeback();

  fbl::RefPtr<Page> page;
  ASSERT_EQ(vnode<File>().FindPage(0, &page), ZX_OK);
  ASSERT_TRUE(page->IsWriteback());
  ASSERT_FALSE(page->IsDirty());
  ASSERT_TRUE(IsValidBlockAddr(page->GetBlockAddr()));

  {
    // Make an exception case that |page| is assigned an invalid address during writeback
    LockedPage locked_page(page);
    locked_page->ClearWriteback();
    locked_page.SetWriteback(kNewAddr);
    locked_page.WaitOnWriteback();
  }

  // |page| should be notified.
  ASSERT_FALSE(page->IsWriteback());
}

TEST_F(FileCacheTest, ReadaheadSize) {
  constexpr size_t size = 10;
  constexpr size_t max_size = size + kMaxReadaheadSize;
  FileCache *cache = &GetPage(0)->GetFileCache();
  size_t num_pages = cache->GetReadHint(0, size, max_size);
  ASSERT_EQ(num_pages, max_size);
  cache->Reset();

  GetPage(size);
  num_pages = cache->GetReadHint(0, size, max_size);
  ASSERT_EQ(num_pages, size);
  cache->Reset();

  size_t offset = (size + max_size) / 2;
  GetPage(offset);
  num_pages = cache->GetReadHint(0, size, max_size);
  ASSERT_EQ(num_pages, offset);
  cache->Reset();

  GetPage(size / 2);
  num_pages = cache->GetReadHint(0, size, max_size);
  ASSERT_EQ(num_pages, max_size);
  cache->Reset();

  GetPage(size / 2);
  num_pages = cache->GetReadHint(0, size, max_size, true);
  ASSERT_EQ(num_pages, size);
  cache->Reset();
}

}  // namespace
}  // namespace f2fs
