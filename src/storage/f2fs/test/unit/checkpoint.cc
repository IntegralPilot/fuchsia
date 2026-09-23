// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>
#include <lib/zircon-internal/thread_annotations.h>

#include <random>
#include <vector>

#include <gtest/gtest.h>

#include "safemath/safe_conversions.h"
#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

using CheckpointCallback =
    fit::function<void(uint32_t expect_cp_position, uint32_t expect_cp_ver, bool after_mkfs)>;
using block_client::FakeBlockDevice;

constexpr uint64_t kBlockCount = 2097152;  // 1GB for SIT Bitmap TC
constexpr uint32_t kCheckpointPack0 = 0;
constexpr uint32_t kCheckpointPack1 = 1;
constexpr uint32_t kCheckpointPackCount = 2;
constexpr uint32_t kMkfsCheckpointVersion = 1;
constexpr uint32_t kFirstCheckpointVersion = 2;
constexpr uint32_t kCheckpointLoopCnt = 4;
constexpr uint8_t kRootDirNatBit = 0x80;
constexpr uint32_t kMapPerSitEntry = kSitVBlockMapSize << kShiftForBitSize;
constexpr uint32_t kOrphanInodeBlockCnt = 10;
constexpr uint32_t kRootInodeNid = 3;
constexpr size_t kStartSegmentOfWarmData = 5;

class CheckpointTest : public F2fsFakeDevTestFixture {
 public:
  CheckpointTest()
      : F2fsFakeDevTestFixture(TestOptions{
            .block_count = kBlockCount, .mount_options = {{MountOption::kDisableRollForward, 1}}}) {
  }

 protected:
  void DoFirstCheckpoint(CheckpointCallback &callback) {
    callback(checkpoint_pack_, kMkfsCheckpointVersion, true);

    ASSERT_EQ(checkpoint_pack_, kCheckpointPack0);
    checkpoint_pack_ = kCheckpointPack1;
  }

  void DoCheckpoints(CheckpointCallback &callback, uint32_t loop_cnt) {
    for (uint32_t cp_version = kFirstCheckpointVersion; cp_version <= loop_cnt + 1; ++cp_version) {
      fs_->Sync();

      callback(checkpoint_pack_, cp_version, false);

      if (checkpoint_pack_ == kCheckpointPack0) {
        checkpoint_pack_ = kCheckpointPack1;
      } else {
        checkpoint_pack_ = kCheckpointPack0;
      }
    }
  }

  void ReadCheckpoint(block_t cp_addr, LockedPage *cp_out) {
    LockedPage cp_page[kCheckpointPackCount];  // cp_page[0]: header, cp_page[1]: footer
    uint64_t version[kCheckpointPackCount];    // version[0]: header, version[1]: footer

    for (uint32_t i = kCheckpointPack0; i <= kCheckpointPack1; ++i) {
      Checkpoint *cp_block;
      uint32_t crc, crc_offset;

      // Read checkpoint pack header/footer
      fs_->GetMetaPage(cp_addr, &cp_page[i]);
      ASSERT_NE(cp_page[i], nullptr);
      // Check header CRC
      cp_block = cp_page[i]->GetAddress<Checkpoint>();
      ASSERT_NE(cp_block, nullptr);
      crc_offset = LeToCpu(cp_block->checksum_offset);
      ASSERT_LT(crc_offset, fs_->GetSuperblockInfo().GetBlocksize());
      crc = *reinterpret_cast<uint32_t *>(reinterpret_cast<uint8_t *>(cp_block) + crc_offset);
      ASSERT_TRUE(F2fsCrcValid(crc, cp_block, crc_offset));

      // Get the version number
      version[i] = LeToCpu(cp_block->checkpoint_ver);

      // Read checkpoint pack footer
      cp_addr += LeToCpu(cp_block->cp_pack_total_block_count) - 1;
    }
    ASSERT_EQ(version[0], version[1]);

    *cp_out = std::move(cp_page[0]);
  }

  void GetLastCheckpoint(uint32_t expect_cp_position, bool after_mkfs, fbl::RefPtr<Page> *cp_out) {
    const Superblock &raw_superblock = fs_->GetSuperblockInfo().GetSuperblock();
    Checkpoint *cp_block1 = nullptr, *cp_block2 = nullptr;
    LockedPage cp_page1, cp_page2;

    block_t cp_addr = LeToCpu(raw_superblock.cp_blkaddr);
    ReadCheckpoint(cp_addr, &cp_page1);
    cp_block1 = cp_page1->GetAddress<Checkpoint>();

    if (!after_mkfs) {
      cp_addr += 1 << LeToCpu(raw_superblock.log_blocks_per_seg);
      ReadCheckpoint(cp_addr, &cp_page2);
      cp_block2 = cp_page2->GetAddress<Checkpoint>();
    }

    if (after_mkfs) {
      *cp_out = cp_page1.CopyRefPtr();
    } else if (cp_block1 && cp_block2) {
      if (VerAfter(cp_block2->checkpoint_ver, cp_block1->checkpoint_ver)) {
        *cp_out = cp_page2.CopyRefPtr();
        ASSERT_EQ(cp_block1->checkpoint_ver, cp_block2->checkpoint_ver - 1);
      } else {
        *cp_out = cp_page1.CopyRefPtr();
        ASSERT_EQ(cp_block2->checkpoint_ver, cp_block1->checkpoint_ver - 1);
      }
    }
  }

  void CreateDirs(int dir_cnt, uint64_t version) {
    auto root_or = fs_->GetVnode(fs_->GetSuperblockInfo().GetRootIno());
    ASSERT_TRUE(root_or.is_ok());
    Dir *root_dir = fbl::RefPtr<Dir>::Downcast(*root_or).get();

    for (int i = 0; i < dir_cnt; ++i) {
      std::string filename = "dir_" + std::to_string(version) + "_" + std::to_string(i);
      zx::result vnode = root_dir->Create(filename, fs::CreationType::kDirectory);
      ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
      vnode->Close();
    }
  }

  void CreateFiles(int file_cnt, uint64_t version) {
    auto root_or = fs_->GetVnode(fs_->GetSuperblockInfo().GetRootIno());
    ASSERT_TRUE(root_or.is_ok());
    Dir *root_dir = fbl::RefPtr<Dir>::Downcast(*root_or).get();

    for (int i = 0; i < file_cnt; ++i) {
      std::string filename = "file_" + std::to_string(version) + "_" + std::to_string(i);
      zx::result vnode = root_dir->Create(filename, fs::CreationType::kFile);
      ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
      vnode->Close();
    }
  }

  bool IsRootInode(CursegType curseg_type, uint32_t offset) {
    return (curseg_type == CursegType::kCursegHotData ||
            curseg_type == CursegType::kCursegHotNode) &&
           offset == 0;
  }

  std::vector<fbl::RefPtr<VnodeF2fs>> &GetVnodes() { return vnodes_; }
  std::vector<uint32_t> &GetPrevValues() { return prev_values_; }

 private:
  uint32_t checkpoint_pack_ = kCheckpointPack0;
  std::vector<fbl::RefPtr<VnodeF2fs>> vnodes_;
  std::vector<uint32_t> prev_values_;
};

TEST_F(CheckpointTest, Version) {
  CheckpointCallback check_version = [this](uint32_t expect_cp_position, uint32_t expect_cp_ver,
                                            bool after_mkfs) {
    fbl::RefPtr<Page> cp_page;

    GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
    Checkpoint *cp = cp_page->GetAddress<Checkpoint>();

    ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);
  };

  DoFirstCheckpoint(check_version);
  DoCheckpoints(check_version, kCheckpointLoopCnt);
}

