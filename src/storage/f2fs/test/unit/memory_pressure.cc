// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.memorypressure/cpp/fidl.h>

#include <unordered_set>

#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

class MemoryPressureTest : public F2fsFakeDevTestFixture {
 public:
  MemoryPressureTest()
      : F2fsFakeDevTestFixture(TestOptions{
            .block_count = kSectorCount100MiB,
        }) {}

  uint32_t GetCachedVnodeCount() {
    uint32_t cached_vnode_count = 0;
    fs_->GetVCache().ForAllVnodes([&cached_vnode_count](fbl::RefPtr<VnodeF2fs> &vnode) {
      ++cached_vnode_count;
      return ZX_OK;
    });
    return cached_vnode_count;
  }

  // mode: S_IFREG, S_IFDIR
  zx::result<fbl::RefPtr<fs::Vnode>> Create(std::string name, umode_t mode) {
    return root_dir_->CreateWithMode(name, mode);
  }

  void SetMemoryPressure(MemoryPressure level) { fs_->SetMemoryPressure(level); }
};

TEST_F(MemoryPressureTest, Basic) TA_NO_THREAD_SAFETY_ANALYSIS {
  char buf[Page::Size()];

  // Create file
  zx::result file_or = Create("test", S_IFREG);
  ASSERT_TRUE(file_or.is_ok());
  auto file = fbl::RefPtr<File>::Downcast(*std::move(file_or));

  zx::result file2_or = Create("test2", S_IFREG);
  ASSERT_TRUE(file2_or.is_ok());

  // Hold refptr pf file2
  auto file2 = fbl::RefPtr<File>::Downcast(*std::move(file2_or));
  FileTester::AppendToFile(file2.get(), buf, Page::Size());
  file2->Close();
  file2.reset();

  size_t before = GetCachedVnodeCount();
  ASSERT_EQ(before, 3U);

  // Make dirty pages under high memory pressure
  SetMemoryPressure(MemoryPressure::kHigh);
  for (int i = 0; i < kAddrsPerInode + kAddrsPerBlock + 1; ++i) {
    FileTester::AppendToFile(file.get(), buf, Page::Size());
    ASSERT_TRUE(fs_->GetSuperblockInfo().GetPageCount(CountType::kDirtyData) <= 1);
    ASSERT_TRUE(fs_->GetSuperblockInfo().GetPageCount(CountType::kDirtyNodes) <= 2);
  }
  file->Close();
  file.reset();

  fs_->WaitForAvailableMemory();
  fs_->GetVCache().Shrink();

  ASSERT_TRUE(GetCachedVnodeCount() < before);
  ASSERT_EQ(fs_->GetSuperblockInfo().GetPageCount(CountType::kDirtyData), 0);
  ASSERT_EQ(fs_->GetSuperblockInfo().GetPageCount(CountType::kDirtyNodes), 0);
}

TEST_F(MemoryPressureTest, AsyncCheckpoint) {
  zx::result test_file = Create("test", S_IFREG);
  ASSERT_TRUE(test_file.is_ok());
  fbl::RefPtr<f2fs::File> file = fbl::RefPtr<f2fs::File>::Downcast(*std::move(test_file));
  constexpr size_t kNTry = kAddrsPerInode + kAddrsPerBlock + 1;

  char buf[Page::Size()];
  for (size_t i = 0; i < kNTry; ++i) {
    zx::result dir = Create("dir_" + std::to_string(i), S_IFDIR);
    ASSERT_TRUE(dir.is_ok()) << dir.status_string();
    dir->Close();
  }

  SetMemoryPressure(MemoryPressure::kHigh);
  std::thread write_thread = std::thread([&]() {
    for (size_t i = 0; i < kNTry; ++i) {
      // Test async. task running writeback and checkpoint
      FileTester::AppendToFile(file.get(), buf, Page::Size());
    }
  });

  std::thread fsync_thread = std::thread([&]() {
    for (size_t i = 0; i < kNTry; ++i) {
      // Delete dentries to make its child do checkpoint for fsync()
      FileTester::DeleteChild(root_dir_.get(), "dir_" + std::to_string(i), true);
      // Test sync. checkpoint
      ASSERT_EQ(file->SyncFile(false), ZX_OK);
    }
  });

  write_thread.join();
  fsync_thread.join();
  // data blocks + 2 dnode blocks
  ASSERT_EQ(file->GetBlockCount(), kNTry + 2U);
  for (size_t i = 0; i < kNTry; ++i) {
    fbl::RefPtr<fs::Vnode> vn;
    ASSERT_EQ(root_dir_->Lookup("dir_" + std::to_string(i), &vn), ZX_ERR_NOT_FOUND);
  }

  file->Close();
  file.reset();
}

}  // namespace
}  // namespace f2fs
