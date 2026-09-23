// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zircon-internal/thread_annotations.h>

#include <algorithm>
#include <random>

#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/test/unit/unit_lib.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/lib/vfs/cpp/shared_mutex.h"

namespace f2fs {
namespace {

constexpr uint32_t kFileBlocks = kDefaultBlocksPerSegment;
class GcTest : public F2fsFakeDevTestFixture {
 public:
  GcTest(TestOptions options = {}) : F2fsFakeDevTestFixture(options) {
    // GcTest should run with MountOption::kForceLfs set.
    mount_options_.SetValue(MountOption::kForceLfs, true);
  }

 protected:
  std::vector<fbl::RefPtr<File>> MakeGcTriggerCondition(uint32_t invalidate_ratio = 25,
                                                        bool sync = true) {
    fs_->GetSegmentManager().DisableFgGc();
    std::vector<fbl::RefPtr<File>> files;
    uint32_t count = 0;
    // Create files until it runs out of space.
    while (true) {
      if (fs_->GetSegmentManager().HasNotEnoughFreeSecs(0, kFileBlocks)) {
        break;
      }
      std::string file_name = std::to_string(count++);
      zx::result test_file = root_dir_->Create(file_name, fs::CreationType::kFile);
      EXPECT_TRUE(test_file.is_ok()) << test_file.status_string();
      auto file_vn = fbl::RefPtr<File>::Downcast(*std::move(test_file));
      std::array<char, Page::Size()> buf;
      f2fs_hash_t hash = DentryHash(file_name);
      std::memcpy(buf.data(), &hash, sizeof(hash));
      for (uint32_t i = 0; i < kFileBlocks; ++i) {
        FileTester::AppendToFile(file_vn.get(), buf.data(), buf.size());
      }
      file_vn->Writeback(true, true);
      EXPECT_EQ(file_vn->Close(), ZX_OK);
      files.push_back(std::move(file_vn));
      fs_->SyncFs();
    }

    // Truncate files to make dirty segments each of which has |invalidate_ratio| of invalid blocks.
    const uint64_t truncate_size = kFileBlocks * (100 - invalidate_ratio) / 100;
    for (auto &file : files) {
      file->Truncate(truncate_size * Page::Size());
    }
    if (sync) {
      fs_->SyncFs();
    }
    fs_->GetSegmentManager().EnableFgGc();
    return files;
  }
};

TEST_F(GcTest, SsrWithoutGc) {
  fs_->GetSuperblockInfo().ClearOpt(MountOption::kForceLfs);
  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  auto file = fbl::RefPtr<File>::Downcast(*std::move(test_file));
  std::array<uint8_t, kBlockSize> buf;
  size_t num_blocks = 0;
  size_t out;
  // Append |file| until the out of space
  while (true) {
    zx_status_t ret =
        FileTester::Write(file.get(), buf.data(), sizeof(buf), num_blocks * sizeof(buf), &out);
    if (ret == ZX_OK) {
      LockedPage page;
      zx_status_t ret = file->GrabLockedPage(num_blocks, &page);
      ASSERT_EQ(ret, ZX_OK);
      ASSERT_TRUE(page->IsDirty());
      ++num_blocks;
      continue;
    }
    ASSERT_EQ(ret, ZX_ERR_NO_SPACE);
    break;
  }
  size_t before = fs_->GetGcRuns();
  ASSERT_EQ(file->GetSize(), num_blocks * sizeof(buf));
  file->SyncFile();
  // 4KiB Rand W. & fsync() for 10 * the size of |file|
  for (uint32_t j = 0; j < 10; ++j) {
    srand(j);
    for (size_t i = 0; i < num_blocks; ++i) {
      size_t index = rand() % num_blocks;
      buf[0] = static_cast<uint8_t>(index + j);
      zx_status_t ret =
          FileTester::Write(file.get(), buf.data(), sizeof(buf), index * sizeof(buf), &out);
      ASSERT_EQ(ret, ZX_OK);
      file->SyncFile();
      {
        auto pages = file->FindLockedPages(index, index + 1);
        ASSERT_EQ(pages.size(), 1U);
      }
      ASSERT_EQ(FileTester::Read(file.get(), buf.data(), sizeof(buf), index * sizeof(buf), &out),
                ZX_OK);
      ASSERT_EQ(buf[0], static_cast<uint8_t>(index + j)) << index << " of " << num_blocks;
    }
  }
  // GC should not run during the test
  ASSERT_EQ(before, fs_->GetGcRuns());
  file->Close();
}

TEST_F(GcTest, TruncateAndLargeFsync) {
  fs_->GetSuperblockInfo().ClearOpt(MountOption::kForceLfs);
  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  auto file = fbl::RefPtr<File>::Downcast(*std::move(test_file));
  std::array<uint8_t, kBlockSize> buf;
  size_t num_blocks = 0;
  size_t out;
  // Append |file| until the out of space
  while (true) {
    zx_status_t ret =
        FileTester::Write(file.get(), buf.data(), sizeof(buf), num_blocks * sizeof(buf), &out);
    if (ret == ZX_OK) {
      LockedPage page;
      zx_status_t ret = file->GrabLockedPage(num_blocks, &page);
      ASSERT_EQ(ret, ZX_OK);
      ASSERT_TRUE(page->IsDirty());
      ++num_blocks;
      continue;
    }
    ASSERT_EQ(ret, ZX_ERR_NO_SPACE);
    break;
  }
  ASSERT_EQ(file->GetSize(), num_blocks * sizeof(buf));
  file->SyncFile();
  // 4KiB Rand W. & fsync() for 10 * the size of |file|
  for (uint32_t j = 1; j <= 10; ++j) {
    for (size_t i = 0; i < num_blocks; ++i) {
      zx_status_t ret =
          FileTester::Write(file.get(), buf.data(), sizeof(buf), i * sizeof(buf), &out);
      ASSERT_EQ(ret, ZX_OK);
    }
    file->Truncate((num_blocks - j) * kBlockSize);
    file->SyncFile();
  }

  file->Close();
}

TEST_F(GcTest, CpError) TA_NO_THREAD_SAFETY_ANALYSIS {
  fs_->GetSuperblockInfo().SetCpFlags(CpFlag::kCpErrorFlag);
  auto result = fs_->StartGc();
  ASSERT_TRUE(result.is_error());
  ASSERT_EQ(result.error_value(), ZX_ERR_BAD_STATE);
}

TEST_F(GcTest, CheckpointDiskReadFailOnGc) TA_NO_THREAD_SAFETY_ANALYSIS {
  DisableFsck();

  constexpr uint32_t kInvalidRatio = 5;
  auto files = MakeGcTriggerCondition(5, false);

  auto hook = [](const BlockFifoRequest &_req, const zx::vmo *_vmo) { return ZX_ERR_PEER_CLOSED; };
  DeviceTester::SetHook(fs_.get(), hook);
  size_t to_secure = files.size() * kFileBlocks * kInvalidRatio / 100;
  ASSERT_EQ(fs_->StartGc(safemath::checked_cast<uint32_t>(to_secure)).error_value(),
            ZX_ERR_PEER_CLOSED);
  ASSERT_TRUE(fs_->GetSuperblockInfo().TestCpFlags(CpFlag::kCpErrorFlag));

  DeviceTester::SetHook(fs_.get(), nullptr);
}

TEST_F(GcTest, CheckpointDiskReadFailOnGcPreFree) TA_NO_THREAD_SAFETY_ANALYSIS {
  DisableFsck();

  auto hook = [](const BlockFifoRequest &_req, const zx::vmo *_vmo) { return ZX_ERR_PEER_CLOSED; };

  uint32_t prefree_segno = fs_->GetSegmentManager().CURSEG_I(CursegType::kCursegWarmData)->segno;
  constexpr uint32_t kInvalidRatio = 100;
  MakeGcTriggerCondition(kInvalidRatio, false);

  fs_->GetSegmentManager().LocateDirtySegment(prefree_segno + 1, DirtyType::kPre);

  DeviceTester::SetHook(fs_.get(), hook);
  ASSERT_EQ(fs_->StartGc().error_value(), ZX_ERR_PEER_CLOSED);

  DeviceTester::SetHook(fs_.get(), nullptr);
}

TEST_F(GcTest, PageColdData) TA_NO_THREAD_SAFETY_ANALYSIS {
  fs_->GetSegmentManager().DisableFgGc();
  zx::result test_file = root_dir_->Create("file", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  auto file = fbl::RefPtr<File>::Downcast(*std::move(test_file));

  char buf[Page::Size()] = {
      0,
  };
  FileTester::AppendToFile(file.get(), buf, sizeof(buf));
  file->SyncFile();

  MakeGcTriggerCondition(10);
  fs_->GetSegmentManager().DisableFgGc();

  // Get old block address.
  zx::result old_addrs_or = file->FindAddresses(0, 1);
  ASSERT_TRUE(old_addrs_or.is_ok());

  {
    fs::SharedLock lock(f2fs::GetGlobalLock());
    auto pages_or = file->WriteBegin(0, kBlockSize);
    ASSERT_TRUE(pages_or.is_ok());
    ASSERT_TRUE(pages_or->front()->IsDirty());
  }
  // If kPageColdData flag is not set, allocate its block on warm data segments.
  auto expected = fs_->GetSegmentManager().NextFreeBlkAddr(CursegType::kCursegWarmData);
  ASSERT_EQ(file->Writeback(true, true), 1UL);
  zx::result new_addrs_or = file->FindAddresses(0, 1);
  ASSERT_TRUE(new_addrs_or.is_ok());
  ASSERT_EQ(expected, new_addrs_or->front());

  {
    fs::SharedLock lock(f2fs::GetGlobalLock());
    auto pages_or = file->WriteBegin(0, kBlockSize);
    ASSERT_TRUE(pages_or.is_ok());
    ASSERT_TRUE(pages_or->front()->IsDirty());
    pages_or->front()->SetColdData();
  }
  // If kPageColdData flag is set, allocate its block on cold data segments.
  expected = fs_->GetSegmentManager().NextFreeBlkAddr(CursegType::kCursegColdData);
  ASSERT_EQ(file->Writeback(true, true), 1UL);
  new_addrs_or = file->FindAddresses(0, 1);
  ASSERT_TRUE(new_addrs_or.is_ok());
  ASSERT_EQ(new_addrs_or->front(), expected);
  {
    LockedPage data_page;
    ASSERT_EQ(file->GrabLockedPage(0, &data_page), ZX_OK);
    ASSERT_FALSE(data_page->IsColdData());
  }

  file->Close();
}

TEST_F(GcTest, OrphanFileGc) {
  DirtySeglistInfo *dirty_info = &fs_->GetSegmentManager().GetDirtySegmentInfo();
  FreeSegmapInfo *free_info = &fs_->GetSegmentManager().GetFreeSegmentInfo();

  zx::result vn = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(vn.is_ok()) << vn.status_string();
  auto file = fbl::RefPtr<File>::Downcast(*std::move(vn));

  uint8_t buffer[kBlockSize] = {
      0xAA,
  };
  FileTester::AppendToFile(file.get(), buffer, kBlockSize);
  file->Writeback(true, true);

  fs_->GetSegmentManager().AllocateNewSegments();
  fs_->SyncFs();

  zx::result addrs_or = file->FindAddresses(0, 1);
  ASSERT_TRUE(addrs_or.is_ok());

  // gc target segno
  auto target_segno = fs_->GetSegmentManager().GetSegmentNumber(addrs_or->front());

  // Check victim seg is dirty
  ASSERT_TRUE(dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirty)].GetOne(target_segno));
  ASSERT_TRUE(free_info->free_segmap.GetOne(target_segno));