TEST_F(CheckpointTest, NatBitmap) {
  DisableFsck();

  CheckpointCallback check_nat_bitmap = [this](uint32_t expect_cp_position, uint32_t expect_cp_ver,
                                               bool after_mkfs) {
    fbl::RefPtr<Page> cp_page;

    // 1. Get the latest checkpoint pack
    GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
    Checkpoint *cp = cp_page->GetAddress<Checkpoint>();
    ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);

    // 2. Get NAT version bitmap
    uint8_t *version_bitmap = cp->sit_nat_version_bitmap + cp->sit_ver_bitmap_bytesize;
    ASSERT_NE(version_bitmap, nullptr);

    // 3. Check if every checkpoint write flips the bit for the root inode.
    ASSERT_EQ((static_cast<uint8_t *>(version_bitmap))[0] & kRootDirNatBit,
              cp->checkpoint_ver % 2 ? 0x00 : kRootDirNatBit);
    // 4. Check if the bit for the last written NAT block is set after checkpoint.
    if (!after_mkfs) {
      auto cur_nat_block = cp->checkpoint_ver - kFirstCheckpointVersion;
      ASSERT_TRUE((static_cast<uint8_t *>(version_bitmap))[cur_nat_block / kBitsPerByte] &
                  1 << ToMsbFirst(cur_nat_block));
    }

    // 5. Creates as many inodes as a NAT block can hold in root dir.
    constexpr int kMkfsNidCount = 4;
    constexpr int kNidCountForMakeOneDirtyNATBlock = 455;
    constexpr int kDirCount = 5;
    ASSERT_TRUE(kNatEntryPerBlock == kNidCountForMakeOneDirtyNATBlock);

    CreateDirs(kDirCount, cp->checkpoint_ver);
    int file_count = after_mkfs ? (kNidCountForMakeOneDirtyNATBlock - kMkfsNidCount - kDirCount)
                                : (kNidCountForMakeOneDirtyNATBlock - kDirCount);
    CreateFiles(file_count, cp->checkpoint_ver);
  };

  DoFirstCheckpoint(check_nat_bitmap);
  DoCheckpoints(check_nat_bitmap, kCheckpointLoopCnt);
}

TEST_F(CheckpointTest, SitBitmap) {
  DisableFsck();

  CheckpointCallback check_sit_bitmap = [this](uint32_t expect_cp_position, uint32_t expect_cp_ver,
                                               bool after_mkfs) {
    fbl::RefPtr<Page> cp_page;

    // Get the latest checkpoint pack
    GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
    Checkpoint *cp = cp_page->GetAddress<Checkpoint>();
    ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);

    uint32_t expected = after_mkfs
                            ? kStartSegmentOfWarmData
                            : static_cast<uint32_t>(cp->checkpoint_ver - 1) * kSitEntryPerBlock +
                                  kStartSegmentOfWarmData;
    // Ensure that |expected| does not exceed underlying disk size.
    ASSERT_TRUE(cp->sit_ver_bitmap_bytesize > expected / kBitsPerByte);
    if (!after_mkfs) {
      uint32_t expected_cur_sit_block = expected / kSitEntryPerBlock;
      uint8_t *version_bitmap = cp->sit_nat_version_bitmap;
      ASSERT_NE(version_bitmap, nullptr);
      ASSERT_TRUE(static_cast<uint8_t *>(version_bitmap)[expected_cur_sit_block / kBitsPerByte] &
                  1 << ToMsbFirst(expected_cur_sit_block));
    }

    for (uint32_t i = 0; i < kMapPerSitEntry * kSitEntryPerBlock; ++i) {
      block_t new_blkaddr;
      if (i && i % kMapPerSitEntry == 0) {
        ++expected;
      }
      MapTester::DoWriteSit(fs_.get(), CursegType::kCursegWarmData, expected, &new_blkaddr);
    }
  };

  DoFirstCheckpoint(check_sit_bitmap);
  DoCheckpoints(check_sit_bitmap, kCheckpointLoopCnt);
}

TEST_F(CheckpointTest, AddOrphanInode) {
  CheckpointCallback check_add_orphan_inode = [this](uint32_t expect_cp_position,
                                                     uint32_t expect_cp_ver, bool after_mkfs) {
    fbl::RefPtr<Page> cp_page;

    // 1. Get last checkpoint
    GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
    Checkpoint *cp = cp_page->GetAddress<Checkpoint>();
    ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);

    uint32_t orphan_inos = kOrphansPerBlock * kOrphanInodeBlockCnt;

    if (!after_mkfs) {
      auto start_ino = (cp->checkpoint_ver - kMkfsCheckpointVersion) * orphan_inos;
      // 2. Get orphan inodes
      std::vector<uint32_t> cp_inos;
      std::vector<uint32_t> exp_inos(orphan_inos);
      std::iota(exp_inos.begin(), exp_inos.end(), start_ino);

      pgoff_t start_blk = cp_page->GetIndex() + 1;
      block_t orphan_blkaddr = cp->cp_pack_start_sum - 1;

      ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpOrphanPresentFlag));

      for (auto ino : exp_inos) {
        fs_->RemoveFromVnodeSet(VnodeSet::kOrphan, ino);
      }

      for (block_t i = 0; i < orphan_blkaddr; ++i) {
        LockedPage page;
        fs_->GetMetaPage(start_blk + i, &page);
        OrphanBlock *orphan_blk;

        orphan_blk = page->GetAddress<OrphanBlock>();
        for (block_t j = 0; j < LeToCpu(orphan_blk->entry_count); ++j) {
          nid_t ino = LeToCpu(orphan_blk->ino[j]);
          cp_inos.push_back(ino);
        }
      }

      // 3. Check orphan inodes
      ASSERT_TRUE(std::equal(exp_inos.begin(), exp_inos.end(), cp_inos.begin()));
    }

    if (cp->checkpoint_ver > kCheckpointLoopCnt) {
      return;
    }

    // 4. Add shuffled orphan inodes for next checkpoint
    std::vector<uint32_t> inos(orphan_inos);
    auto start_ino = cp->checkpoint_ver * orphan_inos;
    std::iota(inos.begin(), inos.end(), start_ino);

    std::shuffle(inos.begin(), inos.end(),
                 std::default_random_engine(static_cast<uint32_t>(cp->checkpoint_ver)));

    for (auto ino : inos) {
      fs_->AddToVnodeSet(VnodeSet::kOrphan, ino);
    }

    ASSERT_EQ(fs_->GetVnodeSetSize(VnodeSet::kOrphan), orphan_inos);

    // Add duplicate orphan inodes
    constexpr uint32_t kGapBetweenTargetInos = 10;
    std::vector<uint32_t> dup_inos(orphan_inos / kGapBetweenTargetInos);
    std::generate(dup_inos.begin(), dup_inos.end(), [n = start_ino]() mutable {
      n += kGapBetweenTargetInos;
      return n - kGapBetweenTargetInos;
    });

    for (auto ino : dup_inos) {
      fs_->AddToVnodeSet(VnodeSet::kOrphan, ino);
    }
  };

  DoFirstCheckpoint(check_add_orphan_inode);
  DoCheckpoints(check_add_orphan_inode, kCheckpointLoopCnt);
}

