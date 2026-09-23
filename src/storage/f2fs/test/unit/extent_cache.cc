// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <unordered_set>

#include <gtest/gtest.h>

#include "safemath/safe_conversions.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

class ExtentCacheTest : public F2fsFakeDevTestFixture {
 public:
  ExtentCacheTest() : F2fsFakeDevTestFixture() {}

  void SetUp() override {
    F2fsFakeDevTestFixture::SetUp();

    zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
    ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
    file_ = fbl::RefPtr<File>::Downcast(*std::move(test_file));
  }

  void TearDown() override {
    ASSERT_EQ(file_->Close(), ZX_OK);
    file_ = nullptr;

    F2fsFakeDevTestFixture::TearDown();
  }

 protected:
  fbl::RefPtr<File> file_;
};

TEST(ExtentCacheMountFlagTest, MountFlag) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDevWithOptions(&bc, MkfsOptions{});

  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  // Disable Extent Cache
  ASSERT_EQ(options.SetValue(MountOption::kReadExtentCache, 0), ZX_OK);
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);
  ASSERT_TRUE(fs->IsValid());

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);

  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  zx::result test_file = root_dir->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<File> file = fbl::RefPtr<File>::Downcast(*std::move(test_file));

  ASSERT_EQ(file->ExtentCacheAvailable(), false);

  ASSERT_EQ(file->Close(), ZX_OK);
  file = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;
  FileTester::Unmount(std::move(fs), &bc);
}

TEST_F(ExtentCacheTest, VnodeFlag) {
  file_->SetFlag(InodeInfoFlag::kNoExtent);
  ASSERT_EQ(file_->ExtentCacheAvailable(), false);
}

TEST_F(ExtentCacheTest, UpdateExtentCache) {
  constexpr uint32_t kNumPage = 10;

  char buf[kBlockSize];
  for (uint32_t i = 0; i < kNumPage; ++i) {
    FileTester::AppendToFile(file_.get(), buf, kBlockSize);
  }
  file_->Writeback(true, true);

  auto extent_info = file_->GetExtentTree().LookupExtent(0);
  ASSERT_TRUE(extent_info.is_ok());
  ASSERT_EQ(extent_info->fofs, 0UL);
  ASSERT_EQ(extent_info->len, kNumPage);

  auto path_or = file_->GetNodePath(0);
  ASSERT_TRUE(path_or.is_ok());
  auto page_or = fs_->GetNodeManager().GetLockedDnodePage(*path_or, file_->IsDir());
  ASSERT_TRUE(page_or.is_ok());

  ASSERT_EQ(extent_info->blk_addr, page_or.value().GetPage<NodePage>().GetBlockAddr(0));
  ASSERT_EQ(extent_info->blk_addr + kNumPage - 1,
            page_or.value().GetPage<NodePage>().GetBlockAddr(kNumPage - 1));

  auto largest_extent_info = file_->GetExtentTree().GetLargestExtent();
  ASSERT_EQ(largest_extent_info.fofs, 0UL);
  ASSERT_EQ(largest_extent_info.len, kNumPage);
  ASSERT_EQ(largest_extent_info.blk_addr, page_or.value().GetPage<NodePage>().GetBlockAddr(0));
}

TEST_F(ExtentCacheTest, LookupExtentCache) {
  DisableFsck();

  constexpr pgoff_t kTestPgOff = 10;
  constexpr block_t kTestBlkaddr = 30;
  constexpr uint32_t kTestLen = 10;

  auto block_addresses_or = file_->FindAddresses(kTestPgOff, kTestLen);
  ASSERT_TRUE(block_addresses_or.is_ok());
  for (uint32_t i = 0; i < kTestLen; ++i) {
    ASSERT_EQ(block_addresses_or.value()[i], kNullAddr);
  }

  // Manually insert extent
  ASSERT_TRUE(
      file_->GetExtentTree()
          .InsertExtent(ExtentInfo{.fofs = kTestPgOff, .blk_addr = kTestBlkaddr, .len = kTestLen})
          .is_ok());

  block_addresses_or = file_->FindAddresses(kTestPgOff, kTestLen);
  ASSERT_TRUE(block_addresses_or.is_ok());
  for (uint32_t i = 0; i < kTestLen; ++i) {
    ASSERT_EQ(block_addresses_or.value()[i], kTestBlkaddr + i);
  }
}

