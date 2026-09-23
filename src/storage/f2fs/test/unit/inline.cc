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

TEST(InlineDirTest, InlineDirCreation) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  // Enable inline dir option
  ASSERT_EQ(options.SetValue(MountOption::kInlineDentry, 1), ZX_OK);
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);

  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  // Inline dir creation
  std::string inline_dir_name("inline");
  zx::result inline_child = root_dir->Create(inline_dir_name, fs::CreationType::kDirectory);
  ASSERT_TRUE(inline_child.is_ok()) << inline_child.status_string();

  fbl::RefPtr<VnodeF2fs> inline_child_dir =
      fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(inline_child));

  FileTester::CheckInlineDir(inline_child_dir.get());

  ASSERT_EQ(inline_child_dir->Close(), ZX_OK);
  inline_child_dir = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);

  // Disable inline dir option
  ASSERT_EQ(options.SetValue(MountOption::kInlineDentry, 0), ZX_OK);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  FileTester::CreateRoot(fs.get(), &root);
  root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  // Check if existing inline dir is still inline regardless of mount option
  fbl::RefPtr<fs::Vnode> child;
  FileTester::Lookup(root_dir.get(), inline_dir_name, &child);
  inline_child_dir = fbl::RefPtr<VnodeF2fs>::Downcast(std::move(child));
  FileTester::CheckInlineDir(inline_child_dir.get());

  // However, newly created dir should be non-inline
  std::string non_inline_dir_name("noninline");
  zx::result non_inline_child = root_dir->Create(non_inline_dir_name, fs::CreationType::kDirectory);
  ASSERT_TRUE(non_inline_child.is_ok()) << non_inline_child.status_string();

  fbl::RefPtr<VnodeF2fs> non_inline_child_dir =
      fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(non_inline_child));
  FileTester::CheckNonInlineDir(non_inline_child_dir.get());

  ASSERT_EQ(inline_child_dir->Close(), ZX_OK);
  inline_child_dir = nullptr;
  ASSERT_EQ(non_inline_child_dir->Close(), ZX_OK);
  non_inline_child_dir = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);
  EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}, &bc), ZX_OK);
}

TEST(InlineDirTest, InlineDirConvert) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  // Enable inline dir option
  ASSERT_EQ(options.SetValue(MountOption::kInlineDentry, 1), ZX_OK);
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);

  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  // Inline dir creation
  std::string inline_dir_name("inline");
  zx::result inline_child = root_dir->Create(inline_dir_name, fs::CreationType::kDirectory);
  ASSERT_TRUE(inline_child.is_ok()) << inline_child.status_string();

  fbl::RefPtr<Dir> inline_child_dir = fbl::RefPtr<Dir>::Downcast(*std::move(inline_child));

  unsigned int child_count = 0;

  // Fill all slots of inline dentry
  // Since two dentry slots are already allocated for "." and "..", decrease 2 from kNrInlineDentry
  for (; child_count < inline_child_dir->MaxInlineDentry() - 2; ++child_count) {
    umode_t mode = child_count % 2 == 0 ? S_IFDIR : S_IFREG;
    FileTester::CreateChild(inline_child_dir.get(), mode, std::to_string(child_count));
  }

  // It should be inline
  FileTester::CheckInlineDir(inline_child_dir.get());

  ASSERT_EQ(inline_child_dir->Close(), ZX_OK);
  inline_child_dir = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);

  // Disable inline dir option
  ASSERT_EQ(options.SetValue(MountOption::kInlineDentry, 0), ZX_OK);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  FileTester::CreateRoot(fs.get(), &root);
  root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  // Check if existing inline dir is still inline regardless of mount option
  fbl::RefPtr<fs::Vnode> child;
  FileTester::Lookup(root_dir.get(), inline_dir_name, &child);
  inline_child_dir = fbl::RefPtr<Dir>::Downcast(std::move(child));
  FileTester::CheckInlineDir(inline_child_dir.get());

  // If one more dentry is added, it should be converted to non-inline dir
  umode_t mode = child_count % 2 == 0 ? S_IFDIR : S_IFREG;
  FileTester::CreateChild(inline_child_dir.get(), mode, std::to_string(child_count));

  FileTester::CheckNonInlineDir(inline_child_dir.get());

  ASSERT_EQ(inline_child_dir->Close(), ZX_OK);
  inline_child_dir = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);
  EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}, &bc), ZX_OK);
}