TEST_F(CheckpointTest, RemoveOrphanInode) {
  CheckpointCallback check_remove_orphan_inode = [this](uint32_t expect_cp_position,
                                                        uint32_t expect_cp_ver, bool after_mkfs) {
    fbl::RefPtr<Page> cp_page;
    SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();

    // 1. Get last checkpoint
    GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
    Checkpoint *cp = cp_page->GetAddress<Checkpoint>();
    ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);

    uint32_t orphan_inos = kOrphansPerBlock * kOrphanInodeBlockCnt;
    constexpr uint32_t kGapBetweenTargetInos = 10;

    if (!after_mkfs) {
      auto start_ino = (cp->checkpoint_ver - kMkfsCheckpointVersion) * orphan_inos;
      // 2. Get orphan inodes
      std::vector<uint32_t> cp_inos;
      std::vector<uint32_t> exp_inos(orphan_inos);
      std::iota(exp_inos.begin(), exp_inos.end(), start_ino);

      // Remove exp orphan inodes
      for (int i = safemath::checked_cast<int>(orphan_inos / kGapBetweenTargetInos - 1); i >= 0;
           --i) {
        uint32_t offset = i * kGapBetweenTargetInos;
        exp_inos.erase(exp_inos.begin() + offset);
      }

      pgoff_t start_blk = cp_page->GetIndex() + 1;
      block_t orphan_blkaddr = cp->cp_pack_start_sum - 1;

      ASSERT_TRUE(superblock_info.TestCpFlags(CpFlag::kCpOrphanPresentFlag));

      for (block_t i = 0; i < orphan_blkaddr; ++i) {
        LockedPage page;
        fs_->GetMetaPage(start_blk + i, &page);
        OrphanBlock *orphan_blk;

        orphan_blk = page->GetAddress<OrphanBlock>();
        for (block_t j = 0; j < LeToCpu(orphan_blk->entry_count); ++j) {
          nid_t ino = LeToCpu(orphan_blk->ino[j]);
          cp_inos.push_back(ino);
          fs_->RemoveFromVnodeSet(VnodeSet::kOrphan, ino);
        }
      }

      // 3. Check orphan inodes
      ASSERT_TRUE(std::equal(exp_inos.begin(), exp_inos.end(), cp_inos.begin()));
    }

    // 4. Add shuffled orphan inodes for next checkpoint
    std::vector<uint32_t> inos(orphan_inos);
    auto start_ino = cp->checkpoint_ver * orphan_inos;
    std::iota(inos.begin(), inos.end(), start_ino);

    std::shuffle(inos.begin(), inos.end(),
                 std::default_random_engine(static_cast<uint32_t>(cp->checkpoint_ver)));

    if (cp->checkpoint_ver <= kCheckpointLoopCnt) {
      for (auto ino : inos) {
        fs_->AddToVnodeSet(VnodeSet::kOrphan, ino);
      }
      ASSERT_EQ(fs_->GetVnodeSetSize(VnodeSet::kOrphan), orphan_inos);
    }

    // 5. Remove orphan inodes
    std::vector<uint32_t> rm_inos(orphan_inos / kGapBetweenTargetInos);
    std::generate(rm_inos.begin(), rm_inos.end(), [n = start_ino]() mutable {
      n += kGapBetweenTargetInos;
      return n - kGapBetweenTargetInos;
    });

    for (auto ino : rm_inos) {
      fs_->RemoveFromVnodeSet(VnodeSet::kOrphan, ino);
    }
  };

  DoFirstCheckpoint(check_remove_orphan_inode);
  DoCheckpoints(check_remove_orphan_inode, kCheckpointLoopCnt);
}

TEST_F(CheckpointTest, PurgeOrphanInode) {
  CheckpointCallback check_recover_orphan_inode =
      [this](uint32_t expect_cp_position, uint32_t expect_cp_ver, bool after_mkfs)
          TA_NO_THREAD_SAFETY_ANALYSIS {
            fbl::RefPtr<Page> cp_page;
            SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();

            // 1. Get last checkpoint
            GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
            Checkpoint *cp = cp_page->GetAddress<Checkpoint>();
            ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);

            uint32_t orphan_inos = kOrphansPerBlock;
            auto &vnodes = GetVnodes();
            if (!after_mkfs) {
              // Reset FileCache for node pages
              fs_->GetNodeVnode().CleanupCache();
              ASSERT_TRUE(superblock_info.TestCpFlags(CpFlag::kCpOrphanPresentFlag));
              ASSERT_EQ(vnodes.size(), orphan_inos);

              std::vector<nid_t> inos;
              for (auto &vnode : vnodes) {
                ASSERT_EQ(vnode->GetLinkCount(), (uint32_t)1);
                inos.push_back(vnode->GetKey());
              }
              vnodes.clear();
              vnodes.shrink_to_fit();

              // Check device peer closed exception
              {
                auto hook = [](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
                  if (_req.command.opcode == BLOCK_OPCODE_READ) {
                    return ZX_ERR_PEER_CLOSED;
                  }
                  return ZX_OK;
                };
                DeviceTester::SetHook(fs_.get(), hook);
                ASSERT_EQ(fs_->PurgeOrphanInodes(), ZX_ERR_PEER_CLOSED);
                ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

                DeviceTester::SetHook(fs_.get(), nullptr);
                superblock_info.ClearCpFlags(CpFlag::kCpErrorFlag);
              }
              ASSERT_EQ(fs_->PurgeOrphanInodes(), 0);

              for (ino_t ino : inos) {
                fbl::RefPtr<VnodeF2fs> vnode;
                ASSERT_EQ(fs_->GetVCache().Lookup(ino, &vnode), ZX_ERR_NOT_FOUND);
                fs_->RemoveFromVnodeSet(VnodeSet::kOrphan, ino);
              }
            }

            if (cp->checkpoint_ver > kCheckpointLoopCnt) {
              return;
            }

            // 3. Add shuffled orphan inodes for next checkpoint
            std::vector<uint32_t> inos(orphan_inos);
            auto start_ino = cp->checkpoint_ver * orphan_inos;
            std::iota(inos.begin(), inos.end(), start_ino);

            std::shuffle(inos.begin(), inos.end(),
                         std::default_random_engine(static_cast<uint32_t>(cp->checkpoint_ver)));

            for (auto ino : inos) {
              fbl::RefPtr<VnodeF2fs> vnode =
                  fbl::MakeRefCounted<File>(fs_.get(), ino, static_cast<umode_t>(S_IFREG));
              vnode->ClearFlag(InodeInfoFlag::kNewInode);
              fs_->GetVCache().Add(vnode.get());
              vnodes.push_back(std::move(vnode));
              fs_->AddToVnodeSet(VnodeSet::kOrphan, ino);
            }

            ASSERT_EQ(fs_->GetVnodeSetSize(VnodeSet::kOrphan), orphan_inos);
          };

  DoFirstCheckpoint(check_recover_orphan_inode);
  DoCheckpoints(check_recover_orphan_inode, kCheckpointLoopCnt);
}