  // Unlink |file| while keeping open
  FileTester::DeleteChild(root_dir_.get(), "test", false);

  fbl::RefPtr<fs::Vnode> deleted;
  ASSERT_EQ(root_dir_->Lookup("test", &deleted), ZX_ERR_NOT_FOUND);

  // Do gc
  ASSERT_EQ(GcTester::DoGarbageCollect(fs_->GetSegmentManager(), target_segno, GcType::kFgGc),
            ZX_OK);

  // It should be able to keep blocks even after gc.
  ASSERT_EQ(file->GetBlockCount(), 1U);
  ASSERT_EQ(root_dir_->Lookup("test", &deleted), ZX_ERR_NOT_FOUND);

  // Check victim seg is clean
  ASSERT_FALSE(dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirty)].GetOne(target_segno));

  // Check if the paged vmo keeps data
  uint8_t read[kBlockSize] = {
      0,
  };
  FileTester::ReadFromFile(file.get(), read, sizeof(read), 0);
  ASSERT_EQ(std::memcmp(buffer, read, sizeof(read)), 0);

  ASSERT_EQ(file->Close(), ZX_OK);
  file = nullptr;

  ASSERT_EQ(root_dir_->Lookup("test", &deleted), ZX_ERR_NOT_FOUND);
}

TEST_F(GcTest, ReadVmoDuringGc) {
  DirtySeglistInfo *dirty_info = &fs_->GetSegmentManager().GetDirtySegmentInfo();
  FreeSegmapInfo *free_info = &fs_->GetSegmentManager().GetFreeSegmentInfo();

  zx::result vn = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(vn.is_ok()) << vn.status_string();
  auto file = fbl::RefPtr<File>::Downcast(*std::move(vn));

  uint8_t buffer[kBlockSize] = {
      0xAA,
  };
  FileTester::AppendToFile(file.get(), buffer, kBlockSize);
  file->Writeback(true, true);

  fs_->GetSegmentManager().AllocateNewSegments();
  fs_->SyncFs();

  zx::result addrs_or = file->FindAddresses(0, 1);
  ASSERT_TRUE(addrs_or.is_ok());

  // gc target segno
  auto target_segno = fs_->GetSegmentManager().GetSegmentNumber(addrs_or->front());

  // Check victim seg is dirty
  ASSERT_TRUE(dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirty)].GetOne(target_segno));
  ASSERT_TRUE(free_info->free_segmap.GetOne(target_segno));

  // Evict |file| to delete paged_vmo
  ASSERT_EQ(file->Close(), ZX_OK);
  ASSERT_EQ(fs_->GetVCache().Evict(file.get()), ZX_OK);
  file.reset();

  // Do gc
  ASSERT_EQ(GcTester::DoGarbageCollect(fs_->GetSegmentManager(), target_segno, GcType::kFgGc),
            ZX_OK);

  // Check victim seg is clean
  ASSERT_FALSE(dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirty)].GetOne(target_segno));

  fbl::RefPtr<fs::Vnode> vnode;
  FileTester::Lookup(root_dir_.get(), "test", &vnode);
  file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  // Check data after gc
  uint8_t read[kBlockSize] = {
      0,
  };
  FileTester::ReadFromFile(file.get(), read, sizeof(read), 0);
  ASSERT_EQ(std::memcmp(buffer, read, sizeof(read)), 0);

  ASSERT_EQ(file->Close(), ZX_OK);
  file = nullptr;
}