TEST(InlineDirTest, InlineDentryOps) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  // Enable inline dir option
  ASSERT_EQ(options.SetValue(MountOption::kInlineDentry, 1), ZX_OK);
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);

  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  // Inline dir creation
  std::string inline_dir_name("inline");
  zx::result inline_child = root_dir->Create(inline_dir_name, fs::CreationType::kDirectory);
  ASSERT_TRUE(inline_child.is_ok()) << inline_child.status_string();

  fbl::RefPtr<Dir> inline_child_dir = fbl::RefPtr<Dir>::Downcast(*std::move(inline_child));

  std::unordered_set<std::string> child_set = {"a", "b", "c", "d", "e"};

  Dir *dir_ptr = inline_child_dir.get();

  for (auto iter : child_set) {
    FileTester::CreateChild(dir_ptr, S_IFDIR, iter);
  }
  FileTester::CheckChildrenFromReaddir(dir_ptr, child_set);

  // remove "b" and "d"
  ASSERT_EQ(dir_ptr->Unlink("b", true), ZX_OK);
  child_set.erase("b");
  ASSERT_EQ(dir_ptr->Unlink("d", true), ZX_OK);
  child_set.erase("d");
  FileTester::CheckChildrenFromReaddir(dir_ptr, child_set);

  // create "f" and "g"
  FileTester::CreateChild(dir_ptr, S_IFDIR, "f");
  child_set.insert("f");
  FileTester::CreateChild(dir_ptr, S_IFDIR, "g");
  child_set.insert("g");
  FileTester::CheckChildrenFromReaddir(dir_ptr, child_set);

  // rename "g" to "h"
  ASSERT_EQ(dir_ptr->Rename(inline_child_dir, "g", "h", true, true), ZX_OK);
  child_set.erase("g");
  child_set.insert("h");
  FileTester::CheckChildrenFromReaddir(dir_ptr, child_set);

  // fill all inline dentry slots
  auto child_count = child_set.size();
  for (; child_count < inline_child_dir->MaxInlineDentry() - 2; ++child_count) {
    FileTester::CreateChild(dir_ptr, S_IFDIR, std::to_string(child_count));
    child_set.insert(std::to_string(child_count));
  }
  FileTester::CheckChildrenFromReaddir(dir_ptr, child_set);

  // It should be inline
  FileTester::CheckInlineDir(dir_ptr);

  // one more entry
  FileTester::CreateChild(dir_ptr, S_IFDIR, std::to_string(child_count));
  child_set.insert(std::to_string(child_count));
  FileTester::CheckChildrenFromReaddir(dir_ptr, child_set);

  // It should be non inline
  FileTester::CheckNonInlineDir(dir_ptr);

  ASSERT_EQ(inline_child_dir->Close(), ZX_OK);
  inline_child_dir = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;
  FileTester::Unmount(std::move(fs), &bc);

  // Check dentry after remount
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  FileTester::CreateRoot(fs.get(), &root);
  root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  fbl::RefPtr<fs::Vnode> child;
  FileTester::Lookup(root_dir.get(), inline_dir_name, &child);
  inline_child_dir = fbl::RefPtr<Dir>::Downcast(std::move(child));
  dir_ptr = inline_child_dir.get();

  FileTester::CheckNonInlineDir(dir_ptr);
  FileTester::CheckChildrenFromReaddir(dir_ptr, child_set);

  ASSERT_EQ(inline_child_dir->Close(), ZX_OK);
  inline_child_dir = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);
  EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}, &bc), ZX_OK);
}