TEST_F(CheckpointTest, CompactedSummaries) TA_NO_THREAD_SAFETY_ANALYSIS {
  CheckpointCallback check_compacted_summaries = [this](
                                                     uint32_t expect_cp_position,
                                                     uint32_t expect_cp_ver,
                                                     bool after_mkfs) TA_NO_THREAD_SAFETY_ANALYSIS {
    DisableFsck();

    fbl::RefPtr<Page> cp_page;
    SegmentManager &segment_manager = fs_->GetSegmentManager();

    // 1. Get last checkpoint
    GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
    Checkpoint *cp = cp_page->GetAddress<Checkpoint>();
    ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);

    if (!after_mkfs) {
      // 2. Clear current segment summaries
      for (int i = static_cast<int>(CursegType::kCursegHotData);
           i <= static_cast<int>(CursegType::kCursegColdData); ++i) {
        CursegInfo *curseg = segment_manager.CURSEG_I(static_cast<CursegType>(i));
        for (auto &entrie : curseg->sum_blk->entries) {
          entrie.nid = 0;
          entrie.version = 0;
          entrie.ofs_in_node = 0;
        }
      }

      // 3. Recover compacted data summaries
      ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpCompactSumFlag));
      ASSERT_EQ(segment_manager.ReadCompactedSummaries(), ZX_OK);

      // 4. Check recovered active summary info
      for (int i = static_cast<int>(CursegType::kCursegHotData);
           i <= static_cast<int>(CursegType::kCursegColdData); ++i) {
        CursegInfo *curseg = segment_manager.CURSEG_I(static_cast<CursegType>(i));
        ASSERT_EQ(curseg->next_blkoff, kEntriesInSum / 2);

        for (uint32_t j = 0; j < kEntriesInSum / 2; ++j) {
          if (cp->checkpoint_ver == kFirstCheckpointVersion &&
              IsRootInode(static_cast<CursegType>(i), j)) {  // root inode dentry
            continue;
          }

          nid_t nid = curseg->sum_blk->entries[j].nid;
          ASSERT_EQ(nid, kRootInodeNid);
          ASSERT_EQ(static_cast<uint64_t>(curseg->sum_blk->entries[j].version),
                    cp->checkpoint_ver - kMkfsCheckpointVersion);
          uint16_t ofs_in_node = curseg->sum_blk->entries[j].ofs_in_node;
          ASSERT_EQ(ofs_in_node, j);
        }
      }
    }

    // 5. Fill compact data summary
    // Close and change current active segment
    // Fill current active segments for compacted data summaries
    for (int i = static_cast<int>(CursegType::kCursegHotData);
         i <= static_cast<int>(CursegType::kCursegColdData); ++i) {
      // Close previous segment
      if (!after_mkfs) {
        for (uint32_t j = 0; j < kEntriesInSum / 2; ++j) {
          block_t new_blkaddr;
          MapTester::DoWriteSit(fs_.get(), static_cast<CursegType>(i), kNullSegNo, &new_blkaddr);
        }
      }

      // Write workload
      for (uint16_t j = 0; j < kEntriesInSum / 2; ++j) {
        block_t new_blkaddr;
        Summary sum;

        if (cp->checkpoint_ver == kMkfsCheckpointVersion &&
            IsRootInode(static_cast<CursegType>(i), j)) {  // root inode dentry
          continue;
        }

        SetSummary(&sum, 3, j, static_cast<uint8_t>(cp->checkpoint_ver));
        segment_manager.AddSumEntry(static_cast<CursegType>(i), &sum, j);

        MapTester::DoWriteSit(fs_.get(), static_cast<CursegType>(i), kNullSegNo, &new_blkaddr);
      }
    }
    // Compact summary page count must less than nomal summary page count(3).
    // If compact summary page count exeeds 2, it will be changed to normal summary.
    constexpr int kMinNormalSummaryPageCount = 3;
    ASSERT_LT(segment_manager.NpagesForSummaryFlush(), kMinNormalSummaryPageCount);
  };

  DoFirstCheckpoint(check_compacted_summaries);
  DoCheckpoints(check_compacted_summaries, kCheckpointLoopCnt);
}

TEST_F(CheckpointTest, NormalSummaries) TA_NO_THREAD_SAFETY_ANALYSIS {
  CheckpointCallback check_normal_summaries = [this](uint32_t expect_cp_position,
                                                     uint32_t expect_cp_ver,
                                                     bool after_mkfs) TA_NO_THREAD_SAFETY_ANALYSIS {
    DisableFsck();

    fbl::RefPtr<Page> cp_page;
    SegmentManager &segment_manager = fs_->GetSegmentManager();

    // 1. Get last checkpoint
    GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
    Checkpoint *cp = cp_page->GetAddress<Checkpoint>();
    ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);

    if (!after_mkfs) {
      // 2. Clear current segment summaries
      for (int i = static_cast<int>(CursegType::kCursegHotData);
           i <= static_cast<int>(CursegType::kCursegColdNode); ++i) {
        CursegInfo *curseg = segment_manager.CURSEG_I(static_cast<CursegType>(i));
        for (auto &entrie : curseg->sum_blk->entries) {
          entrie.nid = 0;
          entrie.version = 0;
          entrie.ofs_in_node = 0;
        }
      }

      // 2. Recover normal data summary
      ASSERT_FALSE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpCompactSumFlag));
      for (int type = static_cast<int>(CursegType::kCursegHotData);
           type <= static_cast<int>(CursegType::kCursegColdNode); ++type) {
        ASSERT_EQ(segment_manager.ReadNormalSummaries(type), ZX_OK);
      }

      // 4. Check recovered active summary info
      for (int i = static_cast<int>(CursegType::kCursegHotData);
           i <= static_cast<int>(CursegType::kCursegColdNode); ++i) {
        CursegInfo *curseg = segment_manager.CURSEG_I(static_cast<CursegType>(i));
        ASSERT_EQ(curseg->next_blkoff, kEntriesInSum - 1);

        for (uint32_t j = 0; j < kEntriesInSum - 1; ++j) {
          if (cp->checkpoint_ver == kFirstCheckpointVersion &&
              IsRootInode(static_cast<CursegType>(i), j)) {  // root inode
            continue;
          }

          nid_t nid = curseg->sum_blk->entries[j].nid;
          ASSERT_EQ(nid, cp->checkpoint_ver - kMkfsCheckpointVersion);
          if (!IsNodeSeg(static_cast<CursegType>(i))) {
            ASSERT_EQ(static_cast<uint64_t>(curseg->sum_blk->entries[j].version),
                      cp->checkpoint_ver - 1);
            uint16_t ofs_in_node = curseg->sum_blk->entries[j].ofs_in_node;
            ASSERT_EQ(ofs_in_node, j);
          }
        }
      }
    }

    // 3. Fill normal data summary
    // Close and change current active segment
    // Fill current active segments for normal summaries
    for (int i = static_cast<int>(CursegType::kCursegHotData);
         i <= static_cast<int>(CursegType::kCursegColdNode); ++i) {
      // Close previous segment
      if (!after_mkfs) {
        block_t new_blkaddr;
        MapTester::DoWriteSit(fs_.get(), static_cast<CursegType>(i), kNullSegNo, &new_blkaddr);
      }

      for (uint16_t j = 0; j < kEntriesInSum - 1; ++j) {
        block_t new_blkaddr;
        Summary sum;

        if (cp->checkpoint_ver == kMkfsCheckpointVersion &&
            IsRootInode(static_cast<CursegType>(i), j)) {
          continue;
        }

        SetSummary(&sum, static_cast<nid_t>(cp->checkpoint_ver), j,
                   static_cast<uint8_t>(cp->checkpoint_ver));
        segment_manager.AddSumEntry(static_cast<CursegType>(i), &sum, j);

        MapTester::DoWriteSit(fs_.get(), static_cast<CursegType>(i), kNullSegNo, &new_blkaddr);
      }
    }
    // Normal summary page count must more than compact page count(2).
    // If compact summary pages count exeeds 2, it will be changed to normal summary.
    constexpr int kMinNormalSummaryPageCount = 3;
    ASSERT_GE(segment_manager.NpagesForSummaryFlush(), kMinNormalSummaryPageCount);
  };

  DoFirstCheckpoint(check_normal_summaries);
  DoCheckpoints(check_normal_summaries, kCheckpointLoopCnt);
}

