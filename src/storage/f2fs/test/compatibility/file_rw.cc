// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/test/compatibility/compatibility.h"

namespace f2fs {
namespace {

using FileRWCompatibilityTest = F2fsGuestTest;

TEST_F(FileRWCompatibilityTest, WriteVerifyLinuxToFuchsia) {
  constexpr uint32_t kVerifyPatternSize = 64 * 1024;  // 64 KB
  constexpr uint32_t num_blocks = kVerifyPatternSize / kBlockSize;
  const std::string filename = "alpha";

  // TODO: Support various mkfs options such as
  // "-O extra_attr"
  // "-O extra_attr,project_quota"
  // "-O extra_attr,inode_checksum"
  // "-O extra_attr,inode_crtime"
  // "-O extra_attr,compression"
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
      test_file->WritePattern(num_blocks, 1);
    }

    // Verify on Fuchsia
    {
      GetEnclosedGuest().GetFuchsiaOperator().Fsck();
      GetEnclosedGuest().GetFuchsiaOperator().Mount();

      auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

      auto test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filename, O_RDWR, 0644);
      ASSERT_TRUE(test_file->IsValid());
      test_file->VerifyPattern(num_blocks, 1);
    }
  }
}

TEST_F(FileRWCompatibilityTest, WriteVerifyFuchsiaToLinux) {
  constexpr uint32_t kVerifyPatternSize = 64 * 1024;  // 64 KB
  constexpr uint32_t num_blocks = kVerifyPatternSize / kBlockSize;
  const std::string filename = "alpha";

  // File write on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Mkfs();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filename, O_CREAT | O_RDWR, 0644);
    ASSERT_TRUE(test_file->IsValid());
    test_file->WritePattern(num_blocks, 1);
  }

  // Verify on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto testfile = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filename, O_RDWR, 0644);
    ASSERT_TRUE(testfile->IsValid());
    testfile->VerifyPattern(num_blocks, 1);
  }
}

TEST_F(FileRWCompatibilityTest, TruncateLinuxToFuchsia) {
  constexpr uint32_t kVerifyPatternSize = 64 * 1024;  // 64 KB
  constexpr uint32_t num_blocks = kVerifyPatternSize / kBlockSize;
  constexpr off_t kTruncateSize = 16 * 1024;  // 16 KB
  constexpr uint32_t num_blocks_truncated = kTruncateSize / kBlockSize;

  const std::string extend_filename = "extend";
  const std::string shrink_filename = "shrink";

  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto extend_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + extend_filename, O_RDWR | O_CREAT, 0755);
    ASSERT_TRUE(extend_file->IsValid());
    ASSERT_EQ(extend_file->Ftruncate(kTruncateSize), 0);

    auto shrink_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + shrink_filename, O_RDWR | O_CREAT, 0755);
    ASSERT_TRUE(shrink_file->IsValid());
    shrink_file->WritePattern(num_blocks, 1);

    ASSERT_EQ(shrink_file->Ftruncate(kTruncateSize), 0);
  }

  // Verify on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    auto extend_file = GetEnclosedGuest().GetFuchsiaOperator().Open(extend_filename, O_RDWR, 0755);
    ASSERT_TRUE(extend_file->IsValid());

    struct stat extend_file_stat{};
    ASSERT_EQ(extend_file->Fstat(extend_file_stat), 0);
    ASSERT_EQ(extend_file_stat.st_size, kTruncateSize);

    uint32_t buffer[kBlockSize / sizeof(uint32_t)];

    for (uint32_t i = 0; i < kTruncateSize / sizeof(buffer); ++i) {
      ASSERT_EQ(extend_file->Read(buffer, sizeof(buffer)), static_cast<ssize_t>(sizeof(buffer)));

      for (uint32_t j = 0; j < sizeof(buffer) / sizeof(uint32_t); ++j) {
        ASSERT_EQ(buffer[j], static_cast<uint32_t>(0));
      }
    }

    auto shrink_file = GetEnclosedGuest().GetFuchsiaOperator().Open(shrink_filename, O_RDWR, 0755);
    ASSERT_TRUE(shrink_file->IsValid());

    struct stat shrink_file_stat{};
    ASSERT_EQ(shrink_file->Fstat(shrink_file_stat), 0);
    ASSERT_EQ(shrink_file_stat.st_size, kTruncateSize);

    shrink_file->VerifyPattern(num_blocks_truncated, 1);
  }
}

