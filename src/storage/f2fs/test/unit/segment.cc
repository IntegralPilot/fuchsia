// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <unordered_set>

#include <gtest/gtest.h>
#include <safemath/checked_math.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

using Runner = ComponentRunner;

class SegmentManagerTest : public F2fsFakeDevTestFixture {
 public:
  SegmentManagerTest()
      : F2fsFakeDevTestFixture(TestOptions{
            .block_count = kSectorCount100MiB,
            .mount_options = {{MountOption::kInlineDentry, false}},
        }) {}

 protected:
  void MakeDirtySegments(size_t invalidate_ratio, int num_files) {
    fs_->GetSegmentManager().DisableFgGc();
    for (int file_no = 0; file_no < num_files; ++file_no) {
      zx::result test_file = root_dir_->Create(std::to_string(file_num++), fs::CreationType::kFile);
      ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
      auto vnode = fbl::RefPtr<File>::Downcast(*std::move(test_file));
      std::array<char, kBlockSize> buf;
      std::memset(buf.data(), file_no, buf.size());
      for (size_t i = 0; i < fs_->GetSuperblockInfo().GetBlocksPerSeg(); ++i) {
        FileTester::AppendToFile(vnode.get(), buf.data(), buf.size());
      }
      vnode->SyncFile(false);
      size_t truncate_size = vnode->GetSize() * (100 - invalidate_ratio) / 100;
      EXPECT_EQ(vnode->Truncate(truncate_size), ZX_OK);
      vnode->Close();
    }
    fs_->SyncFs();
    fs_->GetSegmentManager().EnableFgGc();
  }

 private:
  int file_num = 0;
};

TEST_F(SegmentManagerTest, BlkChaining) {
  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();
  std::vector<block_t> blk_chain(0);
  int nwritten = kDefaultBlocksPerSegment * 2;
  // write the root inode, and read the block where the previous version of the root inode is stored
  // to check if the block has a proper lba address to the next node block
  for (int i = 0; i < nwritten; ++i) {
    NodeInfo ni;
    {
      LockedPage read_page;
      fs_->GetNodeManager().GetNodePage(superblock_info.GetRootIno(), &read_page);
      blk_chain.push_back(read_page.GetPage<NodePage>().NextBlkaddrOfNode());
      read_page.SetDirty();
    }
    fs_->GetNodeVnode().Writeback(true, true);

    fs_->GetNodeManager().GetNodeInfo(superblock_info.GetRootIno(), ni);
    ASSERT_NE(ni.blk_addr, kNullAddr);
    ASSERT_NE(ni.blk_addr, kNewAddr);
    ASSERT_EQ(ni.blk_addr, blk_chain[i]);
  }
}

TEST_F(SegmentManagerTest, DirtyToFree) TA_NO_THREAD_SAFETY_ANALYSIS {
  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();

  // check the precond. before making dirty segments
  std::vector<uint32_t> prefree_array(0);
  int nwritten = kDefaultBlocksPerSegment * 2;
  uint32_t nprefree = 0;
  ASSERT_FALSE(fs_->GetSegmentManager().PrefreeSegments());
  uint32_t nfree_segs = fs_->GetSegmentManager().FreeSegments();
  FreeSegmapInfo *free_i = &fs_->GetSegmentManager().GetFreeSegmentInfo();
  DirtySeglistInfo *dirty_i = &fs_->GetSegmentManager().GetDirtySegmentInfo();

  // write the root inode repeatedly as much as 2 segments
  for (int i = 0; i < nwritten; ++i) {
    NodeInfo ni;
    fs_->GetNodeManager().GetNodeInfo(superblock_info.GetRootIno(), ni);
    ASSERT_NE(ni.blk_addr, kNullAddr);
    ASSERT_NE(ni.blk_addr, kNewAddr);
    block_t old_addr = ni.blk_addr;

    {
      LockedPage read_page;
      fs_->GetNodeManager().GetNodePage(superblock_info.GetRootIno(), &read_page);
      read_page.SetDirty();
    }

    fs_->GetNodeVnode().Writeback(true, true);

    if (fs_->GetSegmentManager().GetValidBlocks(fs_->GetSegmentManager().GetSegmentNumber(old_addr),
                                                0) == 0) {
      prefree_array.push_back(fs_->GetSegmentManager().GetSegmentNumber(old_addr));
      ASSERT_EQ(fs_->GetSegmentManager().PrefreeSegments(), ++nprefree);
    }
  }

  // check the bitmaps and the number of free/prefree segments
  ASSERT_EQ(fs_->GetSegmentManager().FreeSegments(), nfree_segs - nprefree);
  for (auto &pre : prefree_array) {
    ASSERT_TRUE(dirty_i->dirty_segmap[static_cast<int>(DirtyType::kPre)].GetOne(pre));
    ASSERT_TRUE(free_i->free_segmap.GetOne(pre));
  }
  // triggers checkpoint to make prefree segments transit to free ones
  fs_->SyncFs();

  // check the bitmaps and the number of free/prefree segments
  for (auto &pre : prefree_array) {
    ASSERT_FALSE(free_i->free_segmap.GetOne(pre));
    ASSERT_FALSE(dirty_i->dirty_segmap[static_cast<int>(DirtyType::kPre)].GetOne(pre));
  }
  ASSERT_EQ(fs_->GetSegmentManager().FreeSegments(), nfree_segs);
  ASSERT_FALSE(fs_->GetSegmentManager().PrefreeSegments());
}