class GcTestWithLargeSec : public GcTest,
                           public testing::WithParamInterface<std::pair<uint64_t, uint32_t>> {
 public:
  GcTestWithLargeSec()
      : GcTest(TestOptions{.block_count = GetParam().first,
                           .mkfs_options = MkfsOptions{.segs_per_sec = GetParam().second}}) {}
};

TEST_P(GcTestWithLargeSec, SegmentDirtyInfo) TA_NO_THREAD_SAFETY_ANALYSIS {
  constexpr uint32_t kInvalidRatio = 25;
  auto files = MakeGcTriggerCondition(kInvalidRatio);
  DirtySeglistInfo *dirty_info = &fs_->GetSegmentManager().GetDirtySegmentInfo();

  // Get Victim
  uint32_t last_victim =
      fs_->GetSegmentManager().GetLastVictim(static_cast<int>(GcMode::kGcGreedy));
  auto victim_seg_or = fs_->GetSegmentManager().GetVictimByDefault(
      GcType::kFgGc, CursegType::kNoCheckType, AllocType::kLFS);
  ASSERT_FALSE(victim_seg_or.is_error());
  uint32_t victim_seg = victim_seg_or.value();
  fs_->GetSegmentManager().SetLastVictim(static_cast<int>(GcMode::kGcGreedy), last_victim);
  fs_->GetSegmentManager().SetCurVictimSec(kNullSecNo);

  // Check at least one of victim seg is dirty
  bool is_dirty = false;
  const uint32_t start_segno = victim_seg - (victim_seg % fs_->GetSuperblockInfo().GetSegsPerSec());
  for (uint32_t i = 0; i < fs_->GetSuperblockInfo().GetSegsPerSec(); ++i) {
    is_dirty |=
        dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirty)].GetOne(start_segno + i);
  }
  ASSERT_TRUE(is_dirty);

  // Copy prev nr_dirty info
  int prev_nr_dirty[static_cast<int>(DirtyType::kNrDirtytype)] = {};
  memcpy(prev_nr_dirty, dirty_info->nr_dirty, sizeof(prev_nr_dirty));

  // Trigger GC
  size_t to_secure = files.size() * kFileBlocks * kInvalidRatio / 100;
  auto result = fs_->StartGc(safemath::checked_cast<uint32_t>(to_secure));
  ASSERT_FALSE(result.is_error()) << result.status_string();

  // Check victim seg is clean
  for (uint32_t i = 0; i < fs_->GetSuperblockInfo().GetSegsPerSec(); ++i) {
    ASSERT_FALSE(
        dirty_info->dirty_segmap[static_cast<int>(DirtyType::kDirty)].GetOne(start_segno + i));
  }

  // Check nr_dirty decreased
  for (int i = static_cast<int>(DirtyType::kDirtyHotData); i <= static_cast<int>(DirtyType::kDirty);
       ++i) {
    ASSERT_TRUE(dirty_info->nr_dirty[i] <= prev_nr_dirty[i]);
  }
}