TEST_F(CheckpointTest, SitJournal) TA_NO_THREAD_SAFETY_ANALYSIS {
  DisableFsck();

  CheckpointCallback check_sit_journal = [this](uint32_t expect_cp_position, uint32_t expect_cp_ver,
                                                bool after_mkfs) TA_NO_THREAD_SAFETY_ANALYSIS {
    fbl::RefPtr<Page> cp_page;
    SegmentManager &segment_manager = fs_->GetSegmentManager();
    auto &segnos = GetPrevValues();

    // 1. Get last checkpoint
    GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
    Checkpoint *cp = cp_page->GetAddress<Checkpoint>();
    ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);

    if (!after_mkfs) {
      // 2. Recover compacted data summaries
      ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpCompactSumFlag));
      ASSERT_EQ(segment_manager.ReadCompactedSummaries(), ZX_OK);

      // 3. Check recovered journal
      CursegInfo *curseg = segment_manager.CURSEG_I(CursegType::kCursegColdData);

      SummaryBlock *sum = &curseg->sum_blk;
      for (int i = 0; i < SitsInCursum(*sum); ++i) {
        uint32_t segno = LeToCpu(SegnoInJournal(*sum, i));
        ASSERT_EQ(segno, segnos[i]);
      }
    }

    // 4. Fill compact data summary
    if (!after_mkfs) {
      CursegInfo *curseg = segment_manager.CURSEG_I(CursegType::kCursegColdData);

      // Clear SIT journal
      if (SitsInCursum(*curseg->sum_blk) >= static_cast<int>(kSitJournalEntries)) {
        SitInfo &sit_i = segment_manager.GetSitInfo();
        RawBitmap &bitmap = sit_i.dirty_sentries_bitmap;
        block_t nsegs = segment_manager.TotalSegs();

        // Add dummy dirty sentries
        for (uint32_t i = 0; i < kMapPerSitEntry; ++i) {
          block_t new_blkaddr;
          MapTester::DoWriteSit(fs_.get(), CursegType::kCursegColdData, kNullSegNo, &new_blkaddr);
        }

        // Move journal sentries to dirty sentries
        ASSERT_TRUE(segment_manager.FlushSitsInJournal());

        // Clear dirty sentries
        uint32_t segno = 0;
        size_t out;
        while (!bitmap.Scan(segno + 1, nsegs, false, &out)) {
          segno = safemath::checked_cast<uint32_t>(out);
          bitmap.ClearOne(segno);
          --sit_i.dirty_sentries;
        }
      }
    }
    segnos.clear();
    segnos.shrink_to_fit();

    // Fill SIT journal
    for (uint32_t i = 0; i < kSitJournalEntries * kMapPerSitEntry; ++i) {
      block_t new_blkaddr;
      MapTester::DoWriteSit(fs_.get(), CursegType::kCursegColdData, kNullSegNo, &new_blkaddr);
      CursegInfo *curseg = segment_manager.CURSEG_I(CursegType::kCursegColdData);
      if (curseg->next_blkoff == 1) {
        segnos.push_back(curseg->segno);
      }
    }
    ASSERT_LT(segment_manager.NpagesForSummaryFlush(), 3);
  };

  DoFirstCheckpoint(check_sit_journal);
  DoCheckpoints(check_sit_journal, kCheckpointLoopCnt);
}

TEST_F(CheckpointTest, NatJournal) TA_NO_THREAD_SAFETY_ANALYSIS {
  DisableFsck();

  CheckpointCallback check_nat_journal = [this](uint32_t expect_cp_position, uint32_t expect_cp_ver,
                                                bool after_mkfs) TA_NO_THREAD_SAFETY_ANALYSIS {
    SuperblockInfo &superblock_info = fs_->GetSuperblockInfo();
    NodeManager &node_manager = fs_->GetNodeManager();
    SegmentManager &segment_manager = fs_->GetSegmentManager();
    CursegInfo *curseg = fs_->GetSegmentManager().CURSEG_I(CursegType::kCursegHotData);
    fbl::RefPtr<Page> cp_page;
    auto &nids = GetPrevValues();

    // 1. Get last checkpoint
    GetLastCheckpoint(expect_cp_position, after_mkfs, &cp_page);
    Checkpoint *cp = cp_page->GetAddress<Checkpoint>();
    ASSERT_EQ(cp->checkpoint_ver, expect_cp_ver);

    uint32_t dummy_nid_count = static_cast<uint32_t>(cp->checkpoint_ver) - 2;
    uint32_t nid_offset = (kRootInodeNid + 1) +
                          (kNatJournalEntries * (static_cast<uint32_t>(cp->checkpoint_ver) - 1)) +
                          dummy_nid_count;

    if (!after_mkfs) {
      // 2. Recover compacted data summaries
      ASSERT_TRUE(superblock_info.TestCpFlags(CpFlag::kCpCompactSumFlag));
      ASSERT_EQ(segment_manager.ReadCompactedSummaries(), ZX_OK);

      // 3. Check recovered journal
      SummaryBlock *sum = &curseg->sum_blk;
      for (int i = 0; i < NatsInCursum(*sum); ++i) {
        ASSERT_EQ(NidInJournal(*sum, i), nids[i]);
        ASSERT_EQ(NatInJournal(*sum, i).version, cp->checkpoint_ver - kMkfsCheckpointVersion);
      }

      // 4. Fill compact data summary
      // Clear NAT journal
      if (NatsInCursum(*curseg->sum_blk) >= static_cast<int>(kNatJournalEntries)) {
        // Add dummy dirty NAT entries
        MapTester::DoWriteNat(fs_.get(), nid_offset, nid_offset,
                              static_cast<uint8_t>(cp->checkpoint_ver));

        // Move journal sentries to dirty sentries
        ASSERT_TRUE(node_manager.FlushNatsInJournal());

        // Flush NAT cache
        MapTester::RemoveAllNatEntries(node_manager);
      }
    }
    nids.clear();
    nids.shrink_to_fit();

    // Fill NAT journal
    nid_offset += 1;
    for (uint32_t i = nid_offset; i < nid_offset + kNatJournalEntries; ++i) {
      MapTester::DoWriteNat(fs_.get(), i, i, static_cast<uint8_t>(cp->checkpoint_ver));
      nids.push_back(i);
    }
    ASSERT_LT(segment_manager.NpagesForSummaryFlush(), 3);
  };

  DoFirstCheckpoint(check_nat_journal);
  DoCheckpoints(check_nat_journal, kCheckpointLoopCnt);
}

TEST(CheckpointUnmountTest, UmountFlag) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  // create f2fs and root dir
  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);
  SuperblockInfo &superblock_info = fs->GetSuperblockInfo();

  // read the node block where the root inode is stored
  {
    LockedPage root_node_page;
    fs->GetNodeManager().GetNodePage(superblock_info.GetRootIno(), &root_node_page);
    ASSERT_TRUE(root_node_page);
  }

  ASSERT_EQ(root->Close(), ZX_OK);
  root = nullptr;

  fs->Sync();
  FileTester::SuddenPowerOff(std::move(fs), &bc);

  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);
  fs->SyncFs(false);
  FileTester::SuddenPowerOff(std::move(fs), &bc);

  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);
  FileTester::Unmount(std::move(fs), &bc);
}