TEST_F(SegmentManagerTest, BalanceFs) TA_NO_THREAD_SAFETY_ANALYSIS {
  uint32_t nfree_segs = fs_->GetSegmentManager().FreeSegments();

  fs_->ClearOnRecovery();
  fs_->BalanceFs();

  ASSERT_EQ(fs_->GetSegmentManager().FreeSegments(), nfree_segs);
  ASSERT_FALSE(fs_->GetSegmentManager().PrefreeSegments());

  fs_->SetOnRecovery();
  fs_->BalanceFs();

  ASSERT_EQ(fs_->GetSegmentManager().FreeSegments(), nfree_segs);
  ASSERT_FALSE(fs_->GetSegmentManager().PrefreeSegments());
}

TEST_F(SegmentManagerTest, InvalidateBlocksExceptionCase) {
  // read the root inode block
  LockedPage root_node_page;
  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();
  fs_->GetNodeManager().GetNodePage(superblock_info.GetRootIno(), &root_node_page);
  ASSERT_NE(root_node_page, nullptr);

  // Check InvalidateBlocks() exception case
  block_t temp_written_valid_blocks = fs_->GetSegmentManager().GetSitInfo().written_valid_blocks;
  fs_->GetSegmentManager().InvalidateBlocks(kNewAddr);
  ASSERT_EQ(temp_written_valid_blocks, fs_->GetSegmentManager().GetSitInfo().written_valid_blocks);
}

TEST_F(SegmentManagerTest, GetVictimSelPolicy) TA_NO_THREAD_SAFETY_ANALYSIS {
  VictimSelPolicy policy = fs_->GetSegmentManager().GetVictimSelPolicy(
      GcType::kFgGc, CursegType::kCursegHotNode, AllocType::kSSR);
  ASSERT_EQ(policy.gc_mode, GcMode::kGcGreedy);
  ASSERT_EQ(policy.ofs_unit, 1U);

  policy = fs_->GetSegmentManager().GetVictimSelPolicy(GcType::kFgGc, CursegType::kNoCheckType,
                                                       AllocType::kLFS);
  ASSERT_EQ(policy.gc_mode, GcMode::kGcGreedy);
  ASSERT_EQ(policy.ofs_unit, fs_->GetSuperblockInfo().GetSegsPerSec());
  ASSERT_EQ(policy.offset,
            fs_->GetSegmentManager().GetLastVictim(static_cast<int>(GcMode::kGcGreedy)));

  policy = fs_->GetSegmentManager().GetVictimSelPolicy(GcType::kBgGc, CursegType::kNoCheckType,
                                                       AllocType::kLFS);
  ASSERT_EQ(policy.gc_mode, GcMode::kGcCb);
  ASSERT_EQ(policy.ofs_unit, fs_->GetSuperblockInfo().GetSegsPerSec());
  ASSERT_EQ(policy.offset, fs_->GetSegmentManager().GetLastVictim(static_cast<int>(GcMode::kGcCb)));

  DirtySeglistInfo *dirty_info = &fs_->GetSegmentManager().GetDirtySegmentInfo();
  dirty_info->nr_dirty[static_cast<int>(DirtyType::kDirty)] = kMaxSearchLimit + 2;
  policy = fs_->GetSegmentManager().GetVictimSelPolicy(GcType::kBgGc, CursegType::kNoCheckType,
                                                       AllocType::kLFS);
  ASSERT_EQ(policy.max_search, kMaxSearchLimit);
}

