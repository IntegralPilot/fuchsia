// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <unordered_set>

#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

class VnodeCacheTest : public SingleFileTest {
 public:
  VnodeCacheTest()
      : SingleFileTest(S_IFDIR, TestOptions{.mount_options = {{MountOption::kInlineDentry, 0}}}) {}

  uint32_t GetCachedVnodeCount() {
    uint32_t cached_vnode_count = 0;
    fs_->GetVCache().ForAllVnodes([&cached_vnode_count](fbl::RefPtr<VnodeF2fs> &vnode) {
      ++cached_vnode_count;
      return ZX_OK;
    });
    return cached_vnode_count;
  }

  uint32_t GetDirtyVnodeCount() {
    uint32_t dirty_vnode_count = 0;
    fs_->GetVCache().ForDirtyVnodesIf(
        [&dirty_vnode_count](fbl::RefPtr<VnodeF2fs> &vnode) {
          ++dirty_vnode_count;
          return ZX_OK;
        },
        nullptr);
    return dirty_vnode_count;
  }
};

TEST_F(VnodeCacheTest, Basic) {
  Dir *test_dir_ptr = &vnode<Dir>();

  std::unordered_set<std::string> child_set = {"a", "b", "c", "d", "e"};
  std::vector<ino_t> child_ino_set(0);
  std::vector<std::string> deleted_child_set(0);

  // create a, b, c, d, e in test
  for (auto iter : child_set) {
    FileTester::CreateChild(test_dir_ptr, S_IFDIR, iter);
  }

  // check if {a, b, c, d, e} vnodes are in both containers.
  for (auto iter : child_set) {
    fbl::RefPtr<fs::Vnode> vn;
    FileTester::Lookup(test_dir_ptr, iter, &vn);
    ASSERT_TRUE(vn);
    VnodeF2fs *raw_vnode = reinterpret_cast<VnodeF2fs *>(vn.get());
    ASSERT_TRUE(raw_vnode->IsDirty());
    ASSERT_EQ((*raw_vnode).fbl::DoublyLinkedListable<fbl::RefPtr<VnodeF2fs>>::InContainer(), true);
    ASSERT_EQ((*raw_vnode).fbl::WAVLTreeContainable<VnodeF2fs *>::InContainer(), true);
    child_ino_set.push_back(raw_vnode->GetKey());
    vn->Close();
    vn.reset();
  }
  ASSERT_EQ(test_dir_ptr->GetSize(), kBlockSize);

  // flush dirty vnodes.
  fs_->SyncFs();

  // check if dirty vnodes are removed from dirty_list_
  ASSERT_TRUE(fs_->GetVCache().IsDirtyListEmpty());
  for (auto iter : child_set) {
    fbl::RefPtr<fs::Vnode> vn;
    FileTester::Lookup(test_dir_ptr, iter, &vn);
    ASSERT_TRUE(vn);
    VnodeF2fs *raw_vnode = reinterpret_cast<VnodeF2fs *>(vn.get());
    ASSERT_FALSE(raw_vnode->IsDirty());
    ASSERT_FALSE((*raw_vnode).fbl::DoublyLinkedListable<fbl::RefPtr<VnodeF2fs>>::InContainer());
    ASSERT_EQ((*raw_vnode).fbl::WAVLTreeContainable<VnodeF2fs *>::InContainer(), true);
    vn->Close();
  }

  // remove "b" and "d".
  FileTester::DeleteChild(test_dir_ptr, "b");
  deleted_child_set.push_back("b");
  FileTester::DeleteChild(test_dir_ptr, "d");
  deleted_child_set.push_back("d");

  // free nids for b and d.
  fs_->SyncFs();

  // check if nodemgr and vnode cache remove b and d.
  int i = 0;
  for (auto iter : child_set) {
    fbl::RefPtr<fs::Vnode> vn;
    auto child = find(deleted_child_set.begin(), deleted_child_set.end(), iter);
    FileTester::Lookup(test_dir_ptr, iter, &vn);
    if (child != deleted_child_set.end()) {
      ASSERT_FALSE(vn);
      ino_t ino = child_ino_set.at(i);
      fbl::RefPtr<VnodeF2fs> vn2;
      ASSERT_EQ(fs_->GetVCache().Lookup(ino, &vn2), ZX_ERR_NOT_FOUND);
      NodeInfo ni;
      fs_->GetNodeManager().GetNodeInfo(ino, ni);
      ASSERT_FALSE(ni.blk_addr);
    } else {
      ASSERT_TRUE(vn);
      VnodeF2fs *raw_vnode = reinterpret_cast<VnodeF2fs *>(vn.get());
      ASSERT_FALSE(raw_vnode->IsDirty());
      ASSERT_FALSE((*raw_vnode).fbl::DoublyLinkedListable<fbl::RefPtr<VnodeF2fs>>::InContainer());
      ASSERT_EQ((*raw_vnode).fbl::WAVLTreeContainable<VnodeF2fs *>::InContainer(), true);
      vn->Close();
      ino_t ino = child_ino_set.at(i);
      fbl::RefPtr<VnodeF2fs> vn2;
      ASSERT_EQ(fs_->GetVCache().Lookup(ino, &vn2), ZX_OK);
      NodeInfo ni;
      fs_->GetNodeManager().GetNodeInfo(ino, ni);
      ASSERT_TRUE(ni.blk_addr);
    }
    ++i;
  }
}

