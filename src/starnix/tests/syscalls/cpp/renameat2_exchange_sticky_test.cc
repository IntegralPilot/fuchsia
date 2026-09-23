// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Tests for sticky bit enforcement during rename operations.

#include <fcntl.h>
#include <grp.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

#include <cstring>
#include <string>

#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

constexpr uid_t kAliceUid = 1000;
constexpr gid_t kAliceGid = 1000;
constexpr uid_t kBobUid = 1001;
constexpr gid_t kBobGid = 1001;

constexpr const char* kAliceContent = "ALICE";
constexpr const char* kBobContent = "BOB__";

class RenameExchangeStickyTest : public ::testing::Test {
 protected:
  std::string sticky_dir_;
  test_helper::ScopedTempDir temp_dir_;
  test_helper::ScopedMount scoped_mount_;

  void SetUp() override {
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "need CAP_SYS_ADMIN to mount tmpfs and chown";
    }

    sticky_dir_ = temp_dir_.path();

    auto mount_result = test_helper::ScopedMount::Mount("", sticky_dir_, "tmpfs", 0, "");
    ASSERT_THAT(mount_result, SyscallResultIsOk()) << "mount tmpfs failed";
    scoped_mount_ = std::move(mount_result.value());

    // Make the mount world-writable + sticky (mode 1777, like a real /tmp).
    ASSERT_EQ(chmod(sticky_dir_.c_str(), S_ISVTX | S_IRWXU | S_IRWXG | S_IRWXO), 0)
        << "chmod 1777 on tmpfs: " << strerror(errno);

    // Confirm sticky bit is set - if it's not, the whole test is meaningless.
    struct stat st;
    ASSERT_EQ(stat(sticky_dir_.c_str(), &st), 0);
    ASSERT_NE(st.st_mode & S_ISVTX, 0u) << "tmpfs mount is not sticky";
  }

  // Create a 0644 file owned by `uid:gid` with the given content.
  std::string CreateOwnedFile(const std::string& basename, uid_t uid, gid_t gid,
                              const std::string& content) {
    std::string path = sticky_dir_ + "/" + basename;
    fbl::unique_fd fd(open(path.c_str(), O_CREAT | O_WRONLY | O_TRUNC, 0644));
    EXPECT_TRUE(fd.is_valid()) << "open " << path << ": " << strerror(errno);
    SAFE_SYSCALL(write(fd.get(), content.data(), content.size()));
    SAFE_SYSCALL(fchown(fd.get(), uid, gid));
    SAFE_SYSCALL(fchmod(fd.get(), 0644));
    return path;
  }

  static std::string ReadAll(const std::string& path) {
    int fd = open(path.c_str(), O_RDONLY);
    if (fd < 0)
      return std::string();
    char buf[64] = {};
    ssize_t n = read(fd, buf, sizeof(buf) - 1);
    close(fd);
    if (n < 0)
      return std::string();
    return std::string(buf, static_cast<size_t>(n));
  }
};

// Test that renaming a file over another user's file in a sticky directory
// fails with EPERM for an unprivileged caller.
TEST_F(RenameExchangeStickyTest, SingleRenameChecksSticky) {
  std::string alice_path = CreateOwnedFile("alice", kAliceUid, kAliceGid, kAliceContent);
  std::string bob_path = CreateOwnedFile("bob", kBobUid, kBobGid, kBobContent);

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(setgroups(0, nullptr));
    SAFE_SYSCALL(setresgid(kAliceGid, kAliceGid, kAliceGid));
    SAFE_SYSCALL(setresuid(kAliceUid, kAliceUid, kAliceUid));

    ASSERT_THAT(rename(alice_path.c_str(), bob_path.c_str()), SyscallFailsWithErrno(EPERM));
  });

  ASSERT_TRUE(helper.WaitForChildren());

  // Read post-state as root.
  std::string bob_after = ReadAll(bob_path);
  struct stat bob_after_st{};
  SAFE_SYSCALL(stat(bob_path.c_str(), &bob_after_st));

  EXPECT_EQ(bob_after, kBobContent) << "bob's content must remain";
  EXPECT_EQ(bob_after_st.st_uid, kBobUid)
      << "CRITICAL: ownership at bob_path flipped to Alice via rename";
}

// Test that renameat2 with RENAME_EXCHANGE fails with EPERM if the caller
// attempts to replace a file they do not own in a sticky directory.
TEST_F(RenameExchangeStickyTest, ExchangeChecksSticky) {
  std::string alice_path = CreateOwnedFile("alice", kAliceUid, kAliceGid, kAliceContent);
  std::string bob_path = CreateOwnedFile("bob", kBobUid, kBobGid, kBobContent);

  struct stat alice_before, bob_before;
  ASSERT_EQ(stat(alice_path.c_str(), &alice_before), 0);
  ASSERT_EQ(stat(bob_path.c_str(), &bob_before), 0);
  ASSERT_EQ(alice_before.st_uid, kAliceUid);
  ASSERT_EQ(bob_before.st_uid, kBobUid);

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(setgroups(0, nullptr));
    SAFE_SYSCALL(setresgid(kAliceGid, kAliceGid, kAliceGid));
    SAFE_SYSCALL(setresuid(kAliceUid, kAliceUid, kAliceUid));

    ASSERT_THAT(
        renameat2(AT_FDCWD, alice_path.c_str(), AT_FDCWD, bob_path.c_str(), RENAME_EXCHANGE),
        SyscallFailsWithErrno(EPERM));
  });

  ASSERT_TRUE(helper.WaitForChildren());

  std::string bob_after = ReadAll(bob_path);
  std::string alice_after = ReadAll(alice_path);
  struct stat bob_after_st{};
  SAFE_SYSCALL(stat(bob_path.c_str(), &bob_after_st));

  EXPECT_EQ(bob_after, kBobContent)
      << "CRITICAL: content at bob_path was replaced by Alice (cross-user write via EXCHANGE)";
  EXPECT_EQ(bob_after_st.st_uid, kBobUid)
      << "CRITICAL: ownership at bob_path flipped to Alice via EXCHANGE";
}

// Test that renameat2 with RENAME_EXCHANGE succeeds when both files are
// owned by the caller in a sticky directory.
TEST_F(RenameExchangeStickyTest, ExchangeSameOwnerAllowed) {
  std::string a_path = CreateOwnedFile("alice_a", kAliceUid, kAliceGid, "AAAAA");
  std::string b_path = CreateOwnedFile("alice_b", kAliceUid, kAliceGid, "BBBBB");

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(setgroups(0, nullptr));
    SAFE_SYSCALL(setresgid(kAliceGid, kAliceGid, kAliceGid));
    SAFE_SYSCALL(setresuid(kAliceUid, kAliceUid, kAliceUid));

    SAFE_SYSCALL(renameat2(AT_FDCWD, a_path.c_str(), AT_FDCWD, b_path.c_str(), RENAME_EXCHANGE));
  });

  ASSERT_TRUE(helper.WaitForChildren());
  EXPECT_EQ(ReadAll(a_path), "BBBBB");
  EXPECT_EQ(ReadAll(b_path), "AAAAA");
}

}  // namespace