TEST_F(SegmentManagerTest, GetMaxCost) TA_NO_THREAD_SAFETY_ANALYSIS {
  VictimSelPolicy policy = fs_->GetSegmentManager().GetVictimSelPolicy(
      GcType::kFgGc, CursegType::kCursegHotNode, AllocType::kSSR);
  policy.min_cost = fs_->GetSegmentManager().GetMaxCost(policy);
  ASSERT_EQ(policy.min_cost,
            static_cast<uint32_t>(1 << fs_->GetSuperblockInfo().GetLogBlocksPerSeg()));

  policy = fs_->GetSegmentManager().GetVictimSelPolicy(GcType::kFgGc, CursegType::kNoCheckType,
                                                       AllocType::kLFS);
  policy.min_cost = fs_->GetSegmentManager().GetMaxCost(policy);
  ASSERT_EQ(policy.min_cost,
            static_cast<uint32_t>(2 * (1 << fs_->GetSuperblockInfo().GetLogBlocksPerSeg()) *
                                  policy.ofs_unit));

  policy = fs_->GetSegmentManager().GetVictimSelPolicy(GcType::kBgGc, CursegType::kNoCheckType,
                                                       AllocType::kLFS);
  policy.min_cost = fs_->GetSegmentManager().GetMaxCost(policy);
  ASSERT_EQ(policy.min_cost, std::numeric_limits<uint32_t>::max());
}

TEST_F(SegmentManagerTest, GetVictimByDefault) TA_NO_THREAD_SAFETY_ANALYSIS {
  DirtySeglistInfo *dirty_info = &fs_->GetSegmentManager().GetDirtySegmentInfo();

  uint32_t target_segno;
  for (target_segno = 0; target_segno < fs_->GetSegmentManager().TotalSegs(); ++target_segno) {
    if (!fs_->GetSegmentManager().SecUsageCheck(fs_->GetSegmentManager().GetSecNo(target_segno)) &&
        fs_->GetSegmentManager().GetValidBlocks(target_segno, 0) == 0U) {
      break;
    }
  }
  ASSERT_NE(target_segno, fs_->GetSegmentManager().TotalSegs());
  fs_->GetSegmentManager().SetSegmentEntryType(target_segno, CursegType::kCursegHotNode);

  // 1. Test SSR victim
  fs_->GetSegmentManager().SetLastVictim(static_cast<int>(GcType::kBgGc), target_segno);
  if (!dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirtyHotNode)].GetOne(target_segno)) {
    dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirtyHotNode)].SetOne(target_segno);
    ++dirty_info->nr_dirty[static_cast<int>(DirtyType::kDirtyHotNode)];
  }

  auto victim_or = fs_->GetSegmentManager().GetVictimByDefault(
      GcType::kBgGc, CursegType::kCursegHotNode, AllocType::kSSR);
  ASSERT_FALSE(victim_or.is_error());
  uint32_t get_victim = victim_or.value();
  ASSERT_EQ(get_victim, target_segno);

  // 2. Test FgGc victim
  fs_->GetSegmentManager().SetLastVictim(static_cast<int>(GcType::kFgGc), target_segno);
  if (!dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirty)].GetOne(target_segno)) {
    dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirty)].SetOne(target_segno);
    ++dirty_info->nr_dirty[static_cast<int>(DirtyType::kDirty)];
  }

  victim_or = fs_->GetSegmentManager().GetVictimByDefault(GcType::kFgGc, CursegType::kNoCheckType,
                                                          AllocType::kLFS);
  ASSERT_FALSE(victim_or.is_error());
  get_victim = victim_or.value();
  ASSERT_EQ(get_victim, target_segno);

  // 3. Skip if cur_victim_sec is set (SSR)
  ASSERT_EQ(fs_->GetSegmentManager().GetCurVictimSec(),
            fs_->GetSegmentManager().GetSecNo(target_segno));
  ASSERT_TRUE(
      dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirtyHotNode)].GetOne(target_segno));
  ASSERT_EQ(dirty_info->nr_dirty[static_cast<int>(DirtyType::kDirtyHotNode)], 1);
  victim_or = fs_->GetSegmentManager().GetVictimByDefault(GcType::kBgGc, CursegType::kCursegHotNode,
                                                          AllocType::kSSR);
  ASSERT_TRUE(victim_or.is_error());

  // 4. Skip if victim_secmap is set (kBgGc)
  fs_->GetSegmentManager().SetCurVictimSec(kNullSecNo);
  ASSERT_TRUE(
      dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirtyHotNode)].GetOne(target_segno));
  ASSERT_EQ(dirty_info->nr_dirty[static_cast<int>(DirtyType::kDirty)], 1);
  dirty_info->victim_secmap.SetOne(fs_->GetSegmentManager().GetSecNo(target_segno));
  victim_or = fs_->GetSegmentManager().GetVictimByDefault(GcType::kBgGc, CursegType::kCursegHotNode,
                                                          AllocType::kLFS);
  ASSERT_TRUE(victim_or.is_error());
}