TEST_F(ExtentCacheTest, Remount) {
  constexpr uint32_t kNumPage = 10;

  char buf[kBlockSize];
  for (uint32_t i = 0; i < kNumPage; ++i) {
    FileTester::AppendToFile(file_.get(), buf, kBlockSize);
  }

  // Remount
  DisableFsck();
  ASSERT_EQ(file_->Close(), ZX_OK);
  file_ = nullptr;
  ASSERT_EQ(root_dir_->Close(), ZX_OK);
  root_dir_ = nullptr;
  FileTester::Unmount(std::move(fs_), &bc_);
  fbl::RefPtr<VnodeF2fs> root;
  FileTester::MountWithOptions(loop_.dispatcher(), mount_options_, &bc_, &fs_);
  FileTester::CreateRoot(fs_.get(), &root);
  root_dir_ = fbl::RefPtr<Dir>::Downcast(std::move(root));
  fbl::RefPtr<fs::Vnode> test_file;
  FileTester::Lookup(root_dir_.get(), "test", &test_file);
  file_ = fbl::RefPtr<File>::Downcast(std::move(test_file));

  // Check if extent information is preserved
  auto extent_info = file_->GetExtentTree().LookupExtent(0);
  ASSERT_TRUE(extent_info.is_ok());
  ASSERT_EQ(extent_info->fofs, 0UL);
  ASSERT_EQ(extent_info->len, kNumPage);

  auto path_or = file_->GetNodePath(0);
  ASSERT_TRUE(path_or.is_ok());
  auto page_or = fs_->GetNodeManager().GetLockedDnodePage(*path_or, file_->IsDir());
  ASSERT_TRUE(page_or.is_ok());

  ASSERT_EQ(extent_info->blk_addr, page_or.value().GetPage<NodePage>().GetBlockAddr(0));
  ASSERT_EQ(extent_info->blk_addr + kNumPage - 1,
            page_or.value().GetPage<NodePage>().GetBlockAddr(kNumPage - 1));

  auto largest_extent_info = file_->GetExtentTree().GetLargestExtent();
  ASSERT_EQ(largest_extent_info.fofs, 0UL);
  ASSERT_EQ(largest_extent_info.len, kNumPage);
  ASSERT_EQ(largest_extent_info.blk_addr, page_or.value().GetPage<NodePage>().GetBlockAddr(0));
}

TEST_F(ExtentCacheTest, SplitAndMerge) TA_NO_THREAD_SAFETY_ANALYSIS {
  constexpr uint32_t kNumPage = kMinExtentLen * 3;

  char buf[kBlockSize];
  for (uint32_t i = 0; i < kNumPage; ++i) {
    FileTester::AppendToFile(file_.get(), buf, kBlockSize);
  }
  file_->Writeback(true, true);

  block_t start_blkaddr;
  constexpr pgoff_t kHolePos = kMinExtentLen + 1;
  constexpr pgoff_t kHoleSize = 4;

  // Truncate Hole (Split Extent)
  {
    file_->TruncateHole(kHolePos, kHolePos + kHoleSize);

    auto extent_info = file_->GetExtentTree().LookupExtent(0);
    ASSERT_TRUE(extent_info.is_ok());
    ASSERT_EQ(extent_info->fofs, 0UL);
    ASSERT_EQ(extent_info->len, kHolePos);

    // front
    auto path_or = file_->GetNodePath(0);
    ASSERT_TRUE(path_or.is_ok());
    auto page_or = fs_->GetNodeManager().GetLockedDnodePage(*path_or, file_->IsDir());
    ASSERT_TRUE(page_or.is_ok());

    start_blkaddr = page_or.value().GetPage<NodePage>().GetBlockAddr(0);
    ASSERT_EQ(extent_info->blk_addr, start_blkaddr);
    ASSERT_EQ(extent_info->blk_addr + kNumPage - 1,
              page_or.value().GetPage<NodePage>().GetBlockAddr(kNumPage - 1));

    // back
    extent_info = file_->GetExtentTree().LookupExtent(kHolePos + kHoleSize);
    ASSERT_TRUE(extent_info.is_ok());
    ASSERT_EQ(extent_info->fofs, kHolePos + kHoleSize);
    ASSERT_EQ(extent_info->len, kNumPage - (kHolePos + kHoleSize));
    ASSERT_EQ(extent_info->blk_addr,
              page_or.value().GetPage<NodePage>().GetBlockAddr(kHolePos + kHoleSize));
  }

  // Re-insert invalidated region (Merge Extent)
  {
    ASSERT_TRUE(
        file_->GetExtentTree()
            .InsertExtent(ExtentInfo{.fofs = kHolePos,
                                     .blk_addr = start_blkaddr + static_cast<block_t>(kHolePos),
                                     .len = kHoleSize})
            .is_ok());

    auto extent_info = file_->GetExtentTree().LookupExtent(0);
    ASSERT_TRUE(extent_info.is_ok());
    ASSERT_EQ(extent_info->fofs, 0UL);
    ASSERT_EQ(extent_info->len, kNumPage);
    ASSERT_EQ(extent_info->blk_addr, start_blkaddr);
  }
}

