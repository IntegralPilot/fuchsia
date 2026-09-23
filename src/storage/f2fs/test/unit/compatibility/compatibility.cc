// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>

#include <iomanip>
#include <sstream>

#include <gtest/gtest.h>

#include "src/storage/f2fs/f2fs.h"
#include "src/storage/f2fs/test/unit/unit_lib.h"
#include "src/storage/f2fs/vnode.h"

namespace f2fs {
namespace {

class DirectoryTest : public F2fsFakeDevTestFixture {
 public:
  DirectoryTest()
      : F2fsFakeDevTestFixture(TestOptions{.block_count = kSectorCount100MiB,
                                           .image_file = "directory_test.img.zst"}) {}
};

TEST_F(DirectoryTest, Directory) {
  fbl::RefPtr<fs::Vnode> vnode;
  FileTester::Lookup(root_dir_.get(), "depth", &vnode);
  auto test_dir = fbl::RefPtr<Dir>::Downcast(std::move(vnode));

  constexpr int kDirDepth = 60;

  // From "depth", recursively find directories from "0" to "59"
  for (int depth = 0; depth < kDirDepth; ++depth) {
    FileTester::Lookup(test_dir.get(), std::to_string(depth), &vnode);
    test_dir->Close();
    test_dir = fbl::RefPtr<Dir>::Downcast(std::move(vnode));
  }

  // Directory "60" should not be exist
  ASSERT_EQ(test_dir->Lookup(std::to_string(kDirDepth), &vnode), ZX_ERR_NOT_FOUND);

  test_dir->Close();

  // "width" should contain "0", "1", ..., "19", "120", "121", ..., "139"
  FileTester::Lookup(root_dir_.get(), "width", &vnode);
  test_dir = fbl::RefPtr<Dir>::Downcast(std::move(vnode));

  for (int i = 0; i <= 19; ++i) {
    fbl::RefPtr<fs::Vnode> child_vnode;
    FileTester::Lookup(test_dir.get(), std::to_string(i), &child_vnode);
    child_vnode->Close();
  }
  for (int i = 120; i <= 139; ++i) {
    fbl::RefPtr<fs::Vnode> child_vnode;
    FileTester::Lookup(test_dir.get(), std::to_string(i), &child_vnode);
    child_vnode->Close();
  }

  // "width" should not contain "20", "21", ..., "59"
  for (int i = 20; i <= 59; ++i) {
    fbl::RefPtr<fs::Vnode> child_vnode;
    ASSERT_EQ(test_dir->Lookup(std::to_string(i), &vnode), ZX_ERR_NOT_FOUND);
  }

  test_dir->Close();
}

class FileTest : public F2fsFakeDevTestFixture {
 public:
  FileTest()
      : F2fsFakeDevTestFixture(
            TestOptions{.block_count = kSectorCount100MiB, .image_file = "file_test.img.zst"}) {}
};

TEST_F(FileTest, File) {
  fbl::RefPtr<fs::Vnode> vnode;

  // File "file_write" has 64KB of data.
  FileTester::Lookup(root_dir_.get(), "file_write", &vnode);
  auto test_file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  ASSERT_EQ(test_file->GetSize(), static_cast<size_t>(64 * 1024));

  auto r_buf = std::make_unique<char[]>(kBlockSize);
  auto verify = std::make_unique<char[]>(kBlockSize);
  for (uint32_t i = 0; i < 64 * 1024 / kBlockSize; ++i) {
    memset(verify.get(), static_cast<char>(i % 256), kBlockSize);
    FileTester::ReadFromFile(test_file.get(), r_buf.get(), kBlockSize, kBlockSize * i);
    ASSERT_EQ(memcmp(r_buf.get(), verify.get(), kBlockSize), 0);
  }

  test_file->Close();

  // File "file_truncate" has zero-filled 16KB data.
  FileTester::Lookup(root_dir_.get(), "file_truncate", &vnode);
  test_file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  ASSERT_EQ(test_file->GetSize(), static_cast<size_t>(16 * 1024));

  memset(verify.get(), 0, kBlockSize);
  for (uint32_t i = 0; i < 16 * 1024 / kBlockSize; ++i) {
    FileTester::ReadFromFile(test_file.get(), r_buf.get(), kBlockSize, kBlockSize * i);
    ASSERT_EQ(memcmp(r_buf.get(), verify.get(), kBlockSize), 0);
  }

  test_file->Close();

  // File "file_truncate_shrink" has 16KB of data.
  FileTester::Lookup(root_dir_.get(), "file_truncate_shrink", &vnode);
  test_file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  ASSERT_EQ(test_file->GetSize(), static_cast<size_t>(16 * 1024));

  for (uint32_t i = 0; i < 16 * 1024 / kBlockSize; ++i) {
    memset(verify.get(), static_cast<char>(i % 256), kBlockSize);
    FileTester::ReadFromFile(test_file.get(), r_buf.get(), kBlockSize, kBlockSize * i);
    ASSERT_EQ(memcmp(r_buf.get(), verify.get(), kBlockSize), 0);
  }

  test_file->Close();

  // File "file_exceed" has 7KB of data.
  FileTester::Lookup(root_dir_.get(), "file_exceed", &vnode);
  test_file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  ASSERT_EQ(test_file->GetSize(), static_cast<size_t>(7 * 1024));

  memset(verify.get(), 0, kBlockSize);
  memset(verify.get(), 1, 2 * 1024);
  {
    size_t actual;
    // Read 4KB from 5KB offset, then only 2KB should be read
    ASSERT_EQ(
        FileTester::Read(test_file.get(), r_buf.get(), kBlockSize, kBlockSize + 1024, &actual),
        ZX_OK);
    ASSERT_EQ(actual, static_cast<size_t>(2 * 1024));
  }
  ASSERT_EQ(memcmp(r_buf.get(), verify.get(), 2 * 1024), 0);

  test_file->Close();

  // File "file_rename" should not be exist while file "renamed_file" is exist
  {
    fbl::RefPtr<fs::Vnode> vn = nullptr;
    ASSERT_EQ(root_dir_->Lookup("file_rename", &vn), ZX_ERR_NOT_FOUND);
  }
  FileTester::Lookup(root_dir_.get(), "renamed_file", &vnode);
  vnode->Close();

  // File "file_fallocate" has zero-filled 64KB data.
  FileTester::Lookup(root_dir_.get(), "file_fallocate", &vnode);
  test_file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  ASSERT_EQ(test_file->GetSize(), static_cast<size_t>(64 * 1024));

  memset(verify.get(), 0, kBlockSize);
  for (uint32_t i = 0; i < 64 * 1024 / kBlockSize; ++i) {
    FileTester::ReadFromFile(test_file.get(), r_buf.get(), kBlockSize, kBlockSize * i);
    ASSERT_EQ(memcmp(r_buf.get(), verify.get(), kBlockSize), 0);
  }

  test_file->Close();

  // File "file_fallocate_hole" has 64KB of data, with zero-filled hole from offset 8KB to 16KB.
  FileTester::Lookup(root_dir_.get(), "file_fallocate_hole", &vnode);
  test_file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  ASSERT_EQ(test_file->GetSize(), static_cast<size_t>(64 * 1024));

  for (uint32_t i = 0; i < 64 * 1024 / kBlockSize; ++i) {
    if (i == 2 || i == 3) {  // zero fill for punch hole range
      memset(verify.get(), 0, kBlockSize);
    } else {
      memset(verify.get(), static_cast<char>(i % 256), kBlockSize);
    }
    FileTester::ReadFromFile(test_file.get(), r_buf.get(), kBlockSize, kBlockSize * i);
    ASSERT_EQ(memcmp(r_buf.get(), verify.get(), kBlockSize), 0);
  }

  test_file->Close();

  // Files "filemode_xxx" have filemode from 000 to 777
  for (uint32_t i = 0; i <= 0777; ++i) {
    std::ostringstream oss;
    oss << std::setfill('0') << std::setw(3) << std::oct << i;
    std::string filename = "filemode_" + oss.str();

    FileTester::Lookup(root_dir_.get(), filename, &vnode);
    test_file = fbl::RefPtr<File>::Downcast(std::move(vnode));

    zx::result<fs::VnodeAttributes> result = test_file->GetAttributes();
    ASSERT_TRUE(result.is_ok());
    ASSERT_TRUE(result->mode.has_value());
    ASSERT_EQ(*result->mode & 0777, i);

    test_file->Close();
  }
}

class InlineTest : public F2fsFakeDevTestFixture {
 public:
  InlineTest()
      : F2fsFakeDevTestFixture(
            TestOptions{.block_count = kSectorCount100MiB, .image_file = "inline_test.img.zst"}) {}
};

TEST_F(InlineTest, Inline) {
  fbl::RefPtr<fs::Vnode> vnode;

  // Check both |InlineFile| and |DataExist| flags are set
  FileTester::Lookup(root_dir_.get(), "inline_file", &vnode);
  auto test_file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  FileTester::CheckInlineFile(test_file.get());
  FileTester::CheckDataExistFlagSet(test_file.get());
  test_file->Close();

  // For an empty inline file, check |InlineFile| flag is set and |DataExist| flag is unset
  FileTester::Lookup(root_dir_.get(), "inline_file_empty", &vnode);
  test_file = fbl::RefPtr<File>::Downcast(std::move(vnode));
  FileTester::CheckInlineFile(test_file.get());
  FileTester::CheckDataExistFlagUnset(test_file.get());
  test_file->Close();

  // Check if children of inline directory are available
  FileTester::Lookup(root_dir_.get(), "inline_dir", &vnode);
  auto test_dir = fbl::RefPtr<Dir>::Downcast(std::move(vnode));
  FileTester::CheckInlineDir(test_dir.get());
  std::unordered_set<std::string> children_set = {"a", "b", "c"};
  FileTester::CheckChildrenFromReaddir(test_dir.get(), children_set);
  test_dir->Close();
}

}  // namespace
}  // namespace f2fs