TEST(InlineDirTest, NestedInlineDirectories) {
  // There was a reported malfunction of inline-directories when the volume size is small.
  // This test evaluates such case.
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);
  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  zx::result vnode = root_dir->Create("alpha", fs::CreationType::kDirectory);
  ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
  fbl::RefPtr<Dir> parent_dir = fbl::RefPtr<Dir>::Downcast(*std::move(vnode));

  vnode = parent_dir->Create("bravo", fs::CreationType::kDirectory);
  ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
  fbl::RefPtr<Dir> child_dir = fbl::RefPtr<Dir>::Downcast(*std::move(vnode));

  vnode = child_dir->Create("charlie", fs::CreationType::kFile);
  ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();

  fbl::RefPtr<File> child_file = fbl::RefPtr<File>::Downcast(*std::move(vnode));

  char data[] = "Hello, world!";
  FileTester::AppendToFile(child_file.get(), data, sizeof(data));

  ASSERT_EQ(child_file->Close(), ZX_OK);
  ASSERT_EQ(child_dir->Close(), ZX_OK);
  ASSERT_EQ(parent_dir->Close(), ZX_OK);
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = parent_dir = child_dir = nullptr;
  child_file = nullptr;

  FileTester::Unmount(std::move(fs), &bc);
  EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}), ZX_OK);
}

TEST(InlineDirTest, InlineDirPino) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  MountOptions options{};

  // Enable inline dir option
  ASSERT_EQ(options.SetValue(MountOption::kInlineDentry, 1), ZX_OK);
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);

  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  // Inline dir creation
  zx::result vnode = root_dir->Create("a", fs::CreationType::kDirectory);
  ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
  fbl::RefPtr<Dir> a_dir = fbl::RefPtr<Dir>::Downcast(*std::move(vnode));
  ASSERT_EQ(a_dir->GetParentNid(), root_dir->Ino());

  vnode = root_dir->Create("b", fs::CreationType::kDirectory);
  ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
  fbl::RefPtr<Dir> b_dir = fbl::RefPtr<Dir>::Downcast(*std::move(vnode));
  ASSERT_EQ(b_dir->GetParentNid(), root_dir->Ino());

  vnode = a_dir->Create("c", fs::CreationType::kDirectory);
  ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
  fbl::RefPtr<Dir> c_dir = fbl::RefPtr<Dir>::Downcast(*std::move(vnode));
  ASSERT_EQ(c_dir->GetParentNid(), a_dir->Ino());

  vnode = a_dir->Create("d", fs::CreationType::kFile);
  ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
  fbl::RefPtr<File> d1_file = fbl::RefPtr<File>::Downcast(*std::move(vnode));
  ASSERT_EQ(d1_file->GetParentNid(), a_dir->Ino());

  vnode = b_dir->Create("d", fs::CreationType::kFile);
  ASSERT_TRUE(vnode.is_ok()) << vnode.status_string();
  fbl::RefPtr<File> d2_file = fbl::RefPtr<File>::Downcast(*std::move(vnode));
  ASSERT_EQ(d2_file->GetParentNid(), b_dir->Ino());

  // rename "/a/c" to "/b/c" and "/a/d" to "/b/d"
  ASSERT_EQ(a_dir->Rename(b_dir, "c", "c", true, true), ZX_OK);
  ASSERT_EQ(a_dir->Rename(b_dir, "d", "d", false, false), ZX_OK);

  // Check i_pino of renamed directory
  ASSERT_EQ(c_dir->GetParentNid(), b_dir->Ino());
  ASSERT_EQ(d1_file->GetParentNid(), b_dir->Ino());

  ASSERT_EQ(d1_file->Close(), ZX_OK);
  ASSERT_EQ(d2_file->Close(), ZX_OK);
  ASSERT_EQ(c_dir->Close(), ZX_OK);
  ASSERT_EQ(b_dir->Close(), ZX_OK);
  ASSERT_EQ(a_dir->Close(), ZX_OK);
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = a_dir = b_dir = c_dir = nullptr;
  d1_file = d2_file = nullptr;

  // Remount
  FileTester::Unmount(std::move(fs), &bc);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  FileTester::CreateRoot(fs.get(), &root);
  root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  fbl::RefPtr<fs::Vnode> lookup_vn;
  FileTester::Lookup(root_dir.get(), "b", &lookup_vn);
  b_dir = fbl::RefPtr<Dir>::Downcast(std::move(lookup_vn));
  FileTester::Lookup(b_dir.get(), "c", &lookup_vn);
  c_dir = fbl::RefPtr<Dir>::Downcast(std::move(lookup_vn));
  FileTester::Lookup(b_dir.get(), "d", &lookup_vn);
  d1_file = fbl::RefPtr<File>::Downcast(std::move(lookup_vn));

  // Check i_pino of renamed directory
  ASSERT_EQ(c_dir->GetParentNid(), b_dir->Ino());
  ASSERT_EQ(d1_file->GetParentNid(), b_dir->Ino());

  ASSERT_EQ(d1_file->Close(), ZX_OK);
  ASSERT_EQ(c_dir->Close(), ZX_OK);
  ASSERT_EQ(b_dir->Close(), ZX_OK);
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = b_dir = c_dir = nullptr;
  d1_file = nullptr;

  FileTester::Unmount(std::move(fs), &bc);
  EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}, &bc), ZX_OK);
}