TEST_F(CheckpointTest, CpError) {
  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  fbl::RefPtr<f2fs::File> vnode = fbl::RefPtr<f2fs::File>::Downcast(*std::move(test_file));
  char wbuf[] = "Checkpoint error test";
  char rbuf[kBlockSize];

  // Make dirty data, node, and meta Pages.
  FileTester::AppendToFile(vnode.get(), wbuf, sizeof(wbuf));

  // The appended data is written in the node page of |root_dir_|
  // since the inline_data option is enabled by default.
  ASSERT_EQ(fs_->GetSuperblockInfo().GetPageCount(CountType::kDirtyNodes), 1);
  ASSERT_FALSE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

  // Set a hook to trigger an io error with any write requests on FakeBlockDevice,
  // which causes that f2fs sets the checkpoint error flag.
  auto hook = [](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.command.opcode == BLOCK_OPCODE_WRITE) {
      return ZX_ERR_PEER_CLOSED;
    }
    return ZX_OK;
  };
  DeviceTester::SetHook(fs_.get(), hook);
  fs_->SyncFs();

  ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

  // All operations causing dirty pages are not allowed.
  ASSERT_EQ(vnode->Truncate(0), ZX_ERR_BAD_STATE);
  ASSERT_EQ(root_dir_->Unlink("test", false), ZX_ERR_BAD_STATE);
  test_file = root_dir_->Create("test2", fs::CreationType::kFile);
  ASSERT_EQ(test_file.status_value(), ZX_ERR_BAD_STATE) << test_file.status_string();
  ASSERT_EQ(root_dir_->Rename(root_dir_, "test", "test1", false, false), ZX_ERR_BAD_STATE);
  ASSERT_EQ(root_dir_->Link("test", vnode), ZX_ERR_BAD_STATE);

  // Read operations should succeed.
  FileTester::ReadFromFile(vnode.get(), rbuf, sizeof(wbuf), 0);
  fbl::RefPtr<fs::Vnode> vn;
  ASSERT_EQ(root_dir_->Lookup("test", &vn), ZX_OK);
  ASSERT_EQ(strcmp(wbuf, rbuf), 0);
  DeviceTester::SetHook(fs_.get(), nullptr);

  vnode->Close();
  vnode = nullptr;
}

TEST_F(CheckpointTest, ValidateCheckpointFirstCpPackDiskFail) {
  fs_->GetMetaVnode().Writeback(true, true);

  block_t cp_start_blk_no = LeToCpu(fs_->GetSuperblockInfo().GetSuperblock().cp_blkaddr);
  block_t target_addr = cp_start_blk_no * kDefaultSectorsPerBlock;

  auto hook = [target_addr](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.dev_offset == target_addr) {
      return ZX_ERR_PEER_CLOSED;
    }
    return ZX_OK;
  };

  // Check ValidateCheckpoint disk peer closed exception case
  // Read the 1st cp block in this CP pack
  {
    uint64_t cp1_version = 0;
    LockedPage cp_page;
    DeviceTester::SetHook(fs_.get(), hook);
    ASSERT_EQ(fs_->ValidateCheckpoint(cp_start_blk_no, &cp1_version, &cp_page), ZX_ERR_PEER_CLOSED);
    ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

    DeviceTester::SetHook(fs_.get(), nullptr);
    fs_->GetSuperblockInfo().ClearCpFlags(CpFlag::kCpErrorFlag);
  }
}

TEST_F(CheckpointTest, ValidateCheckpointSecondCpPackDiskFail) {
  fs_->GetMetaVnode().Writeback(true, true);

  block_t cp_start_blk_no = LeToCpu(fs_->GetSuperblockInfo().GetSuperblock().cp_blkaddr);
  block_t first_cp_blk_no = cp_start_blk_no;

  LockedPage cp_page;
  ASSERT_EQ(fs_->GetMetaPage(first_cp_blk_no, &cp_page), ZX_OK);
  Checkpoint *cp_block = cp_page->GetAddress<Checkpoint>();
  block_t second_cp_blk_no = first_cp_blk_no + LeToCpu(cp_block->cp_pack_total_block_count) - 1;
  cp_page.reset();

  block_t target_addr = second_cp_blk_no * kDefaultSectorsPerBlock;

  auto hook = [target_addr](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.dev_offset == target_addr) {
      return ZX_ERR_PEER_CLOSED;
    }
    return ZX_OK;
  };

  // Check ValidateCheckpoint disk peer closed exception case
  // Read the 2nd cp block in this CP pack
  {
    uint64_t cp1_version = 0;
    LockedPage cp_page;
    DeviceTester::SetHook(fs_.get(), hook);
    ASSERT_EQ(fs_->ValidateCheckpoint(cp_start_blk_no, &cp1_version, &cp_page), ZX_ERR_PEER_CLOSED);
    ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

    DeviceTester::SetHook(fs_.get(), nullptr);
    fs_->GetSuperblockInfo().ClearCpFlags(CpFlag::kCpErrorFlag);
  }
}

TEST_F(CheckpointTest, FlushNatEntriesDiskFail) {
  DisableFsck();

  fs_->GetMetaVnode().Writeback(true, true);

  pgoff_t target_addr =
      MapTester::GetCurrentNatAddr(fs_->GetNodeManager(), 0) * kDefaultSectorsPerBlock;

  auto hook = [target_addr](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.dev_offset == target_addr) {
      return ZX_ERR_PEER_CLOSED;
    }
    return ZX_OK;
  };

  // It creates 455 dirty nat to make one dirty NAT block.
  constexpr int kNidCountForMakeOneDirtyNATBlock = 455;

  uint32_t nid_offset = kRootInodeNid + 1;
  for (uint32_t i = nid_offset; i < nid_offset + kNidCountForMakeOneDirtyNATBlock; ++i) {
    MapTester::DoWriteNat(fs_.get(), i, i, 0);
  }

  // Check disk peer closed exception case in FlushNatEntries()
  {
    DeviceTester::SetHook(fs_.get(), hook);
    ASSERT_EQ(fs_->GetNodeManager().FlushNatEntries(), ZX_ERR_PEER_CLOSED);
    ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

    DeviceTester::SetHook(fs_.get(), nullptr);
    fs_->GetSuperblockInfo().ClearCpFlags(CpFlag::kCpErrorFlag);
  }
}

TEST_F(CheckpointTest, FlushSitEntriesDiskFail) TA_NO_THREAD_SAFETY_ANALYSIS {
  DisableFsck();

  fs_->GetMetaVnode().Writeback(true, true);
  pgoff_t target_addr = fs_->GetSegmentManager().CurrentSitAddr(0) * kDefaultSectorsPerBlock;

  auto hook = [target_addr](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.dev_offset == target_addr) {
      return ZX_ERR_PEER_CLOSED;
    }
    return ZX_OK;
  };

  // Write sit entries for one block
  for (uint32_t i = 0; i < kMapPerSitEntry * kSitEntryPerBlock; ++i) {
    block_t new_blkaddr;
    MapTester::DoWriteSit(fs_.get(), CursegType::kCursegWarmData,
                          i / kMapPerSitEntry + kStartSegmentOfWarmData, &new_blkaddr);
  }

  // Check disk peer closed exception case in FlushSitEntries()
  {
    DeviceTester::SetHook(fs_.get(), hook);
    ASSERT_EQ(fs_->GetSegmentManager().FlushSitEntries(), ZX_ERR_PEER_CLOSED);
    ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

    DeviceTester::SetHook(fs_.get(), nullptr);
    fs_->GetSuperblockInfo().ClearCpFlags(CpFlag::kCpErrorFlag);
  }

  // Check disk peer closed exception case in WriteCheckpoint()
  {
    DeviceTester::SetHook(fs_.get(), hook);
    ASSERT_EQ(fs_->SyncFs(), ZX_ERR_PEER_CLOSED);
    ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

    DeviceTester::SetHook(fs_.get(), nullptr);
    fs_->GetSuperblockInfo().ClearCpFlags(CpFlag::kCpErrorFlag);
  }
}

TEST_F(CheckpointTest, DoCheckpointDiskFail) {
  DisableFsck();

  WritebackOperation op;
  fs_->GetMetaVnode().Writeback(true, true);

  auto hook = [&](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.command.opcode == BLOCK_OPCODE_WRITE &&
        _req.command.flags & BLOCK_IO_FLAG_FORCE_ACCESS) {
      return ZX_ERR_PEER_CLOSED;
    }
    return ZX_OK;
  };

  // Check disk peer closed exception case in DoCheckpoint()
  DeviceTester::SetHook(fs_.get(), hook);
  ASSERT_NE(fs_->SyncFs(), ZX_OK);
  ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

  DeviceTester::SetHook(fs_.get(), nullptr);
  fs_->GetSuperblockInfo().ClearCpFlags(CpFlag::kCpErrorFlag);
}

