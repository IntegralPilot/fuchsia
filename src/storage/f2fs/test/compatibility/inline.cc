// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/test/compatibility/compatibility.h"

namespace f2fs {
namespace {

using InlineCompatibilityTest = F2fsGuestTest;

TEST_F(InlineCompatibilityTest, InlineDentryLinuxToFuchsia) {
  // Inline dentry on Linux F2FS is available from v3.17
  GetEnclosedGuest().GetLinuxOperator().CheckLinuxVersion(3, 17);

  const std::string inline_dir_path = "/inline";
  const std::string noninline_dir_path = "/noninline";

  const uint32_t max_inline_dentry = GetEnclosedGuest().GetFuchsiaOperator().MaxInlineDentrySlots();

  const uint32_t nr_child_of_inline_dir = max_inline_dentry / 2;
  const uint32_t nr_child_of_noninline_dir = max_inline_dentry * 2;

  // Create child on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    GetEnclosedGuest().GetLinuxOperator().Mkdir(std::string(kLinuxPathPrefix) + inline_dir_path,
                                                0755);

    for (uint32_t i = 0; i < nr_child_of_inline_dir; ++i) {
      std::string child_name(std::string(kLinuxPathPrefix) + inline_dir_path);
      child_name.append("/").append(std::to_string(i));
      GetEnclosedGuest().GetLinuxOperator().Mkdir(child_name, 0755);
    }

    GetEnclosedGuest().GetLinuxOperator().Mkdir(std::string(kLinuxPathPrefix) + noninline_dir_path,
                                                0755);

    for (uint32_t i = 0; i < nr_child_of_noninline_dir; ++i) {
      std::string child_name(std::string(kLinuxPathPrefix) + noninline_dir_path);
      child_name.append("/").append(std::to_string(i));
      GetEnclosedGuest().GetLinuxOperator().Mkdir(child_name, 0755);
    }
  }

  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    // Check if inline directory is still inline on Fuchsia
    auto inline_dir = GetEnclosedGuest().GetFuchsiaOperator().Open(inline_dir_path, O_RDWR, 0644);
    ASSERT_TRUE(inline_dir->IsValid());

    VnodeF2fs *raw_vn_ptr = static_cast<FuchsiaTestFile *>(inline_dir.get())->GetRawVnodePtr();
    ASSERT_TRUE(raw_vn_ptr->TestFlag(InodeInfoFlag::kInlineDentry));

    // Check if children of inline directory are accessible
    for (uint32_t i = 0; i < nr_child_of_inline_dir; ++i) {
      std::string child_name(inline_dir_path);
      child_name.append("/").append(std::to_string(i));
      auto child = GetEnclosedGuest().GetFuchsiaOperator().Open(child_name, O_RDWR, 0644);
      ASSERT_TRUE(child->IsValid());
    }

    // Create one more child, and the directory should remain inline
    std::string additional_child(inline_dir_path);
    additional_child.append("/").append(std::to_string(nr_child_of_inline_dir));
    GetEnclosedGuest().GetFuchsiaOperator().Mkdir(additional_child, 0755);
    ASSERT_TRUE(raw_vn_ptr->TestFlag(InodeInfoFlag::kInlineDentry));

    // Check if noninline directory is still noninline on Fuchsia
    auto noninline_dir =
        GetEnclosedGuest().GetFuchsiaOperator().Open(noninline_dir_path, O_RDWR, 0644);
    ASSERT_TRUE(inline_dir->IsValid());

    raw_vn_ptr = static_cast<FuchsiaTestFile *>(noninline_dir.get())->GetRawVnodePtr();
    ASSERT_FALSE(raw_vn_ptr->TestFlag(InodeInfoFlag::kInlineDentry));

    // Check if children of noninline directory are accessible
    for (uint32_t i = 0; i < nr_child_of_noninline_dir; ++i) {
      std::string child_name(noninline_dir_path);
      child_name.append("/").append(std::to_string(i));
      auto child = GetEnclosedGuest().GetFuchsiaOperator().Open(child_name, O_RDWR, 0644);
      ASSERT_TRUE(child->IsValid());
    }
  }

  // Check new child exists on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    std::string child_name(std::string(kLinuxPathPrefix) + inline_dir_path);
    child_name.append("/").append(std::to_string(nr_child_of_inline_dir));

    auto new_child = GetEnclosedGuest().GetLinuxOperator().Open(child_name, O_RDWR, 0644);
    ASSERT_TRUE(new_child->IsValid());
  }
}