TEST_F(SegmentManagerTest, SelectBGVictims) TA_NO_THREAD_SAFETY_ANALYSIS {
  fs_->GetSuperblockInfo().SetOpt(MountOption::kForceLfs);
  DirtySeglistInfo *dirty_info = &fs_->GetSegmentManager().GetDirtySegmentInfo();
  auto &bitmap = dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirty)];

  const size_t elapsed_time = 100;
  size_t invalid_ratio = 2;
  SitInfo &segments = fs_->GetSegmentManager().GetSitInfo();
  // Adjust mount time
  segments.mounted_time = time(nullptr) - elapsed_time;

  // Make a dirty segment with 98% of valid blocks
  CursegInfo *curseg = fs_->GetSegmentManager().CURSEG_I(CursegType::kCursegWarmData);
  uint32_t expected_by_bg = curseg->segno;
  MakeDirtySegments(invalid_ratio, 1);
  ASSERT_EQ(CountBits(bitmap, 0, bitmap.size()), 1U);
  SegmentEntry &bg_victim = segments.sentries[expected_by_bg];

  // Make a dirty segment with 97% of valid blocks
  curseg = fs_->GetSegmentManager().CURSEG_I(CursegType::kCursegWarmData);
  uint32_t expected_by_fg = curseg->segno;
  MakeDirtySegments(invalid_ratio + 1, 1);
  ASSERT_EQ(CountBits(bitmap, 0, bitmap.size()), 2U);
  SegmentEntry &fg_victim = segments.sentries[expected_by_fg];

  // Make a dirty segment with 99% of valid blocks
  curseg = fs_->GetSegmentManager().CURSEG_I(CursegType::kCursegWarmData);
  MakeDirtySegments(invalid_ratio - 1, 1);
  ASSERT_EQ(CountBits(bitmap, 0, bitmap.size()), 3U);
  SegmentEntry &dirty = segments.sentries[curseg->segno];

  // Adjust the modification time for dirty segments
  bg_victim.mtime = 1;
  fg_victim.mtime = elapsed_time / 2;
  dirty.mtime = elapsed_time;

  size_t valid_blocks_98 = CheckedDivRoundUp<size_t>(
      fs_->GetSuperblockInfo().GetBlocksPerSeg() * (100 - invalid_ratio), 100);
  size_t valid_blocks_97 = CheckedDivRoundUp<size_t>(
      fs_->GetSuperblockInfo().GetBlocksPerSeg() * (100 - invalid_ratio - 1), 100);

  // GcType::kFgGc should select a victim according to valid blocks
  auto victim_seg_or = fs_->GetSegmentManager().GetVictimByDefault(
      GcType::kFgGc, CursegType::kNoCheckType, AllocType::kLFS);
  ASSERT_TRUE(victim_seg_or.is_ok());
  ASSERT_EQ(fs_->GetSegmentManager().GetValidBlocks(*victim_seg_or, true), valid_blocks_97);
  ASSERT_EQ(expected_by_fg, *victim_seg_or);
  fs_->GetSegmentManager().SetCurVictimSec(kNullSecNo);

  // GcType::kBgGc should select a victim according to the segment age and valid blocks
  victim_seg_or = fs_->GetSegmentManager().GetVictimByDefault(
      GcType::kBgGc, CursegType::kNoCheckType, AllocType::kLFS);
  ASSERT_TRUE(victim_seg_or.is_ok());
  ASSERT_EQ(fs_->GetSegmentManager().GetValidBlocks(*victim_seg_or, true), valid_blocks_98);
  ASSERT_EQ(expected_by_bg, *victim_seg_or);

  // When a victim that GcType::kBgGc chooses is not handled yet, GcType::kFgGc should select the
  // victim
  victim_seg_or = fs_->GetSegmentManager().GetVictimByDefault(
      GcType::kFgGc, CursegType::kNoCheckType, AllocType::kLFS);
  ASSERT_TRUE(victim_seg_or.is_ok());
  ASSERT_EQ(fs_->GetSegmentManager().GetValidBlocks(*victim_seg_or, true), valid_blocks_98);
  fs_->GetSegmentManager().SetCurVictimSec(kNullSecNo);

  // Even after system time is modified, it can select a victim correctly.
  fs_->GetSegmentManager().GetSitInfo().min_mtime = LLONG_MAX;
  victim_seg_or = fs_->GetSegmentManager().GetVictimByDefault(
      GcType::kBgGc, CursegType::kNoCheckType, AllocType::kLFS);
  ASSERT_TRUE(victim_seg_or.is_ok());
  ASSERT_EQ(fs_->GetSegmentManager().GetValidBlocks(*victim_seg_or, true), valid_blocks_98);

  fs_->GetSegmentManager().GetSitInfo().max_mtime = 0;
  dirty_info->victim_secmap.ClearOne(*victim_seg_or / fs_->GetSuperblockInfo().GetSegsPerSec());
  victim_seg_or = fs_->GetSegmentManager().GetVictimByDefault(
      GcType::kBgGc, CursegType::kNoCheckType, AllocType::kLFS);
  ASSERT_TRUE(victim_seg_or.is_ok());
  ASSERT_EQ(fs_->GetSegmentManager().GetValidBlocks(*victim_seg_or, true), valid_blocks_98);
}