TEST_F(VnodeCacheTest, VnodeCacheExceptionCase) {
  fbl::RefPtr<VnodeF2fs> new_vnode;
  // Check Create() exception
  ASSERT_EQ(GetDirtyVnodeCount(), 2U);
  ASSERT_EQ(GetCachedVnodeCount(), 2U);
  ASSERT_EQ(fs_->GetVnode(fs_->GetSuperblockInfo().GetNodeIno()).status_value(), ZX_ERR_NOT_FOUND);

  // Check Add() exception
  auto &test_vnode = vnode<Dir>();
  ASSERT_EQ(GetDirtyVnodeCount(), 2U);
  ASSERT_EQ(GetCachedVnodeCount(), 2U);
  ASSERT_EQ(fs_->GetVCache().Add(&test_vnode), ZX_ERR_ALREADY_EXISTS);
  ASSERT_EQ(GetDirtyVnodeCount(), 2U);
  ASSERT_EQ(GetCachedVnodeCount(), 2U);

  // Check AddDirty() exception
  ASSERT_EQ(fs_->GetVCache().AddDirty(test_vnode), ZX_ERR_ALREADY_EXISTS);
  ASSERT_EQ(GetDirtyVnodeCount(), 2U);
  ASSERT_EQ(GetCachedVnodeCount(), 2U);

  // Check ForAllVnodes() callback function
  ASSERT_EQ(
      fs_->GetVCache().ForAllVnodes([](fbl::RefPtr<VnodeF2fs> &vnode) { return ZX_ERR_STOP; }),
      ZX_OK);

  ASSERT_EQ(fs_->GetVCache().ForAllVnodes(
                [](fbl::RefPtr<VnodeF2fs> &vnode) { return ZX_ERR_INVALID_ARGS; }),
            ZX_ERR_INVALID_ARGS);

  ASSERT_EQ(
      fs_->GetVCache().ForDirtyVnodesIf([](fbl::RefPtr<VnodeF2fs> &vnode) { return ZX_ERR_STOP; },
                                        [](fbl::RefPtr<VnodeF2fs> &vnode) { return ZX_OK; }),
      ZX_OK);

  ASSERT_EQ(fs_->GetVCache().ForDirtyVnodesIf(
                [](fbl::RefPtr<VnodeF2fs> &vnode) { return ZX_ERR_INVALID_ARGS; },
                [](fbl::RefPtr<VnodeF2fs> &vnode) { return ZX_OK; }),
            ZX_ERR_INVALID_ARGS);

  // Check Reset()
  fs_->Sync();
  ASSERT_EQ(GetDirtyVnodeCount(), 0U);
  ASSERT_EQ(GetCachedVnodeCount(), 2U);

  fs_->GetVCache().Reset();
  ASSERT_EQ(GetDirtyVnodeCount(), 0U);
  ASSERT_EQ(GetCachedVnodeCount(), 0U);
}

TEST_F(VnodeCacheTest, VnodeActivation) {
  Dir *test_dir_ptr = &vnode<Dir>();

  std::string child_name = "file";
  FileTester::CreateChild(test_dir_ptr, S_IFDIR, child_name);

  fbl::RefPtr<fs::Vnode> test_vnode;
  FileTester::Lookup(test_dir_ptr, child_name, &test_vnode);
  fbl::RefPtr<VnodeF2fs> test_f2fs_vnode = fbl::RefPtr<VnodeF2fs>::Downcast(std::move(test_vnode));
  ASSERT_TRUE(test_f2fs_vnode->IsActive());
  ASSERT_EQ(test_f2fs_vnode->GetNameView().compare(child_name), 0);
  ASSERT_EQ(test_f2fs_vnode->Close(), ZX_OK);

  auto raw_pointer = test_f2fs_vnode.get();
  test_f2fs_vnode.reset();
  // "file" is active as VnodeCache::dirty_list_ keeps its ref.
  ASSERT_TRUE(raw_pointer->IsActive());

  fs_->SyncFs();
  // "file" is inactive after checkpoint writes its vnode to disk.
  ASSERT_FALSE(raw_pointer->IsActive());

  // Get refptr for "file" from the vnode cache while it is being recycled.
  std::thread thread1 = std::thread([&]() {
    int iter = 10000;
    while (--iter) {
      ASSERT_TRUE(fs_->GetVnode(raw_pointer->Ino()).is_ok());
    }
  });
  std::thread thread2 = std::thread([&]() {
    int iter = 10000;
    while (--iter) {
      ASSERT_TRUE(fs_->GetVnode(raw_pointer->Ino()).is_ok());
    }
  });

  thread1.join();
  thread2.join();

  ASSERT_FALSE(raw_pointer->IsActive());
}

}  // namespace
}  // namespace f2fs
