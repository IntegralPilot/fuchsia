// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/f2fs.h"

#include <gtest/gtest.h>
#include <safemath/checked_math.h>

#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {
using Runner = ComponentRunner;

static void WriteSuperblock(const Superblock &sb, BcacheMapper &bc) {
  BlockBuffer block;
  memcpy(block.get<uint8_t>() + kSuperOffset, &sb, sizeof(Superblock));
  bc.Writeblk(0, &block);
  bc.Writeblk(1, &block);
}

TEST(SuperblockTest, ValidateRawSuper) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDevWithOptions(&bc, MkfsOptions{});
  auto superblock = LoadSuperblock(*bc);
  ASSERT_TRUE(superblock.is_ok());

  // Check exception cases
  BlockBuffer<Superblock> corrupted;
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->log_sectors_per_block = CpuToLe(kDefaultSectorsPerBlock);
  corrupted->log_sectorsize = CpuToLe(kMaxLogSectorSize);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->log_sectorsize = CpuToLe(kMaxLogSectorSize + 1);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->log_blocksize = CpuToLe(kMaxLogSectorSize + 1);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->magic = CpuToLe(0xF2F5FFFF);
  corrupted->log_blocksize = CpuToLe(kMaxLogSectorSize + 1);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segs_per_sec = CpuToLe(0u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->secs_per_zone = CpuToLe(0u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segs_per_sec = CpuToLe(2u);
  corrupted->segment_count_main = CpuToLe(5u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segs_per_sec = CpuToLe(1u);
  corrupted->segment_count_main = CpuToLe(5u);
  corrupted->section_count = CpuToLe(4u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->log_blocks_per_seg = CpuToLe(kDefaultLogBlocksPerSegment + 1);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->log_blocks_per_seg = CpuToLe(kDefaultLogBlocksPerSegment - 1);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check bitmap size exceeding checkpoint capacity when cp_payload == 0
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->cp_payload = 0;
  corrupted->segment_count_sit = CpuToLe(2000u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check bitmap size exceeding cp_payload capacity when cp_payload > 0
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->cp_payload = CpuToLe(1u);
  corrupted->segment_count_sit = CpuToLe(200u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check bitmap size exceeding kMaxSitBitmapSize when cp_payload > 0
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->cp_payload = CpuToLe(kMaxCpPayload);
  corrupted->segment_count_sit = CpuToLe(2000u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check cp_payload exceeding kMaxCpPayload
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->cp_payload = CpuToLe(kMaxCpPayload + 1);
  corrupted->segment_count_sit = CpuToLe(2u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check 32-bit shift overflow case for segment_count_sit.
  // (0x01000000 / 2 << 9 wraps to 0 in 32-bit math).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->cp_payload = 0;
  corrupted->segment_count_sit = CpuToLe(0x01000000u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check invalid segment_count_ckpt (must be >= 2 for dual checkpoint packs).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_ckpt = CpuToLe(1u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check zero segment_count_sit (must be non-zero and at least 2).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_sit = CpuToLe(0u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check zero segment_count_nat (must be non-zero and at least 2).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_nat = CpuToLe(0u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check zero segment_count_ssa (must be non-zero).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_ssa = CpuToLe(0u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check odd segment_count_sit (must have even parity for dual SIT packs).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_sit = CpuToLe(1u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check odd segment_count_nat (must have even parity for dual NAT packs).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_nat = CpuToLe(1u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check odd segment_count_sit > 1.
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_sit = CpuToLe(3u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check odd segment_count_nat > 1.
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_nat = CpuToLe(3u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check zero segment_count_main (must be non-zero).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_main = CpuToLe(0u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Check segment_count equal to segment_count_main (insufficient for metadata).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count = corrupted->segment_count_main;
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Total segments required exceeds segment_count.
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count =
      CpuToLe(LeToCpu(corrupted->segment_count_ckpt) + LeToCpu(corrupted->segment_count_sit) +
              LeToCpu(corrupted->segment_count_nat) + LeToCpu(corrupted->segment_count_ssa) +
              LeToCpu(corrupted->segment_count_main) - 1);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Exact equality between required segments and segment_count is valid.
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count =
      CpuToLe(LeToCpu(corrupted->segment_count_ckpt) + LeToCpu(corrupted->segment_count_sit) +
              LeToCpu(corrupted->segment_count_nat) + LeToCpu(corrupted->segment_count_ssa) +
              LeToCpu(corrupted->segment_count_main));
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_OK);

  // It is valid for segment_count to be greater than the sum of meta and main segments
  // (e.g. when mkfs discards remainder segments not forming a full section).
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count = CpuToLe(LeToCpu(corrupted->segment_count) + 1);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_OK);

  // Zero segment_count is invalid.
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count = CpuToLe(0u);
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Integer overflow in metadata segments addition.
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_ssa = CpuToLe(std::numeric_limits<uint32_t>::max());
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);

  // Integer overflow in total segments addition.
  std::memcpy(&corrupted, (*superblock).get(), sizeof(Superblock));
  corrupted->segment_count_main = CpuToLe(std::numeric_limits<uint32_t>::max() - 10);
  corrupted->section_count = corrupted->segment_count_main;
  WriteSuperblock(*corrupted, *bc);
  ASSERT_EQ(LoadSuperblock(*bc).status_value(), ZX_ERR_INVALID_ARGS);
}

TEST(SuperblockTest, GetValidCheckpoint) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDevWithOptions(&bc, MkfsOptions{});

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  // Create a vfs object for unit tests.
  auto vfs_or = Runner::CreateRunner(loop.dispatcher());
  ZX_ASSERT(vfs_or.is_ok());
  std::unique_ptr<F2fs> fs =
      std::make_unique<F2fs>(loop.dispatcher(), std::move(bc), MountOptions{}, (*vfs_or).get());

  auto superblock = LoadSuperblock(fs->GetBc());
  ASSERT_TRUE(superblock.is_ok());
  superblock->cp_blkaddr = LeToCpu(superblock->cp_blkaddr) + 2;
  // Check GetValidCheckpoint exception case
  ASSERT_EQ(fs->LoadSuper(std::move(*superblock)), ZX_ERR_INVALID_ARGS);

  fs->GetVCache().Reset();
  fs->Reset();
}

TEST(SuperblockTest, ValidateCkpt) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDevWithOptions(&bc, MkfsOptions{});

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  // Create a vfs object for unit tests.
  auto vfs_or = Runner::CreateRunner(loop.dispatcher());
  ZX_ASSERT(vfs_or.is_ok());
  std::unique_ptr<F2fs> fs =
      std::make_unique<F2fs>(loop.dispatcher(), std::move(bc), MountOptions{}, (*vfs_or).get());

  // Check ValidateCkpt exception case
  auto superblock = LoadSuperblock(fs->GetBc());
  ASSERT_TRUE(superblock.is_ok());
  superblock->segment_count_nat = 0;
  ASSERT_EQ(fs->LoadSuper(std::move(*superblock)), ZX_ERR_BAD_STATE);

  superblock = LoadSuperblock(fs->GetBc());
  ASSERT_TRUE(superblock.is_ok());
  superblock->segment_count = 0;
  ASSERT_EQ(fs->LoadSuper(std::move(*superblock)), ZX_ERR_BAD_STATE);

  fs->GetVCache().Reset();
  fs->Reset();
}

TEST(SuperblockTest, Reset) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDevWithOptions(&bc, MkfsOptions{});

  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  // Create a vfs object for unit tests.
  auto vfs_or = Runner::CreateRunner(loop.dispatcher());
  ZX_ASSERT(vfs_or.is_ok());
  std::unique_ptr<F2fs> fs =
      std::make_unique<F2fs>(loop.dispatcher(), std::move(bc), MountOptions{}, (*vfs_or).get());

  auto superblock = LoadSuperblock(fs->GetBc());
  ASSERT_TRUE(superblock.is_ok());
  ASSERT_EQ(fs->LoadSuper(std::move(*superblock)), ZX_OK);
  fs->GetVCache().Reset();

  ASSERT_TRUE(fs->IsValid());
  fs->Reset();
  ASSERT_FALSE(fs->IsValid());
  ASSERT_TRUE(fs->GetRootVnode().is_error());

  superblock = LoadSuperblock(fs->GetBc());
  ASSERT_TRUE(superblock.is_ok());
  ASSERT_EQ(fs->LoadSuper(std::move(*superblock)), ZX_OK);
  fs->GetVCache().Reset();

  ASSERT_TRUE(fs->IsValid());
  fs->Reset();
  ASSERT_FALSE(fs->IsValid());
}

TEST(F2fsTest, TakeBc) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDevWithOptions(&bc, MkfsOptions{});
  BcacheMapper *bcache_ptr = bc.get();

  std::unique_ptr<F2fs> fs;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), MountOptions{}, &bc, &fs);
  ASSERT_TRUE(fs->IsValid());
  ASSERT_EQ(&fs->GetBc(), bcache_ptr);

  fs->Sync();
  fs->PutSuper();
  auto bc_or = fs->TakeBc();
  ASSERT_TRUE(bc_or.is_ok());
  ASSERT_TRUE(fs->TakeBc().is_error());
  ASSERT_FALSE(fs->IsValid());
  fs.reset();
  bc = std::move(*bc_or);
  ASSERT_EQ(bc.get(), bcache_ptr);

  FileTester::MkfsOnFakeDevWithOptions(&bc, {});
  FileTester::MountWithOptions(loop.dispatcher(), MountOptions{}, &bc, &fs);

  fs->Sync();
  fs->PutSuper();
  ASSERT_TRUE(fs->TakeBc().is_ok());
  ASSERT_TRUE(fs->TakeBc().is_error());
  ASSERT_FALSE(fs->IsValid());
}

TEST(F2fsTest, BlockBuffer) {
  BlockBuffer block;
  uint8_t data[kBlockSize];
  memset(data, 0xaa, kBlockSize);
  memcpy(&block, data, kBlockSize);
  ASSERT_EQ(memcmp(&block, data, kBlockSize), 0);

  BlockBuffer block2(block);
  ASSERT_EQ(memcmp(&block2, data, kBlockSize), 0);
  memset(&block2, 0xbb, kBlockSize);

  block = block2;
  ASSERT_EQ(memcmp(&block, &block2, kBlockSize), 0);
}

TEST(F2fsTest, GetFilesystemInfo) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDevWithOptions(&bc, MkfsOptions{});

  std::unique_ptr<F2fs> fs;
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), MountOptions{}, &bc, &fs);

  auto &sb_info = fs->GetSuperblockInfo();
  auto info_or = fs->GetFilesystemInfo();
  ASSERT_TRUE(info_or.is_ok());
  auto info = info_or.value();

  ASSERT_EQ(info.block_size, kBlockSize);
  ASSERT_EQ(info.max_filename_size, kMaxNameLen);
  ASSERT_EQ(info.fs_type, fuchsia_fs::VfsType::kF2Fs);
  ASSERT_EQ(info.total_bytes, sb_info.GetTotalBlockCount() * kBlockSize);
  ASSERT_EQ(info.used_bytes, sb_info.GetValidBlockCount() * kBlockSize);
  ASSERT_EQ(info.total_nodes, sb_info.GetMaxNodeCount());
  ASSERT_EQ(info.used_nodes, sb_info.GetValidInodeCount());
  ASSERT_EQ(info.name, "f2fs");

  // Check type conversion
  block_t tmp_user_block_count = sb_info.GetTotalBlockCount();
  block_t tmp_valid_block_count = sb_info.GetTotalBlockCount();

  constexpr uint64_t LARGE_BLOCK_COUNT = 26214400;  // 100GB

  sb_info.SetTotalBlockCount(LARGE_BLOCK_COUNT);
  sb_info.SetValidBlockCount(LARGE_BLOCK_COUNT);

  info_or = fs->GetFilesystemInfo();
  ASSERT_TRUE(info_or.is_ok());
  info = info_or.value();

  ASSERT_EQ(info.total_bytes, LARGE_BLOCK_COUNT * kBlockSize);
  ASSERT_EQ(info.used_bytes, LARGE_BLOCK_COUNT * kBlockSize);

  sb_info.SetTotalBlockCount(tmp_user_block_count);
  sb_info.SetValidBlockCount(tmp_valid_block_count);
  FileTester::Unmount(std::move(fs), &bc);
}

using StorageTest = SingleFileTest;

TEST_F(StorageTest, LargeIOs) {
  // Create a fake block for test
  auto device =
      std::make_unique<block_client::FakeBlockDevice>(block_client::FakeBlockDevice::Config{
          .block_count = kDefaultSectorCount / kDefaultSectorsPerBlock, .block_size = kBlockSize});
  zx::result block_or = CreateBcacheMapper(std::move(device), true);
  ASSERT_TRUE(block_or.is_ok());

  // Create |reader_| and |writer | based on the fake block
  std::unique_ptr<StorageBufferPool> buffer_pool = std::make_unique<StorageBufferPool>(&**block_or);
  size_t large_buffer_size = buffer_pool->GetLargeBufferSize();

  ASSERT_TRUE(kDefaultSectorCount / kDefaultSectorsPerBlock > large_buffer_size * 2);

  std::unique_ptr<Reader> reader_ = std::make_unique<Reader>(std::move(buffer_pool));
  std::unique_ptr<Writer> writer_ =
      std::make_unique<Writer>(std::make_unique<StorageBufferPool>(&**block_or));

  // Use meta vnode for test as we need Pages using discardable vmo
  VnodeF2fs &vnode = fs_->GetMetaVnode();
  std::vector<block_t> addrs;
  PageList pages;

  // Write twice as many Pages as |large_buffer_size|
  size_t num_pages = large_buffer_size * 2;
  for (block_t i = 1; i <= num_pages; ++i) {
    LockedPage page;
    vnode.GrabLockedPage(i, &page);
    page->ClearUptodate();
    page.Zero();
    *page->GetAddress<block_t>() = i;
    page.SetWriteback(i);
    pages.push_back(page.release());
  }
  sync_completion_t completion;
  writer_->ScheduleWriteBlocks(&completion, std::move(pages), true);
  ASSERT_TRUE(sync_completion_wait(&completion, zx::sec(kWriteTimeOut).get()) == ZX_OK);

  // Reset Pages
  std::vector<LockedPage> locked_pages;
  for (block_t i = 1; i <= num_pages; ++i) {
    LockedPage page;
    vnode.GrabLockedPage(i, &page);
    page->ClearUptodate();
    ASSERT_EQ(*page->GetAddress<block_t>(), i);
    page.Zero();
    locked_pages.push_back(std::move(page));
    addrs.push_back(i);
  }

  // Verify and reset read Pages
  ASSERT_TRUE(reader_->ReadBlocks(locked_pages, addrs).is_ok());
  block_t i = 0;
  for (auto &page : locked_pages) {
    ASSERT_TRUE(page->IsUptodate());
    ASSERT_EQ(*page->GetAddress<block_t>(), addrs[i++]);
    page.Zero();
    page->ClearUptodate();
  }

  zx::vmo read_vmo;
  zx_vaddr_t paddr;
  // Verify and reset read vmo
  ASSERT_EQ(zx::vmo::create(num_pages * Page::Size(), 0, &read_vmo), ZX_OK);
  ASSERT_EQ(zx::vmar::root_self()->map(ZX_VM_PERM_READ | ZX_VM_PERM_WRITE, 0, read_vmo, 0,
                                       num_pages * Page::Size(), &paddr),
            ZX_OK);
  ASSERT_TRUE(reader_->ReadBlocks(read_vmo, addrs).is_ok());
  for (block_t i = 0; i < num_pages; ++i) {
    zx_vaddr_t target = paddr + (i * Page::Size());
    ASSERT_EQ(*reinterpret_cast<size_t *>(target), addrs[i]);
    *reinterpret_cast<size_t *>(target) = 0U;
  }

  // The below Pages should not be read.
  // In case that a page is invalidated
  addrs[0] = kNullAddr;
  addrs[num_pages * 2 / 3] = kNullAddr;

  // In case that a page is not alloated addr.
  addrs[num_pages / 3] = kNewAddr;
  locked_pages[num_pages / 3]->SetUptodate();

  // In case that a page is already uptodate
  locked_pages[num_pages - 1]->SetUptodate();

  // Verify read Pages
  ASSERT_TRUE(reader_->ReadBlocks(locked_pages, addrs).is_ok());
  ASSERT_TRUE(reader_->ReadBlocks(read_vmo, addrs).is_ok());
  for (block_t i = 0; i < num_pages; ++i) {
    LockedPage &page = locked_pages[i];
    zx_vaddr_t target = paddr + (i * Page::Size());
    if (!IsValidBlockAddr(addrs[i])) {
      ASSERT_EQ(*page->GetAddress<block_t>(), 0U);
      ASSERT_EQ(page->IsUptodate(), addrs[i] == kNewAddr);
      ASSERT_EQ(*reinterpret_cast<size_t *>(target), 0U);
    } else if (i == num_pages - 1) {
      ASSERT_TRUE(page->IsUptodate());
      ASSERT_EQ(*page->GetAddress<block_t>(), 0U);
    } else {
      ASSERT_TRUE(page->IsUptodate());
      ASSERT_EQ(*page->GetAddress<block_t>(), addrs[i]);
      ASSERT_EQ(*reinterpret_cast<size_t *>(target), addrs[i]);
    }
    page->ClearUptodate();
  }

  ASSERT_EQ(zx::vmar::root_self()->unmap(paddr, num_pages * Page::Size()), ZX_OK);
}

}  // namespace
}  // namespace f2fs
