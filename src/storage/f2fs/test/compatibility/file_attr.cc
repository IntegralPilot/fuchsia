// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/storage/f2fs/test/compatibility/compatibility.h"

namespace f2fs {
namespace {

using FileAttrCompatibilityTest = F2fsGuestTest;

TEST_F(FileAttrCompatibilityTest, VerifyAttributesLinuxToFuchsia) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  std::vector<std::pair<std::string, struct stat>> test_set{};

  // Create files with various file modes
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    for (mode_t mode = 0; mode <= (S_IRWXU | S_IRWXG | S_IRWXO); mode += rand() % 32 + 1) {
      std::string filename = "child";
      filename.append("_").append(std::to_string(mode));

      auto testfile = GetEnclosedGuest().GetLinuxOperator().Open(
          std::string(kLinuxPathPrefix) + filename, O_CREAT, mode);
      ASSERT_TRUE(testfile->IsValid());

      testfile->Fchmod(mode);

      struct stat file_stat{};
      ASSERT_EQ(testfile->Fstat(file_stat), 0);

      test_set.push_back({filename, file_stat});
    }
  }

  // Verify on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    for (auto [name, stat_from_linux] : test_set) {
      auto child_file = GetEnclosedGuest().GetFuchsiaOperator().Open(name, O_RDONLY, 0644);
      ASSERT_TRUE(child_file->IsValid());

      struct stat child_stat{};
      ASSERT_EQ(child_file->Fstat(child_stat), 0);
      CompareStat(child_stat, stat_from_linux);
    }
  }
}

char GetRandomFileNameChar() {
  const char kValidChars[] =
      "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz()<>+-=_,:;";
  return kValidChars[rand() % (sizeof(kValidChars) - 1)];
}

std::vector<std::string> GetRandomFileNameSet() {
  constexpr int kMaxNameLength = 255;
  std::vector<std::string> file_name_set;

  for (int len = 1; len <= kMaxNameLength; len += rand() % 20 + 1) {
    std::string file_name = "/";
    for (int i = 0; i < len; ++i) {
      file_name.push_back(GetRandomFileNameChar());
    }
    file_name_set.push_back(file_name);
  }
  return file_name_set;
}

TEST_F(FileAttrCompatibilityTest, FileNameTestLinuxToFuchsia) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  auto file_name_set = GetRandomFileNameSet();
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    for (auto file_name : file_name_set) {
      auto file = GetEnclosedGuest().GetLinuxOperator().Open(
          std::string(kLinuxPathPrefix) + file_name, O_RDWR | O_CREAT, 0644);
      ASSERT_TRUE(file->IsValid());
    }
  }

  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    for (auto file_name : file_name_set) {
      auto file = GetEnclosedGuest().GetFuchsiaOperator().Open(file_name, O_RDONLY, 0644);
      ASSERT_TRUE(file->IsValid());
    }
  }
}

TEST_F(FileAttrCompatibilityTest, FileNameTestFuchsiaToLinux) {
  srand(testing::UnitTest::GetInstance()->random_seed());

  auto file_name_set = GetRandomFileNameSet();
  {
    GetEnclosedGuest().GetFuchsiaOperator().Mkfs();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    for (auto file_name : file_name_set) {
      auto file = GetEnclosedGuest().GetFuchsiaOperator().Open(file_name, O_RDWR | O_CREAT, 0644);
      ASSERT_TRUE(file->IsValid());
    }
  }

  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    for (auto file_name : file_name_set) {
      auto file = GetEnclosedGuest().GetLinuxOperator().Open(
          std::string(kLinuxPathPrefix) + file_name, O_RDONLY, 0644);
      ASSERT_TRUE(file->IsValid());
    }
  }
}

TEST_F(FileAttrCompatibilityTest, FileRenameTestLinuxToFuchsia) {
  std::vector<std::string> dir_paths = {"/d_a", "/d_a/d_b", "/d_c"};
  std::vector<std::pair<std::string, std::string>> rename_from_to = {
      {"/f_0", "/f_0_"},
      {"/f_1", "/d_c/f_1_"},
      {"/d_a/f_a0", "/d_c/f_a0_"},
      {"/d_a/d_b/f_ab0", "/d_c/f_ab0"}};
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    for (auto dir_name : dir_paths) {
      GetEnclosedGuest().GetLinuxOperator().Mkdir(std::string(kLinuxPathPrefix) + dir_name, 0644);
    }

    // Create
    for (auto [file_name_from, file_name_to] : rename_from_to) {
      auto file = GetEnclosedGuest().GetLinuxOperator().Open(
          std::string(kLinuxPathPrefix) + file_name_from, O_RDWR | O_CREAT, 0644);
      ASSERT_TRUE(file->IsValid());
    }

    // Rename
    for (auto [file_name_from, file_name_to] : rename_from_to) {
      GetEnclosedGuest().GetLinuxOperator().Rename(std::string(kLinuxPathPrefix) + file_name_from,
                                                   std::string(kLinuxPathPrefix) + file_name_to);
    }
  }

  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    for (auto [file_name_from, file_name_to] : rename_from_to) {
      auto file = GetEnclosedGuest().GetFuchsiaOperator().Open(file_name_from, O_RDONLY, 0644);
      ASSERT_FALSE(file->IsValid());

      file = GetEnclosedGuest().GetFuchsiaOperator().Open(file_name_to, O_RDONLY, 0644);
      ASSERT_TRUE(file->IsValid());
    }
  }
}