TEST_F(SegmentManagerTest, AllocateNewSegments) TA_NO_THREAD_SAFETY_ANALYSIS {
  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();

  uint32_t temp_free_segment = fs_->GetSegmentManager().FreeSegments();
  fs_->GetSegmentManager().AllocateNewSegments();
  ASSERT_EQ(temp_free_segment - 3, fs_->GetSegmentManager().FreeSegments());

  superblock_info.ClearOpt(MountOption::kDisableRollForward);
  temp_free_segment = fs_->GetSegmentManager().FreeSegments();
  for (CursegType curseg_type :
       {CursegType::kCursegHotNode, CursegType::kCursegWarmNode, CursegType::kCursegColdNode}) {
    fs_->GetSegmentManager().AllocateSegmentByDefault(curseg_type, true);
  }
  AllocType type = fs_->GetSegmentManager().CursegAllocType(CursegType::kCursegHotNode);
  ASSERT_EQ(superblock_info.GetSegmentCount(type), 6UL);
  ASSERT_EQ(temp_free_segment - 3, fs_->GetSegmentManager().FreeSegments());
}

TEST_F(SegmentManagerTest, DirtySegments) TA_NO_THREAD_SAFETY_ANALYSIS {
  // read the root inode block
  LockedPage root_node_page;
  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();
  fs_->GetNodeManager().GetNodePage(superblock_info.GetRootIno(), &root_node_page);
  ASSERT_NE(root_node_page, nullptr);

  DirtySeglistInfo &dirty_info = fs_->GetSegmentManager().GetDirtySegmentInfo();
  uint32_t dirtyDataSegments = dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyHotData)] +
                               dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyWarmData)] +
                               dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyColdData)];

  uint32_t dirtyNodeSegments = dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyHotNode)] +
                               dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyWarmNode)] +
                               dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyColdNode)];

  ASSERT_EQ(fs_->GetSegmentManager().DirtySegments(), dirtyDataSegments + dirtyNodeSegments);
}

TEST_F(SegmentManagerTest, SsrData) TA_NO_THREAD_SAFETY_ANALYSIS {
  fs_->GetSuperblockInfo().ClearOpt(MountOption::kForceLfs);
  zx::result vnode_or = root_dir_->Create("hot_data_A", fs::CreationType::kDirectory);
  ASSERT_TRUE(vnode_or.is_ok());
  auto dir = fbl::RefPtr<Dir>::Downcast(*std::move(vnode_or));
  std::vector<fbl::RefPtr<Dir>> dirs;
  dirs.push_back(std::move(dir));

  vnode_or = root_dir_->Create("hot_data_B", fs::CreationType::kDirectory);
  ASSERT_TRUE(vnode_or.is_ok());
  dir = fbl::RefPtr<Dir>::Downcast(*std::move(vnode_or));
  dirs.push_back(std::move(dir));

  constexpr size_t kChunkSize = kDefaultBlocksPerSegment / 2;
  size_t index = 1;
  // Write hot data
  while (fs_->GetSuperblockInfo().Utilization() < 30) {
    for (size_t i = 0; i < 2; ++i) {
      zx::result page = dirs[i]->GetNewLockedPage(index);
      ASSERT_TRUE(page.is_ok());
      page->SetDirty();
    }
    // Writeback dirty pages when each file has kChunkSize dirty pages, which makes a hot data
    // segment have data blocks of the two dirs.
    if (!(index++ % kChunkSize)) {
      fs_->SyncFs();
    }
  }
  fs_->SyncFs();

  DirtySeglistInfo &dirty_info = fs_->GetSegmentManager().GetDirtySegmentInfo();

  // There should be no dirty data segments.
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyHotData)], 0);
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyWarmData)], 0);
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyColdData)], 0);

  // Make dirty hot data segments
  size_t expected = dirs[1]->GetSize() / Page::Size() / kChunkSize;
  dirs[1]->DoTruncate(4096);
  fs_->SyncFs();

  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyHotData)],
            static_cast<int>(expected));
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyWarmData)], 0);
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyColdData)], 0);

  // It should be able to allocate new blocks from dirty hot data segments for hot data (e.g.,
  // |dirs[1]|) without gc and checkpoint.
  auto prev_ver = fs_->GetSuperblockInfo().GetCheckpointVer(true);
  for (size_t i = 1; i <= kDefaultBlocksPerSegment; ++i) {
    zx::result page = dirs[1]->GetNewLockedPage(i);
    ASSERT_TRUE(page.is_ok());
    page->SetDirty();
  }
  ASSERT_EQ(prev_ver, fs_->GetSuperblockInfo().GetCheckpointVer(true));
  fs_->SyncFs();

  prev_ver = fs_->GetSuperblockInfo().GetCheckpointVer(true);

  // It should be able to allocate new blocks from dirty hot data segments for warm data (e.g.,
  // |file|) without gc and checkpoint.
  vnode_or = root_dir_->Create("warm_data", fs::CreationType::kFile);
  ASSERT_TRUE(vnode_or.is_ok());
  auto file = fbl::RefPtr<File>::Downcast(*std::move(vnode_or));
  uint8_t buf[Page::Size()];
  for (size_t i = 0; i < kDefaultBlocksPerSegment * 2; ++i) {
    size_t out = 0;
    ASSERT_EQ(FileTester::Write(file.get(), buf, sizeof(buf), i * Page::Size(), &out), ZX_OK);
    ASSERT_EQ(out, Page::Size());
  }
  file->SyncFile(false);

  ASSERT_EQ(prev_ver, fs_->GetSuperblockInfo().GetCheckpointVer(true));

  file->Close();
  dirs[0]->Close();
  dirs[1]->Close();
}