TEST_F(InlineCompatibilityTest, InlineDentryFuchsiaToLinux) {
  // Inline dentry on Linux F2FS is available from v3.17
  GetEnclosedGuest().GetLinuxOperator().CheckLinuxVersion(3, 17);

  const std::string inline_dir_path = "/inline";
  const std::string noninline_dir_path = "/noninline";

  const uint32_t max_inline_dentry = GetEnclosedGuest().GetFuchsiaOperator().MaxInlineDentrySlots();

  const uint32_t nr_child_of_inline_dir = max_inline_dentry / 2;
  const uint32_t nr_child_of_noninline_dir = max_inline_dentry * 2;

  // Create child on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Mkfs();
    MountOptions options;
    ASSERT_EQ(options.SetValue(MountOption::kInlineDentry, 1), ZX_OK);
    GetEnclosedGuest().GetFuchsiaOperator().Mount(options);

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    // Create inline directory
    GetEnclosedGuest().GetFuchsiaOperator().Mkdir(inline_dir_path, 0755);
    auto inline_dir = GetEnclosedGuest().GetFuchsiaOperator().Open(inline_dir_path, O_RDWR, 0644);
    ASSERT_TRUE(inline_dir->IsValid());

    VnodeF2fs *raw_inline_vn_ptr =
        static_cast<FuchsiaTestFile *>(inline_dir.get())->GetRawVnodePtr();
    ASSERT_TRUE(raw_inline_vn_ptr->TestFlag(InodeInfoFlag::kInlineDentry));

    // Create children up to |nr_child_of_inline_dir|, and the directory should be inline
    for (uint32_t i = 0; i < nr_child_of_inline_dir; ++i) {
      std::string child_name(inline_dir_path);
      child_name.append("/").append(std::to_string(i));
      GetEnclosedGuest().GetFuchsiaOperator().Mkdir(child_name, 0755);
    }
    ASSERT_TRUE(raw_inline_vn_ptr->TestFlag(InodeInfoFlag::kInlineDentry));

    // Create inline directory
    GetEnclosedGuest().GetFuchsiaOperator().Mkdir(noninline_dir_path, 0755);
    auto noninline_dir =
        GetEnclosedGuest().GetFuchsiaOperator().Open(noninline_dir_path, O_RDWR, 0644);
    ASSERT_TRUE(noninline_dir->IsValid());

    auto *raw_noninline_vn_ptr =
        static_cast<FuchsiaTestFile *>(noninline_dir.get())->GetRawVnodePtr();
    ASSERT_TRUE(raw_noninline_vn_ptr->TestFlag(InodeInfoFlag::kInlineDentry));

    // Create children up to |nr_child_of_noninline_dir|, and the directory should be non inline
    for (uint32_t i = 0; i < nr_child_of_noninline_dir; ++i) {
      std::string child_name(noninline_dir_path);
      child_name.append("/").append(std::to_string(i));
      GetEnclosedGuest().GetFuchsiaOperator().Mkdir(child_name, 0755);
    }
    ASSERT_FALSE(raw_noninline_vn_ptr->TestFlag(InodeInfoFlag::kInlineDentry));
  }

  // Check children exist on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    for (uint32_t i = 0; i < nr_child_of_inline_dir; ++i) {
      std::string child_name(std::string(kLinuxPathPrefix) + inline_dir_path);
      child_name.append("/").append(std::to_string(i));
      auto child = GetEnclosedGuest().GetLinuxOperator().Open(child_name, O_RDWR, 0644);
      ASSERT_TRUE(child->IsValid());
    }

    for (uint32_t i = 0; i < nr_child_of_noninline_dir; ++i) {
      std::string child_name(std::string(kLinuxPathPrefix) + noninline_dir_path);
      child_name.append("/").append(std::to_string(i));
      auto child = GetEnclosedGuest().GetLinuxOperator().Open(child_name, O_RDWR, 0644);
      ASSERT_TRUE(child->IsValid());
    }
  }
}