TEST(InlineDataTest, InlineRegFileTruncate) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);

  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  // Inline file creation
  std::string inline_file_name("inline");
  zx::result inline_child = root_dir->Create(inline_file_name, fs::CreationType::kFile);
  ASSERT_TRUE(inline_child.is_ok()) << inline_child.status_string();

  fbl::RefPtr<VnodeF2fs> inline_child_file =
      fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(inline_child));

  inline_child_file->SetFlag(InodeInfoFlag::kInlineData);
  FileTester::CheckInlineFile(inline_child_file.get());

  // Write until entire inline data space is written
  File *inline_child_file_ptr = static_cast<File *>(inline_child_file.get());
  size_t target_size = inline_child_file_ptr->MaxInlineData() - 1;

  char w_buf[kBlockSize];
  char r_buf[kBlockSize];

  for (size_t i = 0; i < kBlockSize; ++i) {
    w_buf[i] = static_cast<char>(rand());
  }

  FileTester::AppendToInline(inline_child_file_ptr, w_buf, target_size);
  FileTester::CheckInlineFile(inline_child_file.get());
  ASSERT_EQ(inline_child_file_ptr->GetSize(), target_size);

  // Truncate to reduced size, then verify
  target_size = inline_child_file_ptr->MaxInlineData() / 2;
  ASSERT_EQ(inline_child_file_ptr->Truncate(target_size), ZX_OK);
  FileTester::CheckInlineFile(inline_child_file.get());
  ASSERT_EQ(inline_child_file_ptr->GetSize(), target_size);

  // Truncate to original size, then verify
  target_size = inline_child_file_ptr->MaxInlineData() - 1;

  for (size_t i = inline_child_file_ptr->MaxInlineData() / 2; i < kBlockSize; ++i) {
    w_buf[i] = 0;
  }

  ASSERT_EQ(inline_child_file_ptr->Truncate(target_size), ZX_OK);
  FileTester::CheckInlineFile(inline_child_file.get());
  ASSERT_EQ(inline_child_file_ptr->GetSize(), target_size);

  // Truncate to more than inline data size, then verify
  target_size = kBlockSize;

  ASSERT_EQ(inline_child_file_ptr->Truncate(kBlockSize), ZX_OK);
  FileTester::CheckNonInlineFile(inline_child_file.get());
  ASSERT_EQ(inline_child_file_ptr->GetSize(), target_size);

  FileTester::ReadFromFile(inline_child_file_ptr, r_buf, target_size, 0);
  ASSERT_EQ(memcmp(r_buf, w_buf, target_size), 0);

  inline_child_file_ptr = nullptr;
  ASSERT_EQ(inline_child_file->Close(), ZX_OK);
  inline_child_file = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);

  // Remount and verify
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  FileTester::CreateRoot(fs.get(), &root);
  root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  fbl::RefPtr<fs::Vnode> child;
  FileTester::Lookup(root_dir.get(), inline_file_name, &child);
  inline_child_file = fbl::RefPtr<VnodeF2fs>::Downcast(std::move(child));
  FileTester::CheckNonInlineFile(inline_child_file.get());

  inline_child_file_ptr = static_cast<File *>(inline_child_file.get());
  ASSERT_EQ(inline_child_file_ptr->GetSize(), target_size);

  FileTester::ReadFromFile(inline_child_file_ptr, r_buf, target_size, 0);
  ASSERT_EQ(memcmp(r_buf, w_buf, target_size), 0);

  inline_child_file_ptr = nullptr;
  ASSERT_EQ(inline_child_file->Close(), ZX_OK);
  inline_child_file = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);
  EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}, &bc), ZX_OK);
}