TEST_P(GcTestWithLargeSec, SegmentFreeInfo) TA_NO_THREAD_SAFETY_ANALYSIS {
  constexpr uint32_t kInvalidRatio = 25;
  auto files = MakeGcTriggerCondition(kInvalidRatio);
  FreeSegmapInfo *free_info = &fs_->GetSegmentManager().GetFreeSegmentInfo();

  // Get Victim
  uint32_t last_victim =
      fs_->GetSegmentManager().GetLastVictim(static_cast<int>(GcMode::kGcGreedy));
  auto victim_seg_or = fs_->GetSegmentManager().GetVictimByDefault(
      GcType::kFgGc, CursegType::kNoCheckType, AllocType::kLFS);
  ASSERT_FALSE(victim_seg_or.is_error());
  uint32_t victim_seg = victim_seg_or.value();
  fs_->GetSegmentManager().SetLastVictim(static_cast<int>(GcMode::kGcGreedy), last_victim);
  fs_->GetSegmentManager().SetCurVictimSec(kNullSecNo);
  uint32_t victim_sec = fs_->GetSegmentManager().GetSecNo(victim_seg);

  // Check victim sec is not free
  ASSERT_TRUE(free_info->free_secmap.GetOne(victim_sec));

  // Trigger GC
  size_t to_secure = kFileBlocks;
  auto result = fs_->StartGc(safemath::checked_cast<uint32_t>(to_secure));
  ASSERT_FALSE(result.is_error()) << result.status_string();

  fs_->SyncFs();
  // Check victim sec is freed
  ASSERT_FALSE(free_info->free_secmap.GetOne(victim_sec));
}

