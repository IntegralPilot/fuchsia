// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <grp.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <unistd.h>

#include <filesystem>
#include <optional>

#include <linux/capability.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

constexpr int kOutputFd = 100;

constexpr uid_t kUser1Uid = 65533;
constexpr gid_t kUser1Gid = 65534;
constexpr uid_t kRootUid = 0;
constexpr gid_t kRootGid = 0;

std::string GetCredsBinaryPath() {
  return test_helper::GetTestResourcePath("print_uid_gid_exec_child");
}

bool change_ids(uid_t user, gid_t group) {
  return (setresgid(group, group, group) == 0) && (setresuid(user, user, user) == 0);
}

}  // namespace

class SuidTest : public ::testing::Test, public ::testing::WithParamInterface<unsigned long> {
 public:
  static unsigned long mount_flags() { return GetParam(); }

  std::optional<std::string> MountTmpFs(const std::string &temp_dir) {
    std::string temp = temp_dir + "/tmp";
    EXPECT_THAT(mkdir(temp.c_str(), S_IRWXU), SyscallSucceeds());

    int res = mount(nullptr, temp.c_str(), "tmpfs", mount_flags(), "");
    EXPECT_EQ(res, 0) << "mount: " << std::strerror(errno);

    if (res != 0) {
      return std::nullopt;
    }

    return temp;
  }
};

TEST_P(SuidTest, SuidBinaryBecomesRoot) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  std::string creds_binary = GetCredsBinaryPath();

  test_helper::ScopedTempDir temp_dir;
  auto mounted = MountTmpFs(temp_dir.path());
  ASSERT_TRUE(mounted.has_value()) << "failed to mount fs";
  auto mount_path = mounted.value();

  // Directory Permissions: owner can do everything, user and other can search.
  constexpr int kDirPerms = S_IRWXU | S_IXGRP | S_IXOTH;

  SAFE_SYSCALL(chmod(mount_path.c_str(), kDirPerms));
  SAFE_SYSCALL(chmod(temp_dir.path().c_str(), kDirPerms));

  std::string suid_binary_path = files::JoinPath(mount_path, "suid_binary_becomes_root");
  std::filesystem::copy_file(creds_binary, suid_binary_path);

  // File permissions: A set-user-ID binary that can be executed by Others.
  constexpr int kBinaryPerms = S_ISUID | S_IXOTH;
  SAFE_SYSCALL(chown(suid_binary_path.c_str(), kRootUid, kRootGid));
  SAFE_SYSCALL(chmod(suid_binary_path.c_str(), kBinaryPerms));

  // Binary will output a string to this memfd.
  int fd = SAFE_SYSCALL(test_helper::MemFdCreate("creds", O_RDWR));
  pid_t pid = SAFE_SYSCALL(fork());
  if (pid == 0) {
    ASSERT_TRUE(fcntl(kOutputFd, F_GETFD) == -1 && errno == EBADF);
    SAFE_SYSCALL(dup2(fd, kOutputFd));

    SAFE_SYSCALL(setgroups(0, nullptr));  // drop all supplementary groups.
    ASSERT_TRUE(change_ids(kUser1Uid, kUser1Gid));

    char *const argv[] = {const_cast<char *>(suid_binary_path.c_str()), nullptr};

    SAFE_SYSCALL(execve(suid_binary_path.c_str(), argv, nullptr));
    _exit(EXIT_FAILURE);
  }

  int status = 0;
  SAFE_SYSCALL(waitpid(pid, &status, 0));
  EXPECT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0);

  SAFE_SYSCALL(lseek(fd, 0, SEEK_SET));
  FILE *fp = fdopen(fd, "r");

  {
    uid_t ruid, euid, suid;
    EXPECT_EQ(fscanf(fp, "ruid: %u euid: %u suid: %u\n", &ruid, &euid, &suid), 3);
    EXPECT_EQ(ruid, kUser1Uid);
    if (mount_flags() & MS_NOSUID) {
      EXPECT_EQ(euid, kUser1Uid);
      EXPECT_EQ(suid, kUser1Uid);
    } else {
      EXPECT_EQ(euid, kRootUid);
      EXPECT_EQ(suid, kRootUid);
    }
  }

  {
    gid_t rgid, egid, sgid;
    EXPECT_EQ(fscanf(fp, "rgid: %u egid: %u sgid: %u\n", &rgid, &egid, &sgid), 3);
    EXPECT_EQ(rgid, kUser1Gid);
    EXPECT_EQ(egid, kUser1Gid);
    EXPECT_EQ(sgid, kUser1Gid);
  }

  fclose(fp);
}

