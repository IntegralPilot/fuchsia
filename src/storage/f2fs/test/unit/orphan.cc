// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>
#include <lib/syslog/cpp/macros.h>

#include <algorithm>
#include <cstddef>
#include <numeric>
#include <vector>

#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

constexpr uint32_t kOrphanCnt = 10;

TEST(OrphanInode, RecoverOrphanInode) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);
  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  ASSERT_FALSE(fs->GetSuperblockInfo().TestCpFlags(CpFlag::kCpOrphanPresentFlag));

  // 1. Create files
  std::vector<fbl::RefPtr<VnodeF2fs>> vnodes;
  std::vector<uint32_t> inos;

  ASSERT_EQ(fs->GetSuperblockInfo().GetValidInodeCount(), static_cast<uint64_t>(1));
  ASSERT_EQ(fs->GetSuperblockInfo().GetValidNodeCount(), static_cast<uint64_t>(1));
  ASSERT_EQ(fs->GetSuperblockInfo().GetValidBlockCount(), static_cast<uint64_t>(2));

  FileTester::CreateChildren(fs.get(), vnodes, inos, root_dir, "orphan_", kOrphanCnt);
  ASSERT_EQ(vnodes.size(), kOrphanCnt);
  ASSERT_EQ(inos.size(), kOrphanCnt);

  ASSERT_EQ(fs->GetSuperblockInfo().GetValidInodeCount(), static_cast<uint64_t>(kOrphanCnt + 1));
  ASSERT_EQ(fs->GetSuperblockInfo().GetValidNodeCount(), static_cast<uint64_t>(kOrphanCnt + 1));
  ASSERT_EQ(fs->GetSuperblockInfo().GetValidBlockCount(), static_cast<uint64_t>(kOrphanCnt + 2));

  for (const auto &iter : vnodes) {
    ASSERT_EQ(iter->GetLinkCount(), static_cast<uint32_t>(1));
  }

  // 2. Make orphan inodes
  ASSERT_EQ(fs->GetVnodeSetSize(VnodeSet::kOrphan), static_cast<uint64_t>(0));
  FileTester::DeleteChildren(vnodes, root_dir, kOrphanCnt);
  ASSERT_EQ(fs->GetVnodeSetSize(VnodeSet::kOrphan), kOrphanCnt);

  for (const auto &iter : vnodes) {
    ASSERT_EQ(iter->GetLinkCount(), (uint32_t)0);
  }

  fs->SyncFs();
  ASSERT_EQ(fs->GetVnodeSetSize(VnodeSet::kOrphan), 0UL);

  // 3. Sudden power off
  for (const auto &iter : vnodes) {
    iter->Close();
  }

  vnodes.clear();
  vnodes.shrink_to_fit();
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir.reset();

  FileTester::SuddenPowerOff(std::move(fs), &bc);

  // 4. Remount and purge orphan inodes
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  ASSERT_EQ(fs->GetVnodeSetSize(VnodeSet::kOrphan), static_cast<uint64_t>(0));
  ASSERT_EQ(fs->GetSuperblockInfo().GetValidInodeCount(), static_cast<uint64_t>(1));
  ASSERT_EQ(fs->GetSuperblockInfo().GetValidNodeCount(), static_cast<uint64_t>(1));
  ASSERT_EQ(fs->GetSuperblockInfo().GetValidBlockCount(), static_cast<uint64_t>(2));

  // Check Orphan nids has been freed
  for (const auto &iter : inos) {
    NodeInfo ni;
    fs->GetNodeManager().GetNodeInfo(iter, ni);
    ASSERT_EQ(ni.blk_addr, kNullAddr);
  }

  FileTester::Unmount(std::move(fs), &bc);
}

using OrphanTest = F2fsFakeDevTestFixture;

TEST_F(OrphanTest, VnodeSet) {
  uint32_t inode_count = 100;
  std::vector<uint32_t> inos(inode_count);
  std::iota(inos.begin(), inos.end(), 0);

  for (auto ino : inos) {
    fs_->AddToVnodeSet(VnodeSet::kOrphan, ino);
  }
  ASSERT_EQ(fs_->GetVnodeSetSize(VnodeSet::kOrphan), inode_count);

  // Duplicate ino insertion
  fs_->AddToVnodeSet(VnodeSet::kOrphan, 1);
  fs_->AddToVnodeSet(VnodeSet::kOrphan, 2);
  fs_->AddToVnodeSet(VnodeSet::kOrphan, 3);
  fs_->AddToVnodeSet(VnodeSet::kOrphan, 4);
  ASSERT_EQ(fs_->GetVnodeSetSize(VnodeSet::kOrphan), inode_count);

  fs_->RemoveFromVnodeSet(VnodeSet::kOrphan, 10);
  ASSERT_EQ(fs_->GetVnodeSetSize(VnodeSet::kOrphan), inode_count - 1);

  ASSERT_FALSE(fs_->FindVnodeSet(VnodeSet::kOrphan, 10));
  ASSERT_TRUE(fs_->FindVnodeSet(VnodeSet::kOrphan, 11));
  fs_->AddToVnodeSet(VnodeSet::kOrphan, 10);

  std::vector<uint32_t> tmp_inos;
  fs_->ForAllVnodeSet(VnodeSet::kOrphan, [&tmp_inos](nid_t ino) { tmp_inos.push_back(ino); });
  ASSERT_TRUE(std::equal(inos.begin(), inos.end(), tmp_inos.begin()));

  for (auto ino : inos) {
    fs_->RemoveFromVnodeSet(VnodeSet::kOrphan, ino);
  }
}

}  // namespace
}  // namespace f2fs