TEST_P(GcTestWithLargeSec, SecureSpaceFromDataSegments) {
  constexpr uint32_t kInvalidRatio = 25;
  auto files = MakeGcTriggerCondition(kInvalidRatio);
  size_t to_secure = std::min(safemath::checked_cast<size_t>(kFileBlocks),
                              files.size() * kFileBlocks * kInvalidRatio / 100);
  // It should be able to create new files on free blocks that gc acquires.
  for (uint32_t i = 0; i < to_secure; ++i) {
    std::string file_name = "_" + std::to_string(i);
    zx::result test_file = root_dir_->Create(file_name, fs::CreationType::kFile);
    ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
    test_file->Close();
  }
}

TEST_P(GcTestWithLargeSec, SecureSpaceFromNodeSegments) {
  size_t num_files = 0;
  std::vector<fbl::RefPtr<VnodeF2fs>> deleted;
  while (true) {
    std::string file_name = "_" + std::to_string(num_files);
    zx::result test_file = root_dir_->Create(file_name, fs::CreationType::kFile);
    if (test_file.is_error()) {
      break;
    }
    test_file->Close();
    if (num_files++ % 2) {
      deleted.push_back(fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_file)));
    }
  }
  fs_->SyncFs();

  DirtySeglistInfo &dirty_info = fs_->GetSegmentManager().GetDirtySegmentInfo();
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyWarmNode)], 0);
  // Make dirty node segments
  FileTester::DeleteChildren(deleted, root_dir_, deleted.size());
  deleted.clear();
  fs_->SyncFs();
  ASSERT_EQ(dirty_info.nr_dirty[static_cast<int>(DirtyType::kDirtyWarmNode)],
            safemath::checked_cast<int>(num_files / kDefaultBlocksPerSegment));

  // It should be able to create new files on free segments that gc acquires from dirty segments.
  for (uint32_t i = 0; i < num_files; ++i) {
    if (i % 2) {
      std::string file_name = "_" + std::to_string(i);
      zx::result test_file = root_dir_->Create(file_name, fs::CreationType::kFile);
      ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
      test_file->Close();
    }
  }
}