TEST_F(FileAttrCompatibilityTest, FileRenameTestFuchsiaToLinux) {
  std::vector<std::string> dir_paths = {"/d_a", "/d_a/d_b", "/d_c"};
  std::vector<std::pair<std::string, std::string>> rename_from_to = {
      {"/f_0", "/f_0_"},
      {"/f_1", "/d_c/f_1_"},
      {"/d_a/f_a0", "/d_c/f_a0_"},
      {"/d_a/d_b/f_ab0", "/d_c/f_ab0"}};
  {
    GetEnclosedGuest().GetFuchsiaOperator().Mkfs();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    for (auto dir_name : dir_paths) {
      GetEnclosedGuest().GetFuchsiaOperator().Mkdir(dir_name, 0644);
    }

    // Create
    for (auto [file_name_from, file_name_to] : rename_from_to) {
      auto file =
          GetEnclosedGuest().GetFuchsiaOperator().Open(file_name_from, O_RDWR | O_CREAT, 0644);
      ASSERT_TRUE(file->IsValid());
    }

    // Rename
    for (auto [file_name_from, file_name_to] : rename_from_to) {
      GetEnclosedGuest().GetFuchsiaOperator().Rename(file_name_from, file_name_to);
    }
  }

  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    for (auto [file_name_from, file_name_to] : rename_from_to) {
      auto file = GetEnclosedGuest().GetLinuxOperator().Open(
          std::string(kLinuxPathPrefix) + file_name_from, O_RDONLY, 0644);
      ASSERT_FALSE(file->IsValid());

      file = GetEnclosedGuest().GetLinuxOperator().Open(
          std::string(kLinuxPathPrefix) + file_name_to, O_RDONLY, 0644);
      ASSERT_TRUE(file->IsValid());
    }
  }
}

TEST_F(FileAttrCompatibilityTest, FallocateLinuxToFuchsia) {
  constexpr uint32_t kVerifyPatternSize = 64 * 1024;  // 64 KB
  constexpr off_t kOffset = 5000;
  constexpr uint32_t num_blocks = kVerifyPatternSize / kBlockSize;
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

TEST_F(FileAttrCompatibilityTest, FallocatePunchHoleLinuxToFuchsia) {
  constexpr off_t kOffset = 3000;
  constexpr off_t kLen = 5000;
  const std::string filename = "alpha";

  struct stat host_stat;
  struct stat target_stat;

  // Fallocate on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filename, O_RDWR | O_CREAT, 0755);
    ASSERT_TRUE(test_file->IsValid());

    test_file->Fallocate(FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, kOffset, kLen);
    ASSERT_EQ(test_file->Fstat(host_stat), 0);
  }

  // Verify on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filename, O_RDWR, 0644);
    ASSERT_TRUE(test_file->IsValid());
    ASSERT_EQ(test_file->Fstat(target_stat), 0);
    CompareStat(target_stat, host_stat);
  }
}

TEST_F(FileAttrCompatibilityTest, VerifyXattrsLinuxToFuchsia) {
  constexpr uint32_t kVerifyPatternSize = 64 * 1024;  // 64KB
  constexpr uint32_t num_blocks = kVerifyPatternSize / kBlockSize;
  const std::string filename = "alpha";

  std::string name = "user.comment";
  std::string value = "\"This is a user comment\"";
  std::string mkfs_option = "-O extra_attr -O flexible_inline_xattr";
  std::string mount_option = "-o inline_xattr,inline_xattr_size=60";

  // Setfattr on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Mkfs(mkfs_option);
    GetEnclosedGuest().GetLinuxOperator().Mount(mount_option);

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filename, O_RDWR | O_CREAT, 0755);
    ASSERT_TRUE(test_file->IsValid());

    auto file_path =
        GetEnclosedGuest().GetLinuxOperator().ConvertPath(std::string(kLinuxPathPrefix) + filename);
    std::string command = "setfattr ";
    command.append("-n ").append(name).append(" -v ").append(value).append(" ").append(file_path);
    GetEnclosedGuest().GetLinuxOperator().ExecuteWithAssert({command});
  }

  // Write on Fuchsia
  {
    GetEnclosedGuest().GetFuchsiaOperator().Fsck();
    GetEnclosedGuest().GetFuchsiaOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetFuchsiaOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetFuchsiaOperator().Open(filename, O_RDWR, 0644);
    ASSERT_TRUE(test_file->IsValid());
    test_file->WritePattern(num_blocks, 1);
  }

  // Verify on Linux
  {
    GetEnclosedGuest().GetLinuxOperator().Fsck();
    GetEnclosedGuest().GetLinuxOperator().Mount();

    auto umount = fit::defer([&] { GetEnclosedGuest().GetLinuxOperator().Umount(); });

    auto test_file = GetEnclosedGuest().GetLinuxOperator().Open(
        std::string(kLinuxPathPrefix) + filename, O_RDWR, 0644);
    ASSERT_TRUE(test_file->IsValid());
    test_file->VerifyPattern(num_blocks, 1);

    auto file_path =
        GetEnclosedGuest().GetLinuxOperator().ConvertPath(std::string(kLinuxPathPrefix) + filename);
    std::string command = "getfattr ";
    command.append("-d ")
        .append(file_path)
        .append(" | grep \"")
        .append(name)
        .append("=\\")
        .append(value)
        .append("\\\"")
        .append(" | tr -d '\\n'");

    std::string result;
    GetEnclosedGuest().GetLinuxOperator().ExecuteWithAssert({command}, &result);
    ASSERT_EQ(result, name + "=" + value + "");
  }
}

}  // namespace
}  // namespace f2fs