TEST(InlineDataTest, DataExistFlag) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  // Enable inline data option
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);

  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  // Inline file creation, then check if kDataExist flag is unset
  std::string inline_file_name("inline");
  zx::result inline_child = root_dir->Create(inline_file_name, fs::CreationType::kFile);
  ASSERT_TRUE(inline_child.is_ok()) << inline_child.status_string();

  fbl::RefPtr<VnodeF2fs> inline_child_file =
      fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(inline_child));

  inline_child_file->SetFlag(InodeInfoFlag::kInlineData);
  FileTester::CheckInlineFile(inline_child_file.get());
  FileTester::CheckDataExistFlagUnset(inline_child_file.get());

  // Write some data, then check if kDataExist flag is set
  File *inline_child_file_ptr = static_cast<File *>(inline_child_file.get());
  constexpr std::string_view data_string = "hello";

  FileTester::AppendToInline(inline_child_file_ptr, data_string.data(), data_string.size());
  FileTester::CheckInlineFile(inline_child_file.get());
  ASSERT_EQ(inline_child_file_ptr->GetSize(), data_string.size());
  FileTester::CheckDataExistFlagSet(inline_child_file.get());

  // Truncate to non-zero size, then check if kDataExist flag is set
  ASSERT_EQ(inline_child_file_ptr->Truncate(data_string.size() / 2), ZX_OK);
  FileTester::CheckInlineFile(inline_child_file.get());
  ASSERT_EQ(inline_child_file_ptr->GetSize(), data_string.size() / 2);
  FileTester::CheckDataExistFlagSet(inline_child_file.get());

  // Truncate to zero size, then check if kDataExist flag is unset
  ASSERT_EQ(inline_child_file_ptr->Truncate(0), ZX_OK);
  FileTester::CheckInlineFile(inline_child_file.get());
  ASSERT_EQ(inline_child_file_ptr->GetSize(), 0UL);
  FileTester::CheckDataExistFlagUnset(inline_child_file.get());

  // Write data again, then check if kDataExist flag is set
  FileTester::AppendToInline(inline_child_file_ptr, data_string.data(), data_string.size());
  FileTester::CheckInlineFile(inline_child_file.get());
  ASSERT_EQ(inline_child_file_ptr->GetSize(), data_string.size());
  FileTester::CheckDataExistFlagSet(inline_child_file.get());

  inline_child_file_ptr = nullptr;
  ASSERT_EQ(inline_child_file->Close(), ZX_OK);
  inline_child_file = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);

  // Remount and check if KDataExist flag is still set
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  FileTester::CreateRoot(fs.get(), &root);
  root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  fbl::RefPtr<fs::Vnode> child;
  FileTester::Lookup(root_dir.get(), inline_file_name, &child);
  inline_child_file = fbl::RefPtr<VnodeF2fs>::Downcast(std::move(child));
  FileTester::CheckInlineFile(inline_child_file.get());
  FileTester::CheckDataExistFlagSet(inline_child_file.get());

  ASSERT_EQ(inline_child_file->Close(), ZX_OK);
  inline_child_file = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);
  EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}, &bc), ZX_OK);
}