TEST_P(GcTestWithLargeSec, GcConsistency) TA_NO_THREAD_SAFETY_ANALYSIS {
  constexpr uint32_t kInvalidRatio = 25;
  auto files = MakeGcTriggerCondition(kInvalidRatio);

  // It secures enough free sections.
  size_t to_secure = files.size() * kFileBlocks * kInvalidRatio / 100;
  auto secs_or = fs_->StartGc(safemath::checked_cast<uint32_t>(to_secure));
  ASSERT_TRUE(secs_or.is_ok());

  for (auto &file : files) {
    fbl::RefPtr<fs::Vnode> vn;
    FileTester::Lookup(root_dir_.get(), file->GetNameView(), &vn);
    ASSERT_EQ(vn, file);
    char buf[kBlockSize] = {
        0,
    };
    FileTester::ReadFromFile(file.get(), buf, sizeof(buf), 0);
    f2fs_hash_t hash = DentryHash(file->GetNameView());
    ASSERT_EQ(std::memcmp(buf, &hash, sizeof(hash)), 0);
    file->Close();
  }
}

const std::array<std::pair<uint64_t, uint32_t>, 2> kSecParams = {
    {{kDefaultSectorCount, 1}, {2 * kDefaultSectorCount, 2}}};
INSTANTIATE_TEST_SUITE_P(GcTestWithLargeSec, GcTestWithLargeSec, ::testing::ValuesIn(kSecParams));

class GcTestWithSsrAndLfs : public GcTest, public testing::WithParamInterface<bool> {
 public:
  GcTestWithSsrAndLfs() { mount_options_.SetValue(MountOption::kForceLfs, GetParam()); }
};

TEST_P(GcTestWithSsrAndLfs, TruncateAndLargeFsync) {
  zx::result test_file = root_dir_->Create("test", fs::CreationType::kFile);
  ASSERT_TRUE(test_file.is_ok()) << test_file.status_string();
  auto file = fbl::RefPtr<File>::Downcast(*std::move(test_file));
  std::array<uint8_t, kBlockSize> buf;
  size_t num_blocks = 0;
  size_t out;
  // Append |file| until the out of space
  while (true) {
    zx_status_t ret =
        FileTester::Write(file.get(), buf.data(), sizeof(buf), num_blocks * sizeof(buf), &out);
    if (ret == ZX_OK) {
      LockedPage page;
      zx_status_t ret = file->GrabLockedPage(num_blocks, &page);
      ASSERT_EQ(ret, ZX_OK);
      ASSERT_TRUE(page->IsDirty());
      ++num_blocks;
      continue;
    }
    ASSERT_EQ(ret, ZX_ERR_NO_SPACE);
    break;
  }
  ASSERT_EQ(file->GetSize(), num_blocks * sizeof(buf));
  file->SyncFile();
  // 4KiB Rand W. & fsync() for 10 * the size of |file|
  for (uint32_t j = 1; j <= 10; ++j) {
    for (size_t i = 0; i < num_blocks; ++i) {
      zx_status_t ret =
          FileTester::Write(file.get(), buf.data(), sizeof(buf), i * sizeof(buf), &out);
      ASSERT_EQ(ret, ZX_OK);
    }
    file->Truncate((num_blocks - j) * kBlockSize);
    file->SyncFile();
  }

  file->Close();
}