TEST_F(CheckpointTest, ReadCompactSummaryDiskFail) TA_NO_THREAD_SAFETY_ANALYSIS {
  DisableFsck();

  fs_->GetMetaVnode().Writeback(true, true);

  block_t target_addr = fs_->GetSegmentManager().StartSumBlock() * kDefaultSectorsPerBlock;

  auto hook = [target_addr](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.dev_offset == target_addr) {
      return ZX_ERR_PEER_CLOSED;
    }
    return ZX_OK;
  };

  // Check disk peer closed exception case in ReadCompactedSummaries()
  {
    DeviceTester::SetHook(fs_.get(), hook);
    ASSERT_EQ(fs_->GetSegmentManager().ReadCompactedSummaries(), ZX_ERR_PEER_CLOSED);
    ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

    DeviceTester::SetHook(fs_.get(), nullptr);
    fs_->GetSuperblockInfo().ClearCpFlags(CpFlag::kCpErrorFlag);
  }
}

TEST_F(CheckpointTest, ReadNormalSummaryDiskFail) TA_NO_THREAD_SAFETY_ANALYSIS {
  DisableFsck();

  fs_->GetMetaVnode().Writeback(true, true);

  block_t target_addr = fs_->GetSegmentManager().SumBlkAddr(
                            kNrCursegType, static_cast<int>(CursegType::kCursegWarmData)) *
                        kDefaultSectorsPerBlock;

  auto hook = [target_addr](const BlockFifoRequest &_req, const zx::vmo *_vmo) {
    if (_req.dev_offset == target_addr) {
      return ZX_ERR_PEER_CLOSED;
    }
    return ZX_OK;
  };

  // Check disk peer closed exception case in ReadNormalSummaries()
  {
    DeviceTester::SetHook(fs_.get(), hook);
    ASSERT_EQ(
        fs_->GetSegmentManager().ReadNormalSummaries(static_cast<int>(CursegType::kCursegWarmData)),
        ZX_ERR_PEER_CLOSED);
    ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

    DeviceTester::SetHook(fs_.get(), nullptr);
    fs_->GetSuperblockInfo().ClearCpFlags(CpFlag::kCpErrorFlag);
  }
}

TEST_F(CheckpointTest, InvalidAllocType) {
  DisableFsck();
  SuperblockInfo &sb_info = fs_->GetSuperblockInfo();
  for (size_t i = 0; i < kNrCursegType; ++i) {
    BlockBuffer<Checkpoint> ckpt_block = sb_info.GetCheckpointBlock();
    ckpt_block->alloc_type[i] = static_cast<uint8_t>(AllocType::kSSR) + 1;
    ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);
  }
}

TEST_F(CheckpointTest, InvalidCursegBlkoff) {
  DisableFsck();
  SuperblockInfo &sb_info = fs_->GetSuperblockInfo();
  const uint16_t invalid_blkoff = static_cast<uint16_t>(sb_info.GetBlocksPerSeg());

  for (size_t i = 0; i < kNrCursegDataType; ++i) {
    BlockBuffer<Checkpoint> ckpt_block = sb_info.GetCheckpointBlock();
    ckpt_block->cur_data_blkoff[i] = CpuToLe(invalid_blkoff);
    ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);
  }

  for (size_t i = 0; i < kNrCursegNodeType; ++i) {
    BlockBuffer<Checkpoint> ckpt_block = sb_info.GetCheckpointBlock();
    ckpt_block->cur_node_blkoff[i] = CpuToLe(invalid_blkoff);
    ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);
  }
}

TEST_F(CheckpointTest, InvalidCursegSegno) {
  DisableFsck();
  SuperblockInfo &sb_info = fs_->GetSuperblockInfo();
  const uint32_t main_segs = LeToCpu(sb_info.GetSuperblock().segment_count_main);
  const std::vector<uint32_t> invalid_segnos = {main_segs, main_segs + 100,
                                                std::numeric_limits<uint32_t>::max()};

  for (size_t i = 0; i < kNrCursegDataType; ++i) {
    for (uint32_t invalid_segno : invalid_segnos) {
      BlockBuffer<Checkpoint> ckpt_block = sb_info.GetCheckpointBlock();
      ckpt_block->cur_data_segno[i] = CpuToLe(invalid_segno);
      ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);
    }
  }

  for (size_t i = 0; i < kNrCursegNodeType; ++i) {
    for (uint32_t invalid_segno : invalid_segnos) {
      BlockBuffer<Checkpoint> ckpt_block = sb_info.GetCheckpointBlock();
      ckpt_block->cur_node_segno[i] = CpuToLe(invalid_segno);
      ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);
    }
  }
}

TEST_F(CheckpointTest, InvalidRsvdSegmentCount) {
  DisableFsck();
  SuperblockInfo &sb_info = fs_->GetSuperblockInfo();
  const uint32_t total_segments = LeToCpu(sb_info.GetSuperblock().segment_count);
  const std::vector<uint32_t> invalid_rsvd_segment_counts = {
      total_segments,
      total_segments + 100,
      std::numeric_limits<uint32_t>::max() - 100,
      std::numeric_limits<uint32_t>::max(),
  };

  for (uint32_t invalid_count : invalid_rsvd_segment_counts) {
    BlockBuffer<Checkpoint> ckpt_block = sb_info.GetCheckpointBlock();
    ckpt_block->rsvd_segment_count = CpuToLe(invalid_count);
    ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);
  }
}

TEST_F(CheckpointTest, InvalidChecksumOffset) {
  DisableFsck();
  SuperblockInfo &sb_info = fs_->GetSuperblockInfo();

  // Aligned offset below header
  BlockBuffer<Checkpoint> ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->checksum_offset =
      CpuToLe(static_cast<uint32_t>(Checkpoint::GetHeaderByteSize() - sizeof(uint32_t)));
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  // Unaligned offset within bounds
  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->checksum_offset = CpuToLe(static_cast<uint32_t>(Checkpoint::GetHeaderByteSize() + 1));
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  // Aligned offset exceeding block size (testing upper bounds check independently of alignment)
  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->checksum_offset = CpuToLe(static_cast<uint32_t>(kBlockSize));
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->checksum_offset = CpuToLe(static_cast<uint32_t>(kBlockSize + sizeof(uint32_t)));
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);
}