TEST(InlineXattrTest, InlineXattr) {
  std::unique_ptr<BcacheMapper> bc;
  FileTester::MkfsOnFakeDev(&bc);

  std::unique_ptr<F2fs> fs;
  MountOptions options{};
  // Enable inline xattr option
  ASSERT_EQ(options.SetValue(MountOption::kInlineXattr, 1), ZX_OK);
  async::Loop loop(&kAsyncLoopConfigAttachToCurrentThread);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  fbl::RefPtr<VnodeF2fs> root;
  FileTester::CreateRoot(fs.get(), &root);

  fbl::RefPtr<Dir> root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));

  // Inline xattr file creation
  std::string test_filename("inline");
  zx::result test_child = root_dir->Create(test_filename, fs::CreationType::kFile);
  ASSERT_TRUE(test_child.is_ok()) << test_child.status_string();

  fbl::RefPtr<VnodeF2fs> test_file = fbl::RefPtr<VnodeF2fs>::Downcast(*std::move(test_child));

  File *test_file_ptr = static_cast<File *>(test_file.get());

  FileTester::CheckInlineXattr(test_file_ptr);

  // Set xattr
  constexpr uint8_t kNameLen = 4;
  constexpr uint16_t kValueSize = 32;
  std::vector<std::pair<std::string, std::array<uint8_t, kValueSize>>> xattrs;

  XattrEntryInfo sample_info{.name_index = 0, .name_len = kNameLen, .value_size = kValueSize};

  // Set xattr within inline xattr area, until just before xattr block is allocated
  uint8_t inline_xattr_count = (kInlineXattrAddrs - kXattrHeaderSlots) / sample_info.Slots();
  for (uint8_t i = 0; i < inline_xattr_count; ++i) {
    std::string name;
    for (uint8_t j = 0; j < kNameLen; ++j) {
      name.push_back(static_cast<std::string::value_type>('a' + i));
    }

    std::array<uint8_t, kValueSize> value;
    value.fill(i + 1);

    ASSERT_EQ(
        test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kNone),
        ZX_OK);

    xattrs.emplace_back(name, value);
  }

  // Check if xattr block is not yet allocated
  ASSERT_EQ(test_file_ptr->XattrNid(), 0U);

  // Add one more xattr, then xattr block is allocated
  std::string name = "last";
  std::array<uint8_t, kValueSize> value;
  value.fill(1);
  ASSERT_EQ(test_file_ptr->SetExtendedAttribute(XattrIndex::kUser, name, value, XattrOption::kNone),
            ZX_OK);

  xattrs.emplace_back(name, value);
  ASSERT_NE(test_file_ptr->XattrNid(), 0U);

  // Get and verify
  std::array<uint8_t, kMaxXattrValueLength> buf;
  for (auto &xattr : xattrs) {
    buf.fill(0);
    zx::result<size_t> result =
        test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, xattr.first, buf);
    ASSERT_TRUE(result.is_ok());
    ASSERT_EQ(*result, xattr.second.size());
    ASSERT_EQ(std::memcmp(buf.data(), xattr.second.data(), *result), 0);
  }

  // Remount
  ASSERT_EQ(test_file->Close(), ZX_OK);
  test_file = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);
  FileTester::MountWithOptions(loop.dispatcher(), options, &bc, &fs);

  FileTester::CreateRoot(fs.get(), &root);
  root_dir = fbl::RefPtr<Dir>::Downcast(std::move(root));
  fbl::RefPtr<fs::Vnode> test_vn;
  FileTester::Lookup(root_dir.get(), test_filename, &test_vn);
  test_file = fbl::RefPtr<VnodeF2fs>::Downcast(std::move(test_vn));
  test_file_ptr = static_cast<File *>(test_file.get());

  FileTester::CheckInlineXattr(test_file_ptr);

  // Get and verify
  for (auto &xattr : xattrs) {
    buf.fill(0);
    zx::result<size_t> result =
        test_file_ptr->GetExtendedAttribute(XattrIndex::kUser, xattr.first, buf);
    ASSERT_TRUE(result.is_ok());
    ASSERT_EQ(*result, xattr.second.size());
    ASSERT_EQ(std::memcmp(buf.data(), xattr.second.data(), *result), 0);
  }

  ASSERT_EQ(test_file->Close(), ZX_OK);
  test_file = nullptr;
  ASSERT_EQ(root_dir->Close(), ZX_OK);
  root_dir = nullptr;

  FileTester::Unmount(std::move(fs), &bc);
  EXPECT_EQ(Fsck(std::move(bc), FsckOptions{.repair = false}, &bc), ZX_OK);
}

}  // namespace
}  // namespace f2fs