TEST_P(GcTestWithSsrAndLfs, FillAndRemount) {
  zx::result test_dir = root_dir_->Create("test", fs::CreationType::kDirectory);
  ASSERT_TRUE(test_dir.is_ok()) << test_dir.status_string();
  size_t file_no = 0;
  const std::string prefix(kMaxNameLen - 10, '.');
  while (true) {
    zx::result child_or =
        test_dir->Create(std::string(prefix + std::to_string(file_no++)), fs::CreationType::kFile);
    if (child_or.is_error()) {
      break;
    }
    child_or->Close();
  }
  FX_LOGS(WARNING) << " created : " << file_no;
  test_dir->Close();
  *test_dir = nullptr;
  // No victim is available since there is no dirty segment.
  zx::result victim_or =
      fs_->GetSegmentManager().GetGcVictim(GcType::kFgGc, CursegType::kNoCheckType);
  ASSERT_TRUE(victim_or.is_error());
  Remount();

  // Checkpoint writes during remount yield dirty segments.
  victim_or = fs_->GetSegmentManager().GetGcVictim(GcType::kFgGc, CursegType::kNoCheckType);
  ASSERT_TRUE(victim_or.is_ok());
  size_t before = *victim_or;
  Remount();

  // GC should not happen.
  victim_or = fs_->GetSegmentManager().GetGcVictim(GcType::kFgGc, CursegType::kNoCheckType);
  ASSERT_TRUE(victim_or.is_ok());
  ASSERT_EQ(before, *victim_or);
  before = *victim_or;
  // Punch holes on node segments.
  size_t num_holes = 0;
  {
    fbl::RefPtr<fs::Vnode> vnode;
    FileTester::Lookup(root_dir_.get(), "test", &vnode);
    auto dir = fbl::RefPtr<Dir>::Downcast(std::move(vnode));
    for (size_t i = 1;; i += 2) {
      if (dir->Unlink(std::string(prefix + std::to_string(i)), false) != ZX_OK) {
        break;
      }
      num_holes++;
    }
    dir->Close();
    FX_LOGS(WARNING) << " deleted : " << num_holes;
  }
  Remount();

  // GC should not happen.
  victim_or = fs_->GetSegmentManager().GetGcVictim(GcType::kFgGc, CursegType::kNoCheckType);
  ASSERT_TRUE(victim_or.is_ok());
  ASSERT_EQ(before, *victim_or);

  // Fill holes with data
  constexpr char buf[kBlockSize] = {
      0,
  };
  fbl::RefPtr<fs::Vnode> vnode, dir;
  FileTester::Lookup(root_dir_.get(), "test", &dir);
  FileTester::Lookup(fbl::RefPtr<VnodeF2fs>::Downcast(dir).get(), std::string(prefix + "0"),
                     &vnode);
  size_t num_blocks = 0;
  fbl::RefPtr<File> file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  while (true) {
    size_t out;
    zx_status_t ret =
        FileTester::Write(file.get(), buf, sizeof(buf), num_blocks++ * sizeof(buf), &out);
    if (ret != ZX_OK || num_blocks >= num_holes / 2) {
      break;
    }
  }
  file->Close();
  file.reset();
  FX_LOGS(WARNING) << " written data blocks : " << num_blocks;

  // Fill holes with vnodes
  num_blocks = 0;
  {
    auto test_dir = fbl::RefPtr<Dir>::Downcast(dir);
    while (true) {
      zx::result child_or = test_dir->Create(std::string(prefix + std::to_string(++file_no)),
                                             fs::CreationType::kFile);
      if (child_or.is_error()) {
        break;
      }
      child_or->Close();
      num_blocks++;
    }
  }
  FX_LOGS(WARNING) << " created vnodes : " << num_blocks;
  dir->Close();
  dir.reset();
  Remount();

  // GC ran already.
  victim_or = fs_->GetSegmentManager().GetGcVictim(GcType::kFgGc, CursegType::kNoCheckType);
  ASSERT_TRUE(victim_or.is_ok());
  if (fs_->GetSuperblockInfo().TestOpt(MountOption::kForceLfs)) {
    ASSERT_NE(before, *victim_or);
  } else {
    ASSERT_EQ(before, *victim_or);
  }
}

const std::array<bool, 2> kAllocParams = {false, true};
INSTANTIATE_TEST_SUITE_P(GcTestWithSsrAndLfs, GcTestWithSsrAndLfs,
                         ::testing::ValuesIn(kAllocParams));

}  // namespace
}  // namespace f2fs
