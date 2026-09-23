// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/default.h>

#include <gtest/gtest.h>

#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "src/storage/minfs/bcache.h"
#include "src/storage/minfs/file.h"
#include "src/storage/minfs/format.h"
#include "src/storage/minfs/runner.h"

namespace minfs {
namespace {

using block_client::FakeBlockDevice;

TEST(ValidateDirentTest, ShortReadOfDirentNameFails) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  constexpr uint64_t kBlockCount = 1 << 20;
  auto device = std::make_unique<FakeBlockDevice>(kBlockCount, kMinfsBlockSize);

  auto bcache_or = Bcache::Create(std::move(device), kBlockCount);
  ASSERT_TRUE(bcache_or.is_ok());
  ASSERT_TRUE(Mkfs(bcache_or.value().get()).is_ok());
  MountOptions options = {};

  auto fs_or = Runner::Create(loop.dispatcher(), std::move(bcache_or.value()), options);
  ASSERT_TRUE(fs_or.is_ok());

  // Create a child "foo" in the root directory.
  {
    auto root = fs_or->minfs().VnodeGet(kMinfsRootIno);
    ASSERT_TRUE(root.is_ok());
    zx::result fs_child = root->Create("foo", fs::CreationType::kFile);
    ASSERT_TRUE(fs_child.is_ok()) << fs_child.status_string();
    auto child = fbl::RefPtr<File>::Downcast(*std::move(fs_child));
    EXPECT_EQ(child->Close(), ZX_OK);
  }

  // Sync the journal to ensure the "foo" entry is written to the metadata blocks on disk.
  ASSERT_TRUE(fs_or->minfs().BlockingJournalSync().is_ok());

  // Destroy the runner to write back and access the raw blocks.
  uint32_t inode_block = fs_or->minfs().Info().ino_block;
  bcache_or = zx::ok(Runner::Destroy(std::move(fs_or.value())));

  // Modify the root inode size on disk to be smaller than the full dirent size of "foo".
  // Total size for "." and ".." is DirentSize(1) + DirentSize(2) = 16 + 16 = 32 bytes.
  // "foo" is at offset 32. DirentSize(3) is 16 bytes. So full entry ends at 48.
  // We set root directory size to 44 bytes, so a read will only return 12 bytes for "foo" (which is
  // < 16).
  Inode inodes[kMinfsInodesPerBlock];
  ASSERT_TRUE(bcache_or->Readblk(inode_block, &inodes).is_ok());
  inodes[kMinfsRootIno].size = 44;
  ASSERT_TRUE(bcache_or->Writeblk(inode_block, &inodes).is_ok());

  // Re-create the runner.
  fs_or = Runner::Create(loop.dispatcher(), std::move(bcache_or.value()), options);
  ASSERT_TRUE(fs_or.is_ok());

  // Attempt to lookup "foo". It should fail with ZX_ERR_IO because ValidateDirent detects
  // that the dirent bounds at offset 32 extend past the directory size of 44.
  {
    auto root = fs_or->minfs().VnodeGet(kMinfsRootIno);
    ASSERT_TRUE(root.is_ok());
    EXPECT_EQ(root->GetSize(), 44u);
    fbl::RefPtr<fs::Vnode> unused_child;
    EXPECT_EQ(root->Lookup("foo", &unused_child), ZX_ERR_IO);
  }

  [[maybe_unused]] auto bcache = Runner::Destroy(std::move(fs_or.value()));
}

TEST(DirectoryTest, LookupInvalidNameRejectsTraversal) {
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);

  constexpr uint64_t kBlockCount = 1 << 20;
  auto device = std::make_unique<FakeBlockDevice>(kBlockCount, kMinfsBlockSize);

  auto bcache_or = Bcache::Create(std::move(device), kBlockCount);
  ASSERT_TRUE(bcache_or.is_ok());
  ASSERT_TRUE(Mkfs(bcache_or.value().get()).is_ok());
  MountOptions options = {};

  auto fs_or = Runner::Create(loop.dispatcher(), std::move(bcache_or.value()), options);
  ASSERT_TRUE(fs_or.is_ok());

  {
    auto root = fs_or->minfs().VnodeGet(kMinfsRootIno);
    ASSERT_TRUE(root.is_ok());

    fbl::RefPtr<fs::Vnode> unused_child;
    EXPECT_EQ(root->Lookup("..", &unused_child), ZX_ERR_INVALID_ARGS);
    EXPECT_EQ(root->Lookup(".", &unused_child), ZX_ERR_INVALID_ARGS);
    EXPECT_EQ(root->Lookup("foo/bar", &unused_child), ZX_ERR_INVALID_ARGS);
    EXPECT_EQ(root->Lookup("../foo", &unused_child), ZX_ERR_INVALID_ARGS);
    EXPECT_EQ(root->Lookup("", &unused_child), ZX_ERR_INVALID_ARGS);
  }

  [[maybe_unused]] auto bcache = Runner::Destroy(std::move(fs_or.value()));
}

}  // namespace
}  // namespace minfs
