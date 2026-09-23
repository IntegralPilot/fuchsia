// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>
#include <safemath/checked_math.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/lib/block_client/cpp/fake_block_device.h"
#include "unit_lib.h"

namespace f2fs {
namespace {

using DirEntryCacheTest = F2fsFakeDevTestFixture;

TEST_F(DirEntryCacheTest, Basic) {
  std::unordered_set<std::string> child_set = {"alpha", "bravo", "charlie", "delta", "echo"};

  // Create children
  for (auto &child : child_set) {
    FileTester::CreateChild(root_dir_.get(), S_IFDIR, child.c_str());
  }

  // Check if all children exist on the cache
  for (auto &child : child_set) {
    ASSERT_TRUE(root_dir_->GetDirEntryCache().IsElementInCache(child));
  }

  // Remove "bravo"
  FileTester::DeleteChild(root_dir_.get(), "bravo");
  child_set.erase("bravo");

  // Check if "bravo" is not exist on the cache
  ASSERT_FALSE(root_dir_->GetDirEntryCache().IsElementInCache("bravo"));

  // Check if all other children still exist on the cache
  for (auto &child : child_set) {
    ASSERT_TRUE(root_dir_->GetDirEntryCache().IsElementInCache(child));
  }
}

TEST_F(DirEntryCacheTest, SubDirectory) {
  // Create "alpha"
  FileTester::CreateChild(root_dir_.get(), S_IFDIR, "alpha");
  fbl::RefPtr<fs::Vnode> child_dir_vn;
  FileTester::Lookup(root_dir_.get(), "alpha", &child_dir_vn);
  ASSERT_TRUE(root_dir_->GetDirEntryCache().IsElementInCache("alpha"));

  // Create "alpha/bravo"
  fbl::RefPtr<Dir> child_dir = fbl::RefPtr<Dir>::Downcast(std::move(child_dir_vn));
  FileTester::CreateChild(child_dir.get(), S_IFDIR, "bravo");
  ASSERT_TRUE(child_dir->GetDirEntryCache().IsElementInCache("bravo"));

  // Delete "alpha/bravo"
  FileTester::DeleteChild(child_dir.get(), "bravo");
  ASSERT_FALSE(child_dir->GetDirEntryCache().IsElementInCache("bravo"));

  // Delete "alpha"
  ASSERT_EQ(child_dir->Close(), ZX_OK);
  FileTester::DeleteChild(root_dir_.get(), "alpha");
  ASSERT_FALSE(root_dir_->GetDirEntryCache().IsElementInCache("alpha"));

  // Create "alpha", and check if "alpha/bravo" is not exist
  FileTester::CreateChild(root_dir_.get(), S_IFDIR, "alpha");
  ASSERT_TRUE(root_dir_->GetDirEntryCache().IsElementInCache("alpha"));
  FileTester::Lookup(root_dir_.get(), "alpha", &child_dir_vn);
  child_dir = fbl::RefPtr<Dir>::Downcast(std::move(child_dir_vn));
  ASSERT_FALSE(child_dir->GetDirEntryCache().IsElementInCache("bravo"));

  // Create "alpha/bravo", move "alpha" to "charlie", and check if "charlie/bravo" is exist
  FileTester::CreateChild(child_dir.get(), S_IFDIR, "bravo");
  ASSERT_EQ(child_dir->Close(), ZX_OK);
  ASSERT_EQ(root_dir_->Rename(root_dir_, "alpha", "charlie", true, true), ZX_OK);
  FileTester::Lookup(root_dir_.get(), "charlie", &child_dir_vn);
  child_dir = fbl::RefPtr<Dir>::Downcast(std::move(child_dir_vn));
  ASSERT_TRUE(child_dir->GetDirEntryCache().IsElementInCache("bravo"));
  ASSERT_EQ(child_dir->Close(), ZX_OK);
}

TEST_F(DirEntryCacheTest, CacheDataValidation) {
  const uint32_t nr_child = kNrDentryInBlock * 4;

  // Create children
  for (uint32_t i = 0; i < nr_child; ++i) {
    std::string child = std::to_string(i);
    FileTester::CreateChild(root_dir_.get(), S_IFDIR, child);
    ASSERT_TRUE(root_dir_->GetDirEntryCache().IsElementInCache(child));
  }

  // Check whether cached data is valid
  auto &map = root_dir_->GetDirEntryCache().GetMap();
  for (auto &cached : map) {
    std::string child_name_from_key = cached.first;
    auto element = cached.second;

    ASSERT_EQ(element->GetName(), child_name_from_key);

    // To validate cached parent ino, read a page for cached index
    auto page_or = root_dir_->FindDataPage(element->GetInfo().page_index);
    ASSERT_TRUE(page_or.is_ok());
    DentryBlock *dentry_block = page_or->GetAddress<DentryBlock>();
    PageBitmap dentry_bitmap(dentry_block->dentry_bitmap, kNrDentryInBlock);

    size_t bit_pos = 0;
    while ((bit_pos = dentry_bitmap.FindNextBit(bit_pos)) < kNrDentryInBlock) {
      DirEntry *de = &dentry_block->dentry[bit_pos];
      uint32_t slots = (LeToCpu(de->name_len) + kNameLen - 1) / kNameLen;

      // If child name is found in the block, check if contents are valid
      if (std::string(reinterpret_cast<char *>(dentry_block->filename[bit_pos]),
                      element->GetName().length()) == element->GetName()) {
        // Validate cached dir entry
        // Make a copy for alignment
        DirEntry de_copied = *de;
        ASSERT_EQ(element->GetInfo().ino, de_copied.ino);
        break;
      }

      bit_pos += slots;
    }
    // If not found, |bit_pos| exceeds the bitmap length, |kNrDentryInBlock|
    ASSERT_LT(bit_pos, safemath::checked_cast<uint32_t>(kNrDentryInBlock));
  }
}

}  // namespace
}  // namespace f2fs