TEST_F(SegmentManagerTest, SsrNode) TA_NO_THREAD_SAFETY_ANALYSIS {
  size_t i = 0;
  std::vector<fbl::RefPtr<VnodeF2fs>> deleted;
  // Write warm nodes
  while (fs_->GetSuperblockInfo().Utilization() < 30) {
    zx::result warm_node_or = root_dir_->Create(std::to_string(i), fs::CreationType::kFile);
    ASSERT_TRUE(warm_node_or.is_ok());
    warm_node_or->Close();
    if (i++ % 2) {
      deleted.push_back(fbl::RefPtr<VnodeF2fs>::Downcast(*warm_node_or));
    }
  }
  fs_->SyncFs();

  DirtySeglistInfo &dirty_info = fs_->GetSegmentManager().GetDirtySegmentInfo();
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyHotNode)], 0);
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyWarmNode)], 0);
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyColdNode)], 0);

  // Make dirty warm node segments
  FileTester::DeleteChildren(deleted, root_dir_, deleted.size());
  deleted.clear();

  fs_->SyncFs();
  size_t expected = i / kDefaultBlocksPerSegment;
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyHotNode)], 0);
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyWarmNode)],
            static_cast<int>(expected));
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyColdNode)], 0);
  ASSERT_TRUE(expected >= 8);

  // It should be able to allocate new blocks from dirty warm data segments without gc or
  // checkpoint.
  size_t prev_ver = fs_->GetSuperblockInfo().GetCheckpointVer(true);
  for (size_t i = 0; i < kDefaultBlocksPerSegment * 2; ++i) {
    zx::result warm_node_or = root_dir_->Create(std::to_string(i) + "W", fs::CreationType::kFile);
    ASSERT_TRUE(warm_node_or.is_ok());
    warm_node_or->Close();
    zx::result hot_node_or =
        root_dir_->Create(std::to_string(i) + "H", fs::CreationType::kDirectory);
    ASSERT_TRUE(hot_node_or.is_ok());
    hot_node_or->Close();
  }
  ASSERT_EQ(prev_ver, fs_->GetSuperblockInfo().GetCheckpointVer(true));
  fs_->SyncFs();
}

TEST(SegmentManagerOptionTest, Section) TA_NO_THREAD_SAFETY_ANALYSIS {
  std::unique_ptr<BcacheMapper> bc;
  MkfsOptions mkfs_options{};
  mkfs_options.segs_per_sec = 4;
  FileTester::MkfsOnFakeDevWithOptions(&bc, mkfs_options,
                                       kDefaultSectorCount * mkfs_options.segs_per_sec);

  std::unique_ptr<F2fs> fs;
  MountOptions mount_options{};
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), mount_options, &bc, &fs);

  uint32_t blocks_per_section = kDefaultBlocksPerSegment * mkfs_options.segs_per_sec;
  SuperblockInfo &superblock_info = fs->GetSuperblockInfo();

  for (uint32_t i = 0; i < blocks_per_section; ++i) {
    NodeInfo ni;
    CursegInfo *cur_segment = fs->GetSegmentManager().CURSEG_I(CursegType::kCursegHotNode);

    {
      LockedPage root_node_page;
      fs->GetNodeManager().GetNodePage(superblock_info.GetRootIno(), &root_node_page);
      ASSERT_NE(root_node_page, nullptr);

      // Consume a block in the current section
      root_node_page.SetDirty();
    }
    fs->GetNodeVnode().Writeback(true, true);

    fs->GetNodeManager().GetNodeInfo(superblock_info.GetRootIno(), ni);
    ASSERT_NE(ni.blk_addr, kNullAddr);
    ASSERT_NE(ni.blk_addr, kNewAddr);

    unsigned int expected = 1;
    // When a new secton is allocated, the valid block count of the previous one should be zero
    if ((ni.blk_addr + 1) % blocks_per_section == 0) {
      expected = 0;
    }
    ASSERT_EQ(expected,
              fs->GetSegmentManager().GetValidBlocks(
                  cur_segment->segno, safemath::checked_cast<int>(mkfs_options.segs_per_sec)));
    ASSERT_FALSE(fs->GetSegmentManager().HasNotEnoughFreeSecs());
  }

  FileTester::Unmount(std::move(fs), &bc);
}