TEST_F(InlineCompatibilityTest, InlineDataLinuxToFuchsia) {
  // Inline data on Linux F2FS is available from v3.13
  GetEnclosedGuest().GetLinuxOperator().CheckLinuxVersion(3, 13);

  const std::string inline_file_name = "inline";

  const uint32_t data_length = GetEnclosedGuest().GetFuchsiaOperator().MaxInlineDataLength() / 2;

  uint32_t r_buf[kBlockSize / sizeof(uint32_t)];
  uint32_t w_buf[kBlockSize / sizeof(uint32_t)];
  for (uint32_t i = 0; i < kBlockSize / sizeof(uint32_t); ++i) {
    w_buf[i] = CpuToLe(i);
  }

  // Create and write inline file on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + inline_file_name, O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    ASSERT_EQ(test_file->Write(w_buf, data_length), data_length);
  }

  // Verify on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    // Check if inline file is still inline on Fuchsia
    auto test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(inline_file_name, O_RDWR, 0644);
    ASSERT_TRUE(test_file->IsValid());

    VnodeF2fs *raw_vn_ptr = static_cast<FuchsiaTestFile *>(test_file.get())->GetRawVnodePtr();
    ASSERT_TRUE(raw_vn_ptr->TestFlag(InodeInfoFlag::kInlineData));

    // Read verify
    ASSERT_EQ(test_file->Read(r_buf, data_length), data_length);
    ASSERT_EQ(memcmp(r_buf, w_buf, data_length), 0);
  }
}

TEST_F(InlineCompatibilityTest, DataExistFlagLinuxToFuchsia) {
  // |DataExist| flag on Linux F2FS is available from v3.18
  GetEnclosedGuest().GetLinuxOperator().CheckLinuxVersion(3, 18);

  const std::string filenames[4] = {"alpha", "bravo", "charlie", "delta"};
  const std::string test_string = "hello";

  // Create and write inline files on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    // Create file
    auto test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filenames[0], O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    // Write some data
    test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filenames[1], O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    ASSERT_EQ(test_file->Write(test_string.data(), test_string.size()),
              static_cast<ssize_t>(test_string.size()));

    // Truncate to non-zero size
    test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filenames[2], O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    ASSERT_EQ(test_file->Write(test_string.data(), test_string.size()),
              static_cast<ssize_t>(test_string.size()));
    ASSERT_EQ(test_file->Ftruncate(test_string.size() / 2), 0);

    // Truncate to zero size
    test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filenames[3], O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    ASSERT_EQ(test_file->Write(test_string.data(), test_string.size()),
              static_cast<ssize_t>(test_string.size()));
    ASSERT_EQ(test_file->Ftruncate(0), 0);
  }

  // Check if all files have correct flags on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    // Only created, kDataExist should be unset
    auto test_file =
        GetEnclosedGuest().GetFuchsiaOperator().Open(filenames[0], O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    VnodeF2fs *raw_vn_ptr = static_cast<FuchsiaTestFile *>(test_file.get())->GetRawVnodePtr();
    ASSERT_TRUE(raw_vn_ptr->TestFlag(InodeInfoFlag::kInlineData));
    ASSERT_FALSE(raw_vn_ptr->TestFlag(InodeInfoFlag::kDataExist));

    // Data written, kDataExist should be set
    test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filenames[1], O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    raw_vn_ptr = static_cast<FuchsiaTestFile *>(test_file.get())->GetRawVnodePtr();
    ASSERT_TRUE(raw_vn_ptr->TestFlag(InodeInfoFlag::kInlineData));
    ASSERT_TRUE(raw_vn_ptr->TestFlag(InodeInfoFlag::kDataExist));

    // Truncated to non-zero size, kDataExist should be set
    test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filenames[2], O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    raw_vn_ptr = static_cast<FuchsiaTestFile *>(test_file.get())->GetRawVnodePtr();
    ASSERT_TRUE(raw_vn_ptr->TestFlag(InodeInfoFlag::kInlineData));
    ASSERT_TRUE(raw_vn_ptr->TestFlag(InodeInfoFlag::kDataExist));

    // Truncated to zero size, kDataExist should be unset
    test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filenames[3], O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    raw_vn_ptr = static_cast<FuchsiaTestFile *>(test_file.get())->GetRawVnodePtr();
    ASSERT_TRUE(raw_vn_ptr->TestFlag(InodeInfoFlag::kInlineData));
    ASSERT_FALSE(raw_vn_ptr->TestFlag(InodeInfoFlag::kDataExist));
  }
}

}  // namespace
}  // namespace f2fs