TEST_F(FileRWCompatibilityTest, TruncateFuchsiaToLinux) {
  constexpr uint32_t kVerifyPatternSize = 64 * 1024;  // 64 KB
  constexpr uint32_t num_blocks = kVerifyPatternSize / kBlockSize;
  constexpr off_t kTruncateSize = 16 * 1024;  // 16 KB
  constexpr uint32_t num_blocks_truncated = kTruncateSize / kBlockSize;

  const std::string extend_filename = "extend";
  const std::string shrink_filename = "shrink";

  {
    GetEnclosedGuest().GetFuchsiaOperator().Mkfs();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    auto extend_file =
        GetEnclosedGuest().GetFuchsiaOperator().Open(extend_filename, O_RDWR | O_CREAT, 0755);
    ASSERT_TRUE(extend_file->IsValid());
    ASSERT_EQ(extend_file->Ftruncate(kTruncateSize), 0);

    auto shrink_file =
        GetEnclosedGuest().GetFuchsiaOperator().Open(shrink_filename, O_RDWR | O_CREAT, 0755);
    ASSERT_TRUE(shrink_file->IsValid());
    shrink_file->WritePattern(num_blocks, 1);

    ASSERT_EQ(shrink_file->Ftruncate(kTruncateSize), 0);
  }

  // Verify on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto extend_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + extend_filename, O_RDWR, 0755);
    ASSERT_TRUE(extend_file->IsValid());

    struct stat extend_file_stat{};
    ASSERT_EQ(extend_file->Fstat(extend_file_stat), 0);
    ASSERT_EQ(extend_file_stat.st_size, kTruncateSize);

    // Check zero-filled
    std::string result;
    GetEnclosedGuest().GetLinuxOperator().ExecuteWithAssert(
        {"<" + GetEnclosedGuest().GetLinuxOperator().ConvertPath(std::string(kLinuxPathPrefix) +
                                                                 extend_filename),
         "tr -d '\\0'", "|", "wc -c", "|", "tr -d '\\n'"},
        &result);

    ASSERT_EQ(result, "0");

    auto shrink_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + shrink_filename, O_RDWR, 0755);
    ASSERT_TRUE(shrink_file->IsValid());
    shrink_file->VerifyPattern(num_blocks_truncated, 1);
  }
}

TEST_F(FileRWCompatibilityTest, FileReadExceedFileSizeOnFuchsia) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  constexpr uint32_t kDataSize = 7 * 1024;      // 7kb
  constexpr uint32_t kReadLocation = 5 * 1024;  // 5kb

  const std::string filename = "alpha";

  char w_buf[kDataSize];
  for (size_t i = 0; i < kDataSize; ++i) {
    w_buf[i] = static_cast<char>(rand() % 128);
  }

  // Write on Host
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filename, O_RDWR | O_CREAT, 0644);
    ASSERT_TRUE(test_file->IsValid());

    ASSERT_EQ(test_file->Write(w_buf, sizeof(w_buf)), static_cast<ssize_t>(sizeof(w_buf)));
  }

  // Verify on Fuchsia. Try reading excess file size.
  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filename, O_RDWR, 0644);
    ASSERT_TRUE(test_file->IsValid());

    char r_buf[kReadLocation + kBlockSize];
    ASSERT_EQ(test_file->Read(r_buf, kReadLocation), static_cast<ssize_t>(kReadLocation));
    ASSERT_EQ(test_file->Read(&(r_buf[kReadLocation]), kBlockSize),
              static_cast<ssize_t>(kDataSize - kReadLocation));

    ASSERT_EQ(memcmp(r_buf, w_buf, kDataSize), 0);
  }
}

}  // namespace
}  // namespace f2fs