TEST(SegmentManagerOptionTest, GetNewSegment) TA_NO_THREAD_SAFETY_ANALYSIS {
  std::unique_ptr<BcacheMapper> bc;
  MkfsOptions mkfs_options{};
  mkfs_options.segs_per_sec = 1;
  mkfs_options.secs_per_zone = 2;
  FileTester::MkfsOnFakeDevWithOptions(
      &bc, mkfs_options,
      kDefaultSectorCount * mkfs_options.segs_per_sec * mkfs_options.secs_per_zone);

  std::unique_ptr<F2fs> fs;
  MountOptions mount_options;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), mount_options, &bc, &fs);

  SuperblockInfo &superblock_info = fs->GetSuperblockInfo();
  fs->GetSegmentManager().NewCurseg(CursegType::kCursegHotNode, false);

  uint32_t nwritten =
      kDefaultBlocksPerSegment * mkfs_options.segs_per_sec * mkfs_options.secs_per_zone * 2;

  for (uint32_t i = 0; i < nwritten; ++i) {
    NodeInfo ni, new_ni;
    fs->GetNodeManager().GetNodeInfo(superblock_info.GetRootIno(), ni);
    ASSERT_NE(ni.blk_addr, kNullAddr);
    ASSERT_NE(ni.blk_addr, kNewAddr);

    {
      LockedPage root_node_page;
      fs->GetNodeManager().GetNodePage(superblock_info.GetRootIno(), &root_node_page);
      ASSERT_NE(root_node_page, nullptr);
      root_node_page.SetDirty();
    }
    fs->GetNodeVnode().Writeback(true, true);

    fs->GetNodeManager().GetNodeInfo(superblock_info.GetRootIno(), new_ni);
    ASSERT_NE(new_ni.blk_addr, kNullAddr);
    ASSERT_NE(new_ni.blk_addr, kNewAddr);
    // It tries to find a free nodesction from the start of main area
    ASSERT_GT(new_ni.blk_addr, ni.blk_addr);
  }

  FileTester::Unmount(std::move(fs), &bc);
}

TEST(SegmentManagerOptionTest, DestroySegmentManagerExceptionCase) {
  std::unique_ptr<BcacheMapper> bc;
  MkfsOptions mkfs_options{};
  FileTester::MkfsOnFakeDevWithOptions(&bc, mkfs_options);

  MountOptions mount_options;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto superblock = LoadSuperblock(*bc);
  ASSERT_TRUE(superblock.is_ok());
  // Create a vfs object for unit tests.
  auto vfs_or = Runner::CreateRunner(loop.dispatcher());
  ZX_ASSERT(vfs_or.is_ok());
  std::unique_ptr<F2fs> fs =
      std::make_unique<F2fs>(loop.dispatcher(), std::move(bc), mount_options, (*vfs_or).get());

  ASSERT_EQ(fs->LoadSuper(std::move(*superblock)), ZX_OK);

  fs->Sync();

  // fault injection
  fs->GetSegmentManager().SetDirtySegmentInfo(nullptr);
  fs->GetSegmentManager().SetFreeSegmentInfo(nullptr);
  fs->GetSegmentManager().SetSitInfo(nullptr);

  fs->GetVCache().Reset();
  // test exception case
  fs->Reset();
}

TEST(SegmentManagerExceptionTest, BuildSitEntriesDiskFail) TA_NO_THREAD_SAFETY_ANALYSIS {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDevWithOptions(&bc, MkfsOptions{});

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto superblock = LoadSuperblock(*bc);
  ASSERT_TRUE(superblock.is_ok());

  // Create a vfs object for unit tests.
  auto vfs_or = Runner::CreateRunner(loop.dispatcher());
  ASSERT_TRUE(vfs_or.is_ok());
  std::unique_ptr<F2fs> fs =
      std::make_unique<F2fs>(loop.dispatcher(), std::move(bc), MountOptions{}, (*vfs_or).get());

  ASSERT_EQ(fs->LoadSuper(std::move(*superblock)), ZX_OK);

  pgoff_t target_addr = fs->GetSegmentManager().CurrentSitAddr(0) * kDefaultSectorsPerBlock;

  auto hook = [target_addr](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.dev_offset == target_addr) {
      return ZX_ERR_PEER_CLOSED;
    }
    return ZX_OK;
  };

  fs->GetSegmentManager().DestroySegmentManager();

  // Invalidate sit page to read from disk
  LockedPage sit_page;
  ASSERT_EQ(fs->GetMetaPage(target_addr / kDefaultSectorsPerBlock, &sit_page), ZX_OK);
  sit_page.WaitOnWriteback();
  sit_page.Invalidate();
  sit_page.reset();

  // Expect fail in BuildSitEntries()
  DeviceTester::SetHook(fs.get(), hook);
  ASSERT_EQ(fs->GetSegmentManager().BuildSegmentManager(), ZX_ERR_PEER_CLOSED);

  DeviceTester::SetHook(fs.get(), nullptr);

  fs->GetVCache().Reset();
  fs->Reset();
}