TEST_F(CheckpointTest, InvalidCpPackBlockCounts) {
  DisableFsck();
  SuperblockInfo &sb_info = fs_->GetSuperblockInfo();

  // Test invalid cp_pack_total_block_count (must be in [3, blocks_per_seg])
  BlockBuffer<Checkpoint> ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_total_block_count = CpuToLe(0u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_total_block_count = CpuToLe(1u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_total_block_count = CpuToLe(2u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_total_block_count =
      CpuToLe(static_cast<uint32_t>(sb_info.GetBlocksPerSeg() + 1));
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  // Test invalid cp_pack_start_sum (must be in [cp_payload + 1, blocks_per_seg - 1 -
  // kNrCursegType] and < cp_pack_total_block_count)
  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_start_sum = CpuToLe(0u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_start_sum = ckpt_block->cp_pack_total_block_count;
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_start_sum = CpuToLe(LeToCpu(ckpt_block->cp_pack_total_block_count) + 1);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_start_sum =
      CpuToLe(static_cast<uint32_t>(sb_info.GetBlocksPerSeg() - kNrCursegType));
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  // When kCpCompactSumFlag and kCpUmountFlag are both set, min_sum_blocks is 4
  // (1 compact data summary + 3 normal node summaries).
  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_start_sum = CpuToLe(1u);
  ckpt_block->ckpt_flags |= CpuToLe(static_cast<uint32_t>(CpFlag::kCpCompactSumFlag));
  ckpt_block->ckpt_flags |= CpuToLe(static_cast<uint32_t>(CpFlag::kCpUmountFlag));
  ckpt_block->cp_pack_total_block_count = CpuToLe(6u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_OK);

  // If cp_pack_total_block_count is less than cp_pack_start_sum + min_sum_blocks + 1, it is
  // rejected.
  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_start_sum = CpuToLe(1u);
  ckpt_block->ckpt_flags |= CpuToLe(static_cast<uint32_t>(CpFlag::kCpCompactSumFlag));
  ckpt_block->ckpt_flags |= CpuToLe(static_cast<uint32_t>(CpFlag::kCpUmountFlag));
  ckpt_block->cp_pack_total_block_count = CpuToLe(5u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  // When kCpCompactSumFlag is set without kCpUmountFlag, min_sum_blocks is 1.
  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_start_sum = CpuToLe(1u);
  ckpt_block->ckpt_flags |= CpuToLe(static_cast<uint32_t>(CpFlag::kCpCompactSumFlag));
  ckpt_block->ckpt_flags &= ~CpuToLe(static_cast<uint32_t>(CpFlag::kCpUmountFlag));
  ckpt_block->cp_pack_total_block_count = CpuToLe(3u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_OK);

  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_start_sum = CpuToLe(1u);
  ckpt_block->ckpt_flags |= CpuToLe(static_cast<uint32_t>(CpFlag::kCpCompactSumFlag));
  ckpt_block->ckpt_flags &= ~CpuToLe(static_cast<uint32_t>(CpFlag::kCpUmountFlag));
  ckpt_block->cp_pack_total_block_count = CpuToLe(2u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  // Without kCpCompactSumFlag, unmount requires 6 summary blocks (3 data + 3 node).
  ckpt_block = sb_info.GetCheckpointBlock();
  ckpt_block->cp_pack_start_sum = CpuToLe(1u);
  ckpt_block->ckpt_flags &= ~CpuToLe(static_cast<uint32_t>(CpFlag::kCpCompactSumFlag));
  ckpt_block->ckpt_flags |= CpuToLe(static_cast<uint32_t>(CpFlag::kCpUmountFlag));
  ckpt_block->cp_pack_total_block_count = CpuToLe(7u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  ckpt_block->cp_pack_total_block_count = CpuToLe(8u);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_OK);
}

TEST_F(CheckpointTest, InvalidBitmapByteSizes) {
  DisableFsck();
  SuperblockInfo &sb_info = fs_->GetSuperblockInfo();

  // When cp_payload == 0: sit_size + nat_size > max_cp_bitmap_size
  BlockBuffer<Checkpoint> ckpt_block = sb_info.GetCheckpointBlock();
  const uint32_t sit_size = LeToCpu(ckpt_block->sit_ver_bitmap_bytesize);
  const uint32_t nat_size = LeToCpu(ckpt_block->nat_ver_bitmap_bytesize);
  // Set checksum_offset (4-byte aligned) such that max_cp_bitmap_size < sit_size + nat_size
  const uint32_t tight_offset =
      fbl::round_down(static_cast<uint32_t>(Checkpoint::GetHeaderByteSize() +
                                            (sit_size + nat_size) - sizeof(uint32_t)),
                      static_cast<uint32_t>(sizeof(uint32_t)));
  ckpt_block->checksum_offset = CpuToLe(tight_offset);
  ASSERT_EQ(sb_info.SetCheckpoint(ckpt_block), ZX_ERR_BAD_STATE);

  // When cp_payload > 0: nat_ver_bitmap_bytesize > max_cp_bitmap_size
  auto sb = std::make_unique<Superblock>();
  sb->log_blocksize = CpuToLe(12u);
  sb->log_blocks_per_seg = CpuToLe(kDefaultLogBlocksPerSegment);
  sb->segment_count_ckpt = CpuToLe(2u);
  sb->segment_count_sit = CpuToLe(2u);
  sb->segment_count_nat = CpuToLe(2u);
  sb->segment_count_ssa = CpuToLe(1u);
  sb->segment_count_main = CpuToLe(10u);
  sb->segment_count = CpuToLe(100u);
  sb->cp_payload = CpuToLe(1u);
  SuperblockInfo payload_sb_info(std::move(sb));

  BlockBuffer<Checkpoint> payload_block;
  payload_block->cp_pack_total_block_count = CpuToLe(10u);
  payload_block->cp_pack_start_sum = CpuToLe(2u);
  payload_block->ckpt_flags = static_cast<uint32_t>(CpFlag::kCpCompactSumFlag);
  payload_block->sit_ver_bitmap_bytesize =
      CpuToLe(static_cast<uint32_t>(VersionBitmapByteSize(2u, kDefaultLogBlocksPerSegment)));
  payload_block->nat_ver_bitmap_bytesize =
      CpuToLe(static_cast<uint32_t>(VersionBitmapByteSize(2u, kDefaultLogBlocksPerSegment)));

  // Set checksum_offset such that max_cp_bitmap_size < nat_ver_bitmap_bytesize
  const uint32_t payload_nat_size = LeToCpu(payload_block->nat_ver_bitmap_bytesize);
  const uint32_t payload_tight_offset = fbl::round_down(
      static_cast<uint32_t>(Checkpoint::GetHeaderByteSize() + payload_nat_size - sizeof(uint32_t)),
      static_cast<uint32_t>(sizeof(uint32_t)));
  payload_block->checksum_offset = CpuToLe(payload_tight_offset);
  ASSERT_EQ(payload_sb_info.SetCheckpoint(payload_block), ZX_ERR_BAD_STATE);

  // When cp_payload > 0: sit_ver_bitmap_bytesize > cp_payload * kBlockSize
  auto sb_overflow = std::make_unique<Superblock>();
  sb_overflow->log_blocksize = CpuToLe(12u);
  sb_overflow->log_blocks_per_seg = CpuToLe(kDefaultLogBlocksPerSegment);
  sb_overflow->segment_count_ckpt = CpuToLe(2u);
  sb_overflow->segment_count_sit = CpuToLe(200u);
  sb_overflow->segment_count_nat = CpuToLe(2u);
  sb_overflow->segment_count_ssa = CpuToLe(1u);
  sb_overflow->segment_count_main = CpuToLe(10u);
  sb_overflow->segment_count = CpuToLe(300u);
  sb_overflow->cp_payload = CpuToLe(1u);
  SuperblockInfo payload_overflow_sb_info(std::move(sb_overflow));

  BlockBuffer<Checkpoint> overflow_block;
  overflow_block->cp_pack_total_block_count = CpuToLe(10u);
  overflow_block->cp_pack_start_sum = CpuToLe(2u);
  overflow_block->ckpt_flags = static_cast<uint32_t>(CpFlag::kCpCompactSumFlag);
  overflow_block->sit_ver_bitmap_bytesize =
      CpuToLe(static_cast<uint32_t>(VersionBitmapByteSize(200u, kDefaultLogBlocksPerSegment)));
  overflow_block->nat_ver_bitmap_bytesize =
      CpuToLe(static_cast<uint32_t>(VersionBitmapByteSize(2u, kDefaultLogBlocksPerSegment)));
  overflow_block->checksum_offset = CpuToLe(kChecksumOffset);
  ASSERT_EQ(payload_overflow_sb_info.SetCheckpoint(overflow_block), ZX_ERR_BAD_STATE);
}

}  // namespace
}  // namespace f2fs
