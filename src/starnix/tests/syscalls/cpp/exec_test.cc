// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/fsuid.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <string>

#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

constexpr int kOutputFd = 100;
constexpr uid_t kTestUid = 65533;
constexpr gid_t kTestGid = 65534;

std::string GetCredsBinaryPath() {
  return test_helper::GetTestResourcePath("print_uid_gid_exec_child");
}

}  // namespace

TEST(ExecTest, FsuidFsgidResetOnExec) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }

  std::string creds_binary = GetCredsBinaryPath();

  int fd = SAFE_SYSCALL(test_helper::MemFdCreate("creds", O_RDWR));

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(dup2(fd, kOutputFd));

    // We start as root (ruid=0, euid=0, fsuid=0).
    // We want to set euid to kTestUid, but keep fsuid as 0.
    // This allows us to execute the root-owned (700) helper binary,
    // while still testing that fsuid is reset to euid (kTestUid) on exec.

    SAFE_SYSCALL(setegid(kTestGid));
    SAFE_SYSCALL(seteuid(kTestUid));

    // seteuid/setegid also set fsuid/fsgid to the new euid/egid.
    // We explicitly set them back to 0 (root).
    // This is allowed because our real UID/GID are still 0.
    ASSERT_EQ(setfsuid(0), static_cast<int>(kTestUid));
    ASSERT_EQ(setfsgid(0), static_cast<int>(kTestGid));

    // Verify the state before exec.
    uid_t ruid, euid, suid;
    SAFE_SYSCALL(getresuid(&ruid, &euid, &suid));
    ASSERT_EQ(ruid, 0U);
    ASSERT_EQ(euid, kTestUid);

    gid_t rgid, egid, sgid;
    SAFE_SYSCALL(getresgid(&rgid, &egid, &sgid));
    ASSERT_EQ(rgid, 0U);
    ASSERT_EQ(egid, kTestGid);

    ASSERT_EQ(setfsuid(-1), 0);
    ASSERT_EQ(setfsgid(-1), 0);

    char *const argv[] = {const_cast<char *>(creds_binary.c_str()), nullptr};
    execve(creds_binary.c_str(), argv, nullptr);
    perror("execve");
    _exit(EXIT_FAILURE);
  });

  ASSERT_TRUE(helper.WaitForChildren());

  SAFE_SYSCALL(lseek(fd, 0, SEEK_SET));
  FILE *fp = fdopen(fd, "r");
  ASSERT_NE(fp, nullptr);

  uid_t ruid, euid, suid;
  EXPECT_EQ(fscanf(fp, "ruid: %u euid: %u suid: %u\n", &ruid, &euid, &suid), 3);
  EXPECT_EQ(ruid, 0U);
  EXPECT_EQ(euid, kTestUid);

  gid_t rgid, egid, sgid;
  EXPECT_EQ(fscanf(fp, "rgid: %u egid: %u sgid: %u\n", &rgid, &egid, &sgid), 3);
  EXPECT_EQ(rgid, 0U);
  EXPECT_EQ(egid, kTestGid);

  int fsuid, fsgid;
  EXPECT_EQ(fscanf(fp, "fsuid: %d fsgid: %d\n", &fsuid, &fsgid), 2);
  // fsuid/fsgid should have been reset to euid/egid (kTestUid/kTestGid) on exec,
  // even though they were 0 before exec.
  EXPECT_EQ(fsuid, static_cast<int>(kTestUid));
  EXPECT_EQ(fsgid, static_cast<int>(kTestGid));

  fclose(fp);
}