TEST_F(SegmentManagerTest, StartBlock) {
  SegmentManager &segment_manager = fs_->GetSegmentManager();
  SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();

  for (uint32_t segno = 0; segno < segment_manager.TotalSegs(); ++segno) {
    block_t expected_addr =
        segment_manager.GetMainAreaStartBlock() + (segno << superblock_info.GetLogBlocksPerSeg());
    ASSERT_EQ(segment_manager.StartBlock(segno), expected_addr);
  }
}

TEST(SegmentManagerExceptionTest, InvalidCheckpointSegno) TA_NO_THREAD_SAFETY_ANALYSIS {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDevWithOptions(&bc, MkfsOptions{});

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  auto superblock = LoadSuperblock(*bc);
  ASSERT_TRUE(superblock.is_ok());

  // Create a vfs object for unit tests.
  auto vfs_or = Runner::CreateRunner(loop.dispatcher());
  ASSERT_TRUE(vfs_or.is_ok());
  std::unique_ptr<F2fs> fs =
      std::make_unique<F2fs>(loop.dispatcher(), std::move(bc), MountOptions{}, (*vfs_or).get());

  ASSERT_EQ(fs->LoadSuper(std::move(*superblock)), ZX_OK);

  // Corrupt checkpoint with an invalid segment number.
  Checkpoint &ckpt = *fs->GetSuperblockInfo().GetCheckpointBlock();
  ckpt.cur_node_segno[0] = CpuToLe(fs->GetSegmentManager().TotalSegs() + 100);

  fs->GetSegmentManager().DestroySegmentManager();
  ASSERT_EQ(fs->GetSegmentManager().BuildSegmentManager(), ZX_ERR_INVALID_ARGS);

  fs->GetVCache().Reset();
  fs->Reset();
}

TEST(SegmentManagerStaticTest, IsValidSegmentType) {
  EXPECT_TRUE(SegmentManager::IsValidSegmentType(CursegType::kCursegHotData));
  EXPECT_TRUE(SegmentManager::IsValidSegmentType(CursegType::kCursegWarmData));
  EXPECT_TRUE(SegmentManager::IsValidSegmentType(CursegType::kCursegColdData));
  EXPECT_TRUE(SegmentManager::IsValidSegmentType(CursegType::kCursegHotNode));
  EXPECT_TRUE(SegmentManager::IsValidSegmentType(CursegType::kCursegWarmNode));
  EXPECT_TRUE(SegmentManager::IsValidSegmentType(CursegType::kCursegColdNode));

  EXPECT_FALSE(SegmentManager::IsValidSegmentType(CursegType::kNoCheckType));
  EXPECT_FALSE(SegmentManager::IsValidSegmentType(static_cast<CursegType>(256)));
  EXPECT_FALSE(SegmentManager::IsValidSegmentType(static_cast<CursegType>(512)));
  EXPECT_FALSE(SegmentManager::IsValidSegmentType(static_cast<CursegType>(-1)));
  EXPECT_FALSE(SegmentManager::IsValidSegmentType(static_cast<uint8_t>(kNrCursegType)));
  EXPECT_FALSE(SegmentManager::IsValidSegmentType(static_cast<uint8_t>(kCurSegNull)));
  EXPECT_FALSE(SegmentManager::IsValidSegmentType(uint8_t{255}));
}

TEST_F(SegmentManagerTest, CheckBlockCountInvalidType) {
  SitEntry sit{};
  sit.vblocks = CpuToLe(static_cast<uint16_t>(kNrCursegType << kSitVblocksShift));
  ASSERT_DEATH(fs_->GetSegmentManager().CheckBlockCount(0, sit), "");

  SitEntry sit_null{};
  sit_null.vblocks = CpuToLe(static_cast<uint16_t>(kCurSegNull << kSitVblocksShift));
  ASSERT_DEATH(fs_->GetSegmentManager().CheckBlockCount(0, sit_null), "");
}

}  // namespace
}  // namespace f2fs