TEST_F(ExtentCacheTest, GcConsistency) {
  constexpr uint32_t kNumPage = 10;

  char buf[kBlockSize];
  for (uint32_t i = 0; i < kNumPage; ++i) {
    FileTester::AppendToFile(file_.get(), buf, kBlockSize);
  }
  file_->Writeback(true, true);

  block_t blkaddr_before, blkaddr_after;

  // Check extent cache before gc
  {
    auto path_or = file_->GetNodePath(0);
    ASSERT_TRUE(path_or.is_ok());
    auto page_or = fs_->GetNodeManager().GetLockedDnodePage(*path_or, file_->IsDir());
    ASSERT_TRUE(page_or.is_ok());
    blkaddr_before = page_or.value().GetPage<NodePage>().GetBlockAddr(0);

    auto extent_info = file_->GetExtentTree().LookupExtent(0);
    ASSERT_TRUE(extent_info.is_ok());
    ASSERT_EQ(extent_info->fofs, 0UL);
    ASSERT_EQ(extent_info->len, kNumPage);
    ASSERT_EQ(extent_info->blk_addr, blkaddr_before);
  }

  // Trigger GC
  {
    // To allow the foreground GC to move the block, we need to change the curseg
    fs_->GetSegmentManager().AllocateNewSegments();
    auto segno = fs_->GetSegmentManager().GetSegmentNumber(blkaddr_before);

    fbl::RefPtr<Page> sum_page;
    {
      LockedPage locked_sum_page;
      fs_->GetSegmentManager().GetSumPage(segno, &locked_sum_page);
      sum_page = locked_sum_page.release();
    }

    SummaryBlock *sum_blk = sum_page->GetAddress<SummaryBlock>();
    ASSERT_EQ(GetSumType((&sum_blk->footer)), kSumTypeData);

    ASSERT_EQ(GcTester::GcDataSegment(fs_->GetSegmentManager(), *sum_blk, segno, GcType::kFgGc),
              ZX_OK);
  }

  // Check extent cache after gc
  {
    auto path_or = file_->GetNodePath(0);
    ASSERT_TRUE(path_or.is_ok());
    auto page_or = fs_->GetNodeManager().GetLockedDnodePage(*path_or, file_->IsDir());
    ASSERT_TRUE(page_or.is_ok());
    blkaddr_after = page_or.value().GetPage<NodePage>().GetBlockAddr(0);
    ASSERT_NE(blkaddr_before, blkaddr_after);

    auto extent_info = file_->GetExtentTree().LookupExtent(0);
    ASSERT_TRUE(extent_info.is_ok());
    ASSERT_EQ(extent_info->fofs, 0UL);
    ASSERT_EQ(extent_info->len, kNumPage);
    ASSERT_EQ(extent_info->blk_addr, blkaddr_after);
  }
}

