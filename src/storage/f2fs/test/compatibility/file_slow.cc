// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/test/compatibility/compatibility.h"

namespace f2fs {
namespace {

using FileSlowCompatibilityTest = F2fsGuestTest;

TEST_F(FileSlowCompatibilityTest, SlowWriteVerifyLinuxToFuchsia) {
  constexpr uint32_t kVerifyPatternSize = 32 * 1024 * 1024;  // 32 MB
  constexpr uint32_t num_blocks = kVerifyPatternSize / kBlockSize;
  constexpr uint32_t kPatternInterval = 16;
  const std::string filename = "alpha";

  std::string mkfs_option_list[] = {""};

  for (std::string_view mkfs_option : mkfs_option_list) {
    // File write on Linux
    {
      GetEnclosedGuest().GetLinuxOperator().Mkfs(mkfs_option);
      GetEnclosedGuest().GetLinuxOperator().Mount();

      auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

      auto test_file = GetEnclosedGuest().GetLinuxOperator().Open(
          std::string(kLinuxPathPrefix) + filename, O_CREAT | O_RDWR, 0644);
      ASSERT_TRUE(test_file->IsValid());
      test_file->WritePattern(num_blocks, kPatternInterval);
    }

    // Verify on Fuchsia
    {
      GetEnclosedGuest().GetFuchsiaOperator().Fsck();
      GetEnclosedGuest().GetFuchsiaOperator().Mount();

      auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

      auto test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filename, O_RDWR, 0644);
      ASSERT_TRUE(test_file->IsValid());
      test_file->VerifyPattern(num_blocks, kPatternInterval);
    }
  }
}

TEST_F(FileSlowCompatibilityTest, SlowWriteVerifyFuchsiaToLinux) {
  constexpr uint32_t kVerifyPatternSize = 32 * 1024 * 1024;  // 32 MB
  constexpr uint32_t num_blocks = kVerifyPatternSize / kBlockSize;
  constexpr uint32_t kPatternInterval = 16;
  const std::string filename = "alpha";

  // File write on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Mkfs();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filename, O_CREAT | O_RDWR, 0644);
    ASSERT_TRUE(test_file->IsValid());
    test_file->WritePattern(num_blocks, kPatternInterval);
  }

  // Verify on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto testfile = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filename, O_RDWR, 0644);
    ASSERT_TRUE(testfile->IsValid());
    testfile->VerifyPattern(num_blocks, kPatternInterval);
  }
}

TEST_F(FileSlowCompatibilityTest, SlowFallocateLinuxToFuchsia) {
  constexpr uint32_t kVerifyPatternSize = 32 * 1024 * 1024;  // 32 MB
  constexpr off_t kOffset = 5000;
  constexpr uint32_t num_blocks = kVerifyPatternSize / kBlockSize;
  constexpr uint32_t kPatternInterval = 16;
  const std::string filename = "alpha";

  struct stat host_stat;
  struct stat target_stat;

  // Fallocate on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filename, O_CREAT | O_RDWR, 0644);
    ASSERT_TRUE(test_file->IsValid());

    test_file->Fallocate(0, kOffset, kVerifyPatternSize);

    ASSERT_EQ(test_file->Fstat(host_stat), 0);
  }

  // Verify and write on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filename, O_RDWR, 0644);
    ASSERT_TRUE(test_file->IsValid());
    ASSERT_EQ(test_file->Fstat(target_stat), 0);
    CompareStat(target_stat, host_stat);

    test_file->WritePattern(num_blocks, kPatternInterval);
  }

  // Verify on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto testfile = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filename, O_RDWR, 0644);
    ASSERT_TRUE(testfile->IsValid());
    testfile->VerifyPattern(num_blocks, kPatternInterval);
  }
}

}  // namespace
}  // namespace f2fs