TEST_P(SuidTest, FileModificationsRemoveSuid) {
  if (!test_helper::HasSysAdmin() || !test_helper::HasCapability(CAP_FSETID)) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  test_helper::ScopedTempDir temp_dir;
  auto mounted = MountTmpFs(temp_dir.path());
  ASSERT_TRUE(mounted.has_value()) << "failed to mount fs";
  auto mount_path = mounted.value();

  test_helper::ForkHelper helper;

  // We will drop capabilities, so let's do that inside a new process.
  helper.RunInForkedProcess([&] {
    std::string test_suid_file = files::JoinPath(mount_path, "file_modification_drops_suid");
    int fd = SAFE_SYSCALL(creat(test_suid_file.c_str(), S_ISUID | S_ISGID | S_IRWXU));

    struct stat file_stat;
    SAFE_SYSCALL(fstat(fd, &file_stat));
    EXPECT_NE((file_stat.st_mode & S_ISUID), 0U);

    uint8_t data = 'x';

    // Because we have CAP_FSETID, modifying the file doesn't remove the
    // set-user-ID bit.
    SAFE_SYSCALL(write(fd, &data, sizeof(data)));
    SAFE_SYSCALL(fstat(fd, &file_stat));
    EXPECT_NE((file_stat.st_mode & S_ISUID), 0U);

    // After dropping CAP_FSETID, modifications to the file should remove the
    // set-user-ID bit.
    test_helper::UnsetCapability(CAP_FSETID);
    SAFE_SYSCALL(write(fd, &data, sizeof(data)));
    SAFE_SYSCALL(fstat(fd, &file_stat));
    EXPECT_EQ((file_stat.st_mode & S_ISUID), 0U);

    // Setting the file as set-user-ID again works.
    SAFE_SYSCALL(fchmod(fd, S_ISUID | S_IRWXU));
    SAFE_SYSCALL(fstat(fd, &file_stat));
    EXPECT_NE((file_stat.st_mode & S_ISUID), 0U);

    close(fd);

    // But can be removed again by truncating the file.
    SAFE_SYSCALL(truncate(test_suid_file.c_str(), 0));
    SAFE_SYSCALL(stat(test_suid_file.c_str(), &file_stat));
    EXPECT_EQ((file_stat.st_mode & S_ISUID), 0U);

    SAFE_SYSCALL(unlink(test_suid_file.c_str()));
  });
}

TEST_P(SuidTest, OwnershipChangesRemoveSuid) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }

  test_helper::ScopedTempDir temp_dir;
  auto mounted = MountTmpFs(temp_dir.path());
  ASSERT_TRUE(mounted.has_value()) << "failed to mount fs";
  auto mount_path = mounted.value();

  std::string test_suid_file = files::JoinPath(mount_path, "ownership_change_drops_suid");
  int fd = SAFE_SYSCALL(creat(test_suid_file.c_str(), S_ISUID | S_IRWXU));

  struct stat file_stat;
  SAFE_SYSCALL(fstat(fd, &file_stat));
  EXPECT_NE((file_stat.st_mode & S_ISUID), 0U);

  SAFE_SYSCALL(fchown(fd, kUser1Uid, kRootGid));
  SAFE_SYSCALL(fstat(fd, &file_stat));
  EXPECT_EQ((file_stat.st_mode & S_ISUID), 0U);
  close(fd);
  SAFE_SYSCALL(unlink(test_suid_file.c_str()));
}

INSTANTIATE_TEST_SUITE_P(SuidTest, SuidTest, ::testing::Values(0, MS_NOSUID));