TEST_F(ExtentCacheTest, RecoveryConsistency) {
  constexpr uint32_t kNumPage = 10;

  char buf[kBlockSize];
  for (uint32_t i = 0; i < kNumPage; ++i) {
    FileTester::AppendToFile(file_.get(), buf, kBlockSize);
  }
  file_->Writeback(true, true);

  block_t blkaddr_before_modify, blkaddr_after_modify;

  // Check extent cache before modify
  {
    auto path_or = file_->GetNodePath(0);
    ASSERT_TRUE(path_or.is_ok());
    auto page_or = fs_->GetNodeManager().GetLockedDnodePage(*path_or, file_->IsDir());
    ASSERT_TRUE(page_or.is_ok());
    blkaddr_before_modify = page_or.value().GetPage<NodePage>().GetBlockAddr(0);

    auto extent_info = file_->GetExtentTree().LookupExtent(0);
    ASSERT_TRUE(extent_info.is_ok());
    ASSERT_EQ(extent_info->fofs, 0UL);
    ASSERT_EQ(extent_info->len, kNumPage);
    ASSERT_EQ(extent_info->blk_addr, blkaddr_before_modify);
  }

  // Write Checkpoint (update largest extent info to disk)
  fs_->SyncFs(false);

  // Rewrite data and fsync. It should change the block address of the data.
  uint64_t pre_checkpoint_ver = fs_->GetSuperblockInfo().GetCheckpoint().checkpoint_ver;
  for (uint32_t i = 0; i < kNumPage; ++i) {
    size_t out_actual;
    FileTester::Write(file_.get(), buf, kBlockSize, i * kBlockSize, &out_actual);
    ASSERT_EQ(out_actual, kBlockSize);
  }

  ASSERT_EQ(file_->SyncFile(false), ZX_OK);

  // Checkpoint should not be performed.
  uint64_t curr_checkpoint_ver = fs_->GetSuperblockInfo().GetCheckpoint().checkpoint_ver;
  ASSERT_EQ(pre_checkpoint_ver, curr_checkpoint_ver);

  // Check extent cache after modify
  {
    auto path_or = file_->GetNodePath(0);
    ASSERT_TRUE(path_or.is_ok());
    auto page_or = fs_->GetNodeManager().GetLockedDnodePage(*path_or, file_->IsDir());
    ASSERT_TRUE(page_or.is_ok());
    blkaddr_after_modify = page_or.value().GetPage<NodePage>().GetBlockAddr(0);
    ASSERT_NE(blkaddr_before_modify, blkaddr_after_modify);

    auto extent_info = file_->GetExtentTree().LookupExtent(0);
    ASSERT_TRUE(extent_info.is_ok());
    ASSERT_EQ(extent_info->fofs, 0UL);
    ASSERT_EQ(extent_info->len, kNumPage);
    ASSERT_EQ(extent_info->blk_addr, blkaddr_after_modify);
  }

  // SPO
  ASSERT_EQ(file_->Close(), ZX_OK);
  file_ = nullptr;
  ASSERT_EQ(root_dir_->Close(), ZX_OK);
  root_dir_ = nullptr;
  FileTester::SuddenPowerOff(std::move(fs_), &bc_);

  // Remount without roll-forward recovery
  MountOptions options{};
  ASSERT_EQ(options.SetValue(MountOption::kDisableRollForward, 1), ZX_OK);
  FileTester::MountWithOptions(loop_.dispatcher(), options, &bc_, &fs_);
  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs_.get(), &root);
  root_dir_ = fbl::RefPtr<Dir>::Downcast(std::move(root));
  fbl::RefPtr<fs::Vnode> test_file;
  FileTester::Lookup(root_dir_.get(), "test", &test_file);
  file_ = fbl::RefPtr<File>::Downcast(std::move(test_file));
  curr_checkpoint_ver = fs_->GetSuperblockInfo().GetCheckpoint().checkpoint_ver;
  ASSERT_EQ(pre_checkpoint_ver, curr_checkpoint_ver);

  // Check extent cache withouth recovery. The largest extent info should not be updated.
  {
    auto extent_info = file_->GetExtentTree().LookupExtent(0);
    ASSERT_TRUE(extent_info.is_ok());
    ASSERT_EQ(extent_info->fofs, 0UL);
    ASSERT_EQ(extent_info->len, kNumPage);
    ASSERT_EQ(extent_info->blk_addr, blkaddr_before_modify);
  }

  // SPO
  ASSERT_EQ(file_->Close(), ZX_OK);
  file_ = nullptr;
  ASSERT_EQ(root_dir_->Close(), ZX_OK);
  root_dir_ = nullptr;
  FileTester::SuddenPowerOff(std::move(fs_), &bc_);

  // Remount with roll-forward recovery
  ASSERT_EQ(options.SetValue(MountOption::kDisableRollForward, 0), ZX_OK);
  FileTester::MountWithOptions(loop_.dispatcher(), options, &bc_, &fs_);
  FileTester::CreateRoot(fs_.get(), &root);
  root_dir_ = fbl::RefPtr<Dir>::Downcast(std::move(root));
  FileTester::Lookup(root_dir_.get(), "test", &test_file);
  file_ = fbl::RefPtr<File>::Downcast(std::move(test_file));
  curr_checkpoint_ver = fs_->GetSuperblockInfo().GetCheckpoint().checkpoint_ver;
  ASSERT_EQ(pre_checkpoint_ver + 1, curr_checkpoint_ver);

  // Check extent cache after recovery
  {
    auto extent_info = file_->GetExtentTree().LookupExtent(0);
    ASSERT_TRUE(extent_info.is_ok());
    ASSERT_EQ(extent_info->fofs, 0UL);
    ASSERT_EQ(extent_info->len, kNumPage);
    ASSERT_EQ(extent_info->blk_addr, blkaddr_after_modify);
  }
}

}  // namespace
}  // namespace f2fs
