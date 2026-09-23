// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <lib/fit/defer.h>
#include <sys/fsuid.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <sys/mount.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/syscall.h>

#include <linux/fs.h>

#ifndef FS_CASEFOLD_FL
#define FS_CASEFOLD_FL 0x40000000
#endif

#include <sys/sysmacros.h>
#include <sys/types.h>
#include <sys/uio.h>
#include <sys/un.h>
#include <unistd.h>

#include <algorithm>
#include <climits>
#include <cstdint>
#include <optional>
#include <string>
#include <vector>

#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

std::vector<std::string> GetEntries(DIR *d) {
  std::vector<std::string> entries;

  errno = 0;
  struct dirent *entry;
  while ((entry = readdir(d)) != nullptr) {
    entries.push_back(entry->d_name);
  }
  EXPECT_EQ(errno, 0) << "readdir failed: " << strerror(errno);
  return entries;
}

TEST(FsTest, NoDuplicatedDoDirectories) {
  DIR *root_dir = opendir("/");
  std::vector<std::string> entries = GetEntries(root_dir);
  std::vector<std::string> dot_entries;
  std::copy_if(entries.begin(), entries.end(), std::back_inserter(dot_entries),
               [](const std::string &filename) { return filename == "." || filename == ".."; });
  closedir(root_dir);

  ASSERT_EQ(2u, dot_entries.size());
  ASSERT_NE(dot_entries[0], dot_entries[1]);
}

TEST(FsTest, ReadDirRespectsSeek) {
  DIR *root_dir = opendir("/");
  std::vector<std::string> entries = GetEntries(root_dir);
  closedir(root_dir);

  root_dir = opendir("/");
  readdir(root_dir);
  long position = telldir(root_dir);
  closedir(root_dir);
  root_dir = opendir("/");
  seekdir(root_dir, position);
  std::vector<std::string> next_entries = GetEntries(root_dir);
  closedir(root_dir);

  EXPECT_NE(next_entries[0], entries[0]);
  EXPECT_LT(next_entries.size(), entries.size());
  // Remove the first elements from entries
  entries.erase(entries.begin(), entries.begin() + (entries.size() - next_entries.size()));
  EXPECT_EQ(entries, next_entries);
}

TEST(FsTest, FchmodTest) {
  char *tmp = getenv("TEST_TMPDIR");
  std::string path = tmp == nullptr ? "/tmp/fchmodtest" : std::string(tmp) + "/fchmodtest";
  int fd = open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0777);
  ASSERT_GE(fd, 0);
  ASSERT_EQ(fchmod(fd, S_IRWXU | S_IRWXG), 0);
  ASSERT_EQ(fchmod(fd, S_IRWXU | S_IRWXG | S_IFCHR), 0);
}

TEST(FsTest, DevTmpFsInitialDirectoryLinkCount) {
  auto mount_info = test_helper::ReadMountInfoLine("/dev");
  if (!mount_info || mount_info->fs_type != "devtmpfs") {
    GTEST_SKIP() << "/dev is not a devtmpfs mount, skipping test.";
  }

  struct stat st = {};
  ASSERT_THAT(stat("/dev", &st), SyscallSucceeds());
  // /dev is a devtmpfs instance containing initial subdirectories (e.g., /dev/shm, /dev/pts).
  // Under POSIX, each child subdirectory increments the parent's link count for its ".." entry,
  // so /dev must have at least 2 + 2 = 4 links.
  EXPECT_GE(st.st_nlink, 4u);
}

TEST(FsTest, DirectoryLinkCountAndRmdirEmptiness) {
  test_helper::ScopedTempDir temp_dir;
  std::string parent = temp_dir.path();

  struct stat parent_stat = {};
  ASSERT_THAT(stat(parent.c_str(), &parent_stat), SyscallSucceeds());
  nlink_t initial_nlink = parent_stat.st_nlink;

  std::string sub = parent + "/sub";
  ASSERT_THAT(mkdir(sub.c_str(), 0755), SyscallSucceeds());

  ASSERT_THAT(stat(parent.c_str(), &parent_stat), SyscallSucceeds());
  EXPECT_EQ(parent_stat.st_nlink, initial_nlink + 1);

  struct stat sub_stat = {};
  ASSERT_THAT(stat(sub.c_str(), &sub_stat), SyscallSucceeds());
  EXPECT_EQ(sub_stat.st_nlink, 2u);

  std::string file = sub + "/file.txt";
  int fd = open(file.c_str(), O_CREAT | O_WRONLY | O_EXCL, 0644);
  ASSERT_GE(fd, 0);
  close(fd);

  // Attempting to rmdir a non-empty directory must fail with ENOTEMPTY.
  EXPECT_THAT(rmdir(sub.c_str()), SyscallFailsWithErrno(ENOTEMPTY));

  // Unlink the file.
  ASSERT_THAT(unlink(file.c_str()), SyscallSucceeds());

  // Now rmdir on the empty directory succeeds and decrements parent link count.
  EXPECT_THAT(rmdir(sub.c_str()), SyscallSucceeds());
  ASSERT_THAT(stat(parent.c_str(), &parent_stat), SyscallSucceeds());
  EXPECT_EQ(parent_stat.st_nlink, initial_nlink);
}

TEST(FsTest, RenameDirectoryLinkCounts) {
  test_helper::ScopedTempDir temp_dir;
  std::string root = temp_dir.path();

  std::string parent1 = root + "/parent1";
  std::string parent2 = root + "/parent2";
  ASSERT_THAT(mkdir(parent1.c_str(), 0755), SyscallSucceeds());
  ASSERT_THAT(mkdir(parent2.c_str(), 0755), SyscallSucceeds());

  struct stat st = {};
  ASSERT_THAT(stat(parent1.c_str(), &st), SyscallSucceeds());
  nlink_t p1_initial = st.st_nlink;
  ASSERT_THAT(stat(parent2.c_str(), &st), SyscallSucceeds());
  nlink_t p2_initial = st.st_nlink;

  // 1. Move directory across parents to a new name.
  // parent1/dirA -> parent2/dirA
  std::string dir_a = parent1 + "/dirA";
  ASSERT_THAT(mkdir(dir_a.c_str(), 0755), SyscallSucceeds());
  ASSERT_THAT(stat(parent1.c_str(), &st), SyscallSucceeds());
  EXPECT_EQ(st.st_nlink, p1_initial + 1);

  std::string p2_dir_a = parent2 + "/dirA";
  ASSERT_THAT(rename(dir_a.c_str(), p2_dir_a.c_str()), SyscallSucceeds());
  ASSERT_THAT(stat(parent1.c_str(), &st), SyscallSucceeds());
  EXPECT_EQ(st.st_nlink, p1_initial);
  ASSERT_THAT(stat(parent2.c_str(), &st), SyscallSucceeds());
  EXPECT_EQ(st.st_nlink, p2_initial + 1);

  // 2. Move directory across parents replacing an existing empty directory.
  // parent1/dirB -> parent2/dirA
  std::string dir_b = parent1 + "/dirB";
  ASSERT_THAT(mkdir(dir_b.c_str(), 0755), SyscallSucceeds());
  ASSERT_THAT(stat(parent1.c_str(), &st), SyscallSucceeds());
  EXPECT_EQ(st.st_nlink, p1_initial + 1);

  ASSERT_THAT(rename(dir_b.c_str(), p2_dir_a.c_str()), SyscallSucceeds());
  ASSERT_THAT(stat(parent1.c_str(), &st), SyscallSucceeds());
  EXPECT_EQ(st.st_nlink, p1_initial);
  ASSERT_THAT(stat(parent2.c_str(), &st), SyscallSucceeds());
  EXPECT_EQ(st.st_nlink, p2_initial + 1);

  // 3. Rename directory within the same parent to a new name.
  // parent2/dirA -> parent2/dirC
  std::string p2_dir_c = parent2 + "/dirC";
  ASSERT_THAT(rename(p2_dir_a.c_str(), p2_dir_c.c_str()), SyscallSucceeds());
  ASSERT_THAT(stat(parent2.c_str(), &st), SyscallSucceeds());
  EXPECT_EQ(st.st_nlink, p2_initial + 1);

  // 4. Rename directory within the same parent replacing an existing empty directory.
  // parent2/dirD -> parent2/dirC
  std::string p2_dir_d = parent2 + "/dirD";
  ASSERT_THAT(mkdir(p2_dir_d.c_str(), 0755), SyscallSucceeds());
  ASSERT_THAT(stat(parent2.c_str(), &st), SyscallSucceeds());
  EXPECT_EQ(st.st_nlink, p2_initial + 2);

  ASSERT_THAT(rename(p2_dir_d.c_str(), p2_dir_c.c_str()), SyscallSucceeds());
  ASSERT_THAT(stat(parent2.c_str(), &st), SyscallSucceeds());
  EXPECT_EQ(st.st_nlink, p2_initial + 1);
}

// This test passes non-null arguments and has other quirks that fail under sanitizers.
#if (!__has_feature(address_sanitizer) && !defined(__arm__))
TEST(FsTest, DevZeroAndNullQuirks) {
  // TODO(https://fxbug.dev/317285180) these fail on hosts > kernel 6.1 (e.g. Debian 13+)
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
  }

  size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGESIZE));

  for (const auto path : {"/dev/zero", "/dev/null"}) {
    SCOPED_TRACE(path);
    int fd = open(path, O_RDWR);

    // Attempting to write with an invalid buffer pointer still successfully "writes" the specified
    // number of bytes.
    EXPECT_EQ(write(fd, nullptr, page_size), static_cast<ssize_t>(page_size));

    // write will report success up to the maximum number of bytes.
    ssize_t max_rw_count = 0x8000'0000 - page_size;
    EXPECT_EQ(write(fd, nullptr, max_rw_count), max_rw_count);

    // Attempting to write more than this reports a short write.
    EXPECT_EQ(write(fd, nullptr, max_rw_count + 1), max_rw_count);

    // Producing a range that goes outside the userspace accessible range does produce EFAULT.
    ssize_t implausibly_large_len = (1ll << 48);

    EXPECT_EQ(write(fd, nullptr, implausibly_large_len), -1);
    EXPECT_EQ(errno, EFAULT);

    // A pointer unlikely to be backed by real memory is successful.
    void *plausible_pointer = reinterpret_cast<void *>(1ll << 30);
    EXPECT_EQ(write(fd, plausible_pointer, 1), 1);

    // An implausible pointer is unsuccessful.
    void *implausible_pointer = reinterpret_cast<void *>(implausibly_large_len);
    EXPECT_EQ(write(fd, implausible_pointer, 1), -1);
    EXPECT_EQ(errno, EFAULT);

    // Passing an invalid iov pointer produces EFAULT.
    EXPECT_EQ(writev(fd, nullptr, 1), -1);
    EXPECT_EQ(errno, EFAULT);

    struct iovec iov_null_base_valid_length[] = {{
        .iov_base = nullptr,
        .iov_len = 1,
    }};

    // Passing a valid iov pointer with null base pointers "successfully" writes the number of bytes
    // specified in the entry.
    EXPECT_EQ(writev(fd, iov_null_base_valid_length, 1), 1);

    struct iovec iov_null_base_max_rw_count_length[] = {{
        .iov_base = nullptr,
        .iov_len = static_cast<size_t>(max_rw_count),
    }};
    EXPECT_EQ(writev(fd, iov_null_base_max_rw_count_length, 1), max_rw_count);

    struct iovec iov_null_base_max_rw_count_in_two_entries[] = {
        {
            .iov_base = nullptr,
            .iov_len = static_cast<size_t>(max_rw_count - 100),
        },
        {
            .iov_base = nullptr,
            .iov_len = 100,
        },
    };
    EXPECT_EQ(writev(fd, iov_null_base_max_rw_count_in_two_entries, 2), max_rw_count);

    struct iovec iov_null_base_max_rwcount_length_plus_one[] = {{
        .iov_base = nullptr,
        .iov_len = static_cast<size_t>(max_rw_count + 1),
    }};
    EXPECT_EQ(writev(fd, iov_null_base_max_rwcount_length_plus_one, 1), max_rw_count);

    struct iovec iov_null_base_max_rwcount_length_plus_one_in_two_entries[] = {
        {
            .iov_base = nullptr,
            .iov_len = static_cast<size_t>(max_rw_count - 100),
        },
        {
            .iov_base = nullptr,
            .iov_len = 101,
        },
    };
    EXPECT_EQ(writev(fd, iov_null_base_max_rwcount_length_plus_one_in_two_entries, 2),
              max_rw_count);

    // Implausibly large iov_len values still generate EFAULT.
    struct iovec iov_null_base_implausible_length[] = {{
        .iov_base = nullptr,
        .iov_len = static_cast<size_t>(implausibly_large_len),
    }};
    EXPECT_EQ(writev(fd, iov_null_base_implausible_length, 1), -1);
    EXPECT_EQ(errno, EFAULT);

    struct iovec iov_null_base_implausible_length_behind_max_rw_count[] = {
        {
            .iov_base = nullptr,
            .iov_len = static_cast<size_t>(max_rw_count),
        },
        {
            .iov_base = nullptr,
            .iov_len = static_cast<size_t>(implausibly_large_len),
        },
    };

    EXPECT_EQ(writev(fd, iov_null_base_implausible_length_behind_max_rw_count, 2), -1);
    EXPECT_EQ(errno, EFAULT);

    if (std::string(path) == "/dev/null") {
      // Reading any plausible number of bytes from an invalid buffer pointer into /dev/null
      // will successfully read 0 bytes.
      EXPECT_EQ(read(fd, nullptr, 1), 0);
      EXPECT_EQ(read(fd, nullptr, max_rw_count), 0);
      EXPECT_EQ(read(fd, nullptr, max_rw_count + 1), 0);
    }

    // Reading an implausibly large number of bytes from /dev/zero or /dev/null will fail with
    // EFAULT.
    EXPECT_EQ(read(fd, nullptr, implausibly_large_len), -1);
    EXPECT_EQ(errno, EFAULT);

    close(fd);
  }
}
#endif

TEST(FsTest, CreateExistingFileInReadonlyFilesystemReturnsEEXIST) {
  // This test requires that / is readonly.
  ASSERT_EQ(mkdir("/asdfasdf", 0777), -1);
  ASSERT_EQ(errno, EROFS);

  EXPECT_EQ(mkdir("/tmp", 0777), -1);
  EXPECT_EQ(errno, EEXIST);
}

constexpr uid_t kOwnerUid = 65534;
constexpr uid_t kNonOwnerUid = 65533;
constexpr gid_t kOwnerGid = 65534;
constexpr gid_t kNonOwnerGid = 65533;

constexpr uid_t kUser1Uid = 65532;
constexpr uid_t kUser2Uid = 65531;
constexpr gid_t kUser1Gid = 65532;
constexpr gid_t kUser2Gid = 65531;

class UtimensatTest : public ::testing::Test {
 protected:
  void SetUp() {
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
    }

    char dir_template[] = "/tmp/XXXXXX";
    ASSERT_NE(mkdtemp(dir_template), nullptr)
        << "failed to create test folder: " << std::strerror(errno);
    test_folder_ = std::string(dir_template);

    test_file_ = test_folder_ + "/testfile";
    int fd = open(test_file_.c_str(), O_RDWR | O_CREAT, 0666);
    ASSERT_NE(fd, -1) << "failed to create test file: " << std::strerror(errno);
    close(fd);

    ASSERT_EQ(chown(test_folder_.c_str(), kOwnerUid, kOwnerGid), 0);
    ASSERT_EQ(chmod(test_folder_.c_str(), 0777), 0);
    ASSERT_EQ(chmod(test_file_.c_str(), 0666), 0);
    ASSERT_EQ(chown(test_file_.c_str(), kOwnerUid, kOwnerGid), 0);
  }

  void TearDown() {
    if (test_file_.length() != 0) {
      ASSERT_EQ(remove(test_file_.c_str()), 0);
    }
    if (test_folder_.length() != 0) {
      ASSERT_EQ(remove(test_folder_.c_str()), 0);
    }
  }

  // test folder owned by kOwnerUid, perms 0o777
  std::string test_folder_;

  // test file owned by kOwnerUid, perms 0o666
  std::string test_file_;
};

bool change_ids(uid_t user, gid_t group) {
  return (setresgid(group, group, group) == 0) && (setresuid(user, user, user) == 0);
}

TEST_F(UtimensatTest, OwnerCanAlwaysSetTime) {
  ASSERT_EQ(chmod(test_file_.c_str(), 0), 0);

  // File owner can change time to now even without write perms.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([this] {
    ASSERT_TRUE(change_ids(kOwnerUid, kOwnerGid));
    EXPECT_EQ(0, utimensat(-1, test_file_.c_str(), nullptr, 0))
        << "utimensat failed: " << std::strerror(errno);
  });

  EXPECT_TRUE(helper.WaitForChildren());

  // File owner can change time to any time without write perms.
  helper.RunInForkedProcess([this] {
    ASSERT_TRUE(change_ids(kOwnerUid, kOwnerGid));
    struct timespec times[2] = {{0, 0}};
    EXPECT_EQ(0, utimensat(-1, test_file_.c_str(), times, 0))
        << "utimensat failed: " << std::strerror(errno);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(UtimensatTest, NonOwnerWithWriteAccessCanOnlySetTimeToNow) {
  ASSERT_EQ(chmod(test_file_.c_str(), 0), 0);

  // Non file owner cannot change time to now without write perms.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([this] {
    ASSERT_TRUE(change_ids(kNonOwnerUid, kNonOwnerGid));
    EXPECT_NE(0, utimensat(-1, test_file_.c_str(), nullptr, 0));
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Non file owner can change time to now with write perms.
  ASSERT_EQ(chmod(test_file_.c_str(), 0006), 0);
  helper.RunInForkedProcess([this] {
    ASSERT_TRUE(change_ids(kNonOwnerUid, kNonOwnerGid));
    EXPECT_EQ(0, utimensat(-1, test_file_.c_str(), nullptr, 0))
        << "utimensat failed: " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Non file owner cannot change time to some other value, even with write
  // perms.
  helper.RunInForkedProcess([this] {
    ASSERT_TRUE(change_ids(kNonOwnerUid, kNonOwnerGid));
    struct timespec times[2] = {{0, 0}};
    EXPECT_NE(0, utimensat(-1, test_file_.c_str(), times, 0));
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(UtimensatTest, NonOwnerWithCapabilitiesCanSetTime) {
  ASSERT_EQ(chmod(test_file_.c_str(), 0), 0);

  // Non file owner without write permissions can set the time to now with
  // either CAP_DAC_OVERRIDE or CAP_FOWNER capability.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([this] {
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_OVERRIDE));
    ASSERT_TRUE(test_helper::HasCapability(CAP_FOWNER));
    EXPECT_EQ(0, utimensat(-1, test_file_.c_str(), nullptr, 0))
        << "utimensat failed: " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());

  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    ASSERT_FALSE(test_helper::HasCapability(CAP_DAC_OVERRIDE));
    ASSERT_TRUE(test_helper::HasCapability(CAP_FOWNER));
    EXPECT_EQ(0, utimensat(-1, test_file_.c_str(), nullptr, 0))
        << "utimensat failed: " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());

  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_FOWNER);
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_OVERRIDE));
    ASSERT_FALSE(test_helper::HasCapability(CAP_FOWNER));
    EXPECT_EQ(0, utimensat(-1, test_file_.c_str(), nullptr, 0))
        << "utimensat failed: " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());

  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    test_helper::UnsetCapability(CAP_FOWNER);
    ASSERT_FALSE(test_helper::HasCapability(CAP_DAC_OVERRIDE));
    ASSERT_FALSE(test_helper::HasCapability(CAP_FOWNER));
    EXPECT_NE(0, utimensat(-1, test_file_.c_str(), nullptr, 0));
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Non file owner without write permissions can set the time to some other
  // value with the CAP_FOWNER capability.
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    ASSERT_FALSE(test_helper::HasCapability(CAP_DAC_OVERRIDE));
    ASSERT_TRUE(test_helper::HasCapability(CAP_FOWNER));
    struct timespec times[2] = {{0, 0}};
    EXPECT_EQ(0, utimensat(-1, test_file_.c_str(), times, 0))
        << "utimensat failed: " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());

  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    test_helper::UnsetCapability(CAP_FOWNER);
    ASSERT_FALSE(test_helper::HasCapability(CAP_DAC_OVERRIDE));
    ASSERT_FALSE(test_helper::HasCapability(CAP_FOWNER));
    struct timespec times[2] = {{0, 0}};
    EXPECT_NE(0, utimensat(-1, test_file_.c_str(), times, 0));
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(UtimensatTest, CanSetOmitTimestampsWithoutPermissions) {
  // Non file owner without write permissions and without the CAP_DAC_OVERRIDE or
  // CAP_FOWNER capability can set the timestamps to UTIME_OMIT.
  ASSERT_EQ(chmod(test_file_.c_str(), 0), 0);
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    test_helper::UnsetCapability(CAP_FOWNER);
    ASSERT_FALSE(test_helper::HasCapability(CAP_DAC_OVERRIDE));
    ASSERT_FALSE(test_helper::HasCapability(CAP_FOWNER));
    struct timespec times[2] = {{0, UTIME_OMIT}, {0, UTIME_OMIT}};
    EXPECT_EQ(0, utimensat(-1, test_file_.c_str(), times, 0))
        << "utimensat failed: " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(UtimensatTest, ReturnsEFAULTOnNullPathAndCWDDirFd) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    struct timespec times[2] = {{0, 0}};
    EXPECT_NE(0, syscall(SYS_utimensat, AT_FDCWD, nullptr, times, 0));
    EXPECT_EQ(errno, EFAULT);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(UtimensatTest, ReturnsENOENTOnEmptyPath) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    EXPECT_NE(0, utimensat(-1, "", nullptr, 0));
    EXPECT_EQ(errno, ENOENT);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

class CapDacTest : public ::testing::Test {
 protected:
  void SetUp() {
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
    }

    char dir_template[] = "/tmp/XXXXXX";
    ASSERT_NE(mkdtemp(dir_template), nullptr)
        << "failed to create test folder: " << std::strerror(errno);
    test_folder_ = std::string(dir_template);

    test_file_ = test_folder_ + "/testfile";
    int fd = open(test_file_.c_str(), O_RDWR | O_CREAT, 0666);
    ASSERT_NE(fd, -1) << "failed to create test file: " << std::strerror(errno);
    close(fd);

    ASSERT_EQ(chown(test_folder_.c_str(), kOwnerUid, kOwnerGid), 0);
    ASSERT_EQ(chmod(test_folder_.c_str(), 0777), 0);
    ASSERT_EQ(chmod(test_file_.c_str(), 0666), 0);
    ASSERT_EQ(chown(test_file_.c_str(), kOwnerUid, kOwnerGid), 0);
  }

  void TearDown() {
    if (test_file_.length() != 0) {
      ASSERT_EQ(remove(test_file_.c_str()), 0);
    }
    if (test_folder_.length() != 0) {
      ASSERT_EQ(remove(test_folder_.c_str()), 0);
    }
  }

  // test folder owned by kOwnerUid, perms 0o777
  std::string test_folder_;

  // test file owned by kOwnerUid, perms 0o666
  std::string test_file_;
};

TEST_F(CapDacTest, NonOwnerCanReadAndTraverseDirectoryWithDacOverrideOrReadSearch) {
  ASSERT_EQ(chmod(test_folder_.c_str(), 0), 0);

  // Unreadable directory is unreadable without CAP_DAC_* capabilities.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    test_helper::UnsetCapability(CAP_DAC_READ_SEARCH);

    fbl::unique_fd fd(open(test_folder_.c_str(), O_RDONLY));
    EXPECT_FALSE(fd.is_valid()) << "open(O_RDONLY)";

    fd.reset(open(test_file_.c_str(), O_RDONLY));
    EXPECT_FALSE(fd.is_valid()) << "open(O_RDONLY)";
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Unreadable directory can be read and traversed with only CAP_DAC_READ_SEARCH.
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_READ_SEARCH));

    fbl::unique_fd fd(open(test_folder_.c_str(), O_RDONLY));
    EXPECT_TRUE(fd.is_valid()) << "open(O_RDONLY): " << std::strerror(errno);
    fd.reset(open(test_folder_.c_str(), O_RDWR));
    EXPECT_FALSE(fd.is_valid()) << "open(O_RDWR)";

    fd.reset(open(test_file_.c_str(), O_RDONLY));
    EXPECT_TRUE(fd.is_valid()) << "open(O_RDONLY): " << std::strerror(errno);
    // The file can also be written, since the caller still has write permission to it.
    fd.reset(open(test_file_.c_str(), O_RDWR));
    EXPECT_TRUE(fd.is_valid()) << "open(O_RDWR): " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Unreadable directory can be read and traversed with only CAP_DAC_OVERRIDE.
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_READ_SEARCH);
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_OVERRIDE));

    fbl::unique_fd fd(open(test_folder_.c_str(), O_RDONLY));
    EXPECT_TRUE(fd.is_valid()) << "open(O_RDONLY): " << std::strerror(errno);
    fd.reset(open(test_folder_.c_str(), O_RDWR));
    EXPECT_FALSE(fd.is_valid()) << "open(O_RDWR)";

    fd.reset(open(test_file_.c_str(), O_RDONLY));
    EXPECT_TRUE(fd.is_valid()) << "open(O_RDONLY): " << std::strerror(errno);
    // The file can also be written, since the caller still has write permission to it.
    fd.reset(open(test_file_.c_str(), O_RDWR));
    EXPECT_TRUE(fd.is_valid()) << "open(O_RDWR): " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(CapDacTest, NonOwnerCanWriteDirectoryWithDacOverride) {
  ASSERT_EQ(chmod(test_folder_.c_str(), 0), 0);

  // Unwritable directory is unwritable without CAP_DAC_OVERRIDE capability.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    test_helper::UnsetCapability(CAP_DAC_READ_SEARCH);
    std::string write_test_file = test_folder_ + "/testfile_without_dac_caps";
    fbl::unique_fd fd(open(write_test_file.c_str(), O_RDWR | O_CREAT, 0666));
    EXPECT_FALSE(fd.is_valid()) << "open(O_RDWR|O_CREAT) inside dir";
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Unwritable directory cannot be written with only CAP_DAC_READ_SEARCH.
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_READ_SEARCH));
    std::string write_test_file = test_folder_ + "/testfile_with_dac_read_search";
    fbl::unique_fd fd(open(write_test_file.c_str(), O_RDWR | O_CREAT, 0666));
    EXPECT_FALSE(fd.is_valid()) << "open(O_RDWR|O_CREAT) inside dir";
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Unwritable directory can be written with only CAP_DAC_OVERRIDE.
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_READ_SEARCH);
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_OVERRIDE));

    std::string write_test_file = test_folder_ + "/testfile_with_dac_override";
    fbl::unique_fd fd(open(write_test_file.c_str(), O_RDWR | O_CREAT, 0666));
    EXPECT_TRUE(fd.is_valid()) << "open(O_RDWR|O_CREAT) inside dir: " << std::strerror(errno);
    ASSERT_EQ(remove(write_test_file.c_str()), 0) << "remove() test file: " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(CapDacTest, NonOwnerCanReadFileWithDacOverrideOrReadSearch) {
  ASSERT_EQ(chmod(test_file_.c_str(), 0), 0);

  // Unreadable file is unreadable without CAP_DAC_* capabilities.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    test_helper::UnsetCapability(CAP_DAC_READ_SEARCH);
    fbl::unique_fd fd(open(test_file_.c_str(), O_RDONLY));
    EXPECT_FALSE(fd.is_valid()) << "open(O_RDONLY)";
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Unreadable file can be read with only CAP_DAC_READ_SEARCH.
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_READ_SEARCH));
    fbl::unique_fd fd(open(test_file_.c_str(), O_RDONLY));
    EXPECT_TRUE(fd.is_valid()) << "open(O_RDONLY): " << std::strerror(errno);
    fd.reset(open(test_file_.c_str(), O_RDWR));
    EXPECT_FALSE(fd.is_valid()) << "open(O_RDWR)";
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Unreadable file can be read with only CAP_DAC_OVERRIDE.
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_READ_SEARCH);
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_OVERRIDE));
    fbl::unique_fd fd(open(test_file_.c_str(), O_RDONLY));
    EXPECT_TRUE(fd.is_valid()) << "open(O_RDONLY): " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(CapDacTest, NonOwnerCanWriteFileWithDacOverride) {
  ASSERT_EQ(chmod(test_file_.c_str(), 0), 0);

  // Unwritable file is unwritable without CAP_DAC_OVERRIDE capability.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    fbl::unique_fd fd(open(test_file_.c_str(), O_WRONLY));
    EXPECT_FALSE(fd.is_valid()) << "open(O_WRONLY)";
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Unwritable file cannot be written with only CAP_DAC_READ_SEARCH.
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_OVERRIDE);
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_READ_SEARCH));
    fbl::unique_fd fd(open(test_file_.c_str(), O_WRONLY));
    EXPECT_FALSE(fd.is_valid()) << "open(O_WRONLY)";
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Unwritable file can be written with only CAP_DAC_OVERRIDE.
  helper.RunInForkedProcess([this] {
    test_helper::UnsetCapability(CAP_DAC_READ_SEARCH);
    ASSERT_TRUE(test_helper::HasCapability(CAP_DAC_OVERRIDE));
    fbl::unique_fd fd(open(test_file_.c_str(), O_WRONLY));
    EXPECT_TRUE(fd.is_valid()) << "open(O_WRONLY): " << std::strerror(errno);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

class AccessTest : public ::testing::Test {
 protected:
  void SetUp() {
    if (getuid() != 0) {
      GTEST_SKIP() << "Not running as root, skipping.";
    }

    ASSERT_THAT(chmod(test_folder_.path().c_str(), 0777), SyscallSucceeds());

    ASSERT_TRUE(CreateFile("only_user", 0700, kOwnerUid, kOwnerGid, only_user_file_));
    ASSERT_TRUE(CreateFile("everyone", 0777, kOwnerUid, kOwnerGid, everyone_file_));
  }

  bool CreateFile(const char *name, int mode, uid_t uid, gid_t gid, std::string &path) {
    path = test_folder_.path() + "/" + name;
    auto fd = fbl::unique_fd(open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC, mode));
    return fd.is_valid() && (chown(path.c_str(), uid, gid) == 0);
  }

  // World readable & writable test folder.
  test_helper::ScopedTempDir test_folder_;

  // File owned by kOwnerUid/Gid with permissions only granting owning user access.
  std::string only_user_file_;

  // File owned by kOwnerUid/Gid with permissions only granting everyone access.
  std::string everyone_file_;
};

TEST_F(AccessTest, ChecksAgainstRealCredsNonOwner) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresgid(kNonOwnerGid, kOwnerGid, 0), SyscallSucceeds());
    ASSERT_THAT(setresuid(kNonOwnerUid, kOwnerUid, 0), SyscallSucceeds());

    EXPECT_THAT(access(only_user_file_.c_str(), R_OK), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(access(everyone_file_.c_str(), R_OK), SyscallSucceeds());
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(AccessTest, ChecksAgainstRealCredsOwner) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresgid(kOwnerGid, kNonOwnerGid, 0), SyscallSucceeds());
    ASSERT_THAT(setresuid(kOwnerUid, kNonOwnerUid, 0), SyscallSucceeds());

    EXPECT_THAT(access(only_user_file_.c_str(), R_OK), SyscallSucceeds());
    EXPECT_THAT(access(everyone_file_.c_str(), R_OK), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(AccessTest, X_OK) {
  test_helper::ForkHelper helper;

  std::string user_exec_file;
  ASSERT_TRUE(CreateFile("user_exec", 0100, kOwnerUid, kOwnerGid, user_exec_file));

  std::string everyone_exec_file;
  ASSERT_TRUE(CreateFile("everyone_exec", 0111, kOwnerUid, kOwnerGid, everyone_exec_file));

  std::string no_exec_file;
  ASSERT_TRUE(CreateFile("no_exec", 0600, kOwnerUid, kOwnerGid, no_exec_file));

  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresgid(kNonOwnerGid, kNonOwnerGid, 0), SyscallSucceeds());
    ASSERT_THAT(setresuid(kNonOwnerUid, kNonOwnerUid, 0), SyscallSucceeds());

    EXPECT_THAT(access(user_exec_file.c_str(), X_OK), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(access(everyone_exec_file.c_str(), X_OK), SyscallSucceeds());
    EXPECT_THAT(access(no_exec_file.c_str(), X_OK), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());

  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresgid(kOwnerGid, kOwnerGid, 0), SyscallSucceeds());
    ASSERT_THAT(setresuid(kOwnerUid, kOwnerUid, 0), SyscallSucceeds());

    EXPECT_THAT(access(user_exec_file.c_str(), X_OK), SyscallSucceeds());
    EXPECT_THAT(access(everyone_exec_file.c_str(), X_OK), SyscallSucceeds());
    EXPECT_THAT(access(no_exec_file.c_str(), X_OK), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());

  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresgid(0, 0, 0), SyscallSucceeds());
    ASSERT_THAT(setresuid(0, 0, 0), SyscallSucceeds());

    EXPECT_THAT(access(user_exec_file.c_str(), X_OK), SyscallSucceeds());
    EXPECT_THAT(access(everyone_exec_file.c_str(), X_OK), SyscallSucceeds());
    EXPECT_THAT(access(no_exec_file.c_str(), X_OK), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());

  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresuid(0, kNonOwnerUid, 0), SyscallSucceeds());
    test_helper::SetCapabilityEffective(CAP_DAC_OVERRIDE);

    EXPECT_THAT(faccessat(AT_FDCWD, user_exec_file.c_str(), X_OK, AT_EACCESS), SyscallSucceeds());
    EXPECT_THAT(faccessat(AT_FDCWD, everyone_exec_file.c_str(), X_OK, AT_EACCESS),
                SyscallSucceeds());
    EXPECT_THAT(faccessat(AT_FDCWD, no_exec_file.c_str(), X_OK, AT_EACCESS),
                SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());

  EXPECT_EQ(unlink(user_exec_file.c_str()), 0);
  EXPECT_EQ(unlink(everyone_exec_file.c_str()), 0);
  EXPECT_EQ(unlink(no_exec_file.c_str()), 0);
}

TEST_F(AccessTest, EaccessChecksAgainstEffectiveCredsNonOwner) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresgid(kOwnerGid, kNonOwnerGid, 0), SyscallSucceeds());
    ASSERT_THAT(setresuid(kOwnerUid, kNonOwnerUid, 0), SyscallSucceeds());

    EXPECT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), R_OK, AT_EACCESS),
                SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(faccessat(AT_FDCWD, everyone_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(AccessTest, EaccessChecksAgainstEffectiveCredsOwner) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresgid(kNonOwnerGid, kOwnerGid, 0), SyscallSucceeds());
    ASSERT_THAT(setresuid(kNonOwnerUid, kOwnerUid, 0), SyscallSucceeds());

    EXPECT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());
    EXPECT_THAT(faccessat(AT_FDCWD, everyone_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(AccessTest, FsUidIgnoredUnlessEaccess) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresgid(kNonOwnerGid, kNonOwnerGid, kOwnerUid), SyscallSucceeds());
    ASSERT_THAT(setresuid(kNonOwnerUid, kNonOwnerUid, kOwnerUid), SyscallSucceeds());

    ASSERT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), R_OK, AT_EACCESS),
                SyscallFailsWithErrno(EACCES));
    ASSERT_THAT(faccessat(AT_FDCWD, everyone_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());

    ASSERT_EQ(setfsuid(kOwnerUid), static_cast<int>(kNonOwnerUid));
    ASSERT_EQ(setfsuid(-1), static_cast<int>(kOwnerUid));

    // Even though the "fsuid" is the owning UID, access to the only-user access file is denied.
    EXPECT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());
    EXPECT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), R_OK, 0),
                SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(access(only_user_file_.c_str(), R_OK), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(faccessat(AT_FDCWD, everyone_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());

  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresgid(kOwnerGid, kOwnerGid, kNonOwnerUid), SyscallSucceeds());
    ASSERT_THAT(setresuid(kOwnerUid, kOwnerUid, kNonOwnerUid), SyscallSucceeds());

    ASSERT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());
    ASSERT_THAT(faccessat(AT_FDCWD, everyone_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());

    ASSERT_EQ(setfsuid(kNonOwnerUid), static_cast<int>(kOwnerUid));
    ASSERT_EQ(setfsuid(-1), static_cast<int>(kNonOwnerUid));

    // Even though the "fsuid" is the owning UID, access to the only-user access file is denied.
    EXPECT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), R_OK, AT_EACCESS),
                SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), R_OK, 0), SyscallSucceeds());
    EXPECT_THAT(access(only_user_file_.c_str(), R_OK), SyscallSucceeds());
    EXPECT_THAT(faccessat(AT_FDCWD, everyone_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

// access() for root users should use permitted capabilities instead of effective capabilities.
TEST_F(AccessTest, RootAccessCheckUsesPermittedCaps) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_EQ(geteuid(), 0u);
    if (!test_helper::HasCapabilityPermitted(CAP_DAC_OVERRIDE)) {
      GTEST_SKIP() << "Missing CAP_DAC_OVERRIDE in permitted set, skipping.";
    }
    test_helper::UnsetCapabilityEffective(CAP_DAC_OVERRIDE);
    ASSERT_FALSE(test_helper::HasCapabilityEffective(CAP_DAC_OVERRIDE));
    EXPECT_THAT(access(only_user_file_.c_str(), W_OK), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

// faccessat() with AT_EACCESS for root users should check against the effective capability set.
TEST_F(AccessTest, RootAccessCheckWithAtEaccessUsesEffectiveCaps) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_EQ(geteuid(), 0u);
    if (!test_helper::HasCapabilityPermitted(CAP_DAC_OVERRIDE)) {
      GTEST_SKIP() << "Missing CAP_DAC_OVERRIDE in permitted set, skipping.";
    }
    test_helper::UnsetCapabilityEffective(CAP_DAC_OVERRIDE);
    ASSERT_FALSE(test_helper::HasCapabilityEffective(CAP_DAC_OVERRIDE));
    EXPECT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), W_OK, AT_EACCESS),
                SyscallFailsWithErrno(EACCES));
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

// access() for non-root users should use an empty set of capabilities.
TEST_F(AccessTest, NonRootAccessCheckUsesEmptyCaps) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresuid(kNonOwnerUid, 0, 0), SyscallSucceeds());
    test_helper::SetCapabilityEffective(CAP_DAC_OVERRIDE);
    ASSERT_TRUE(test_helper::HasCapabilityEffective(CAP_DAC_OVERRIDE));
    EXPECT_THAT(access(only_user_file_.c_str(), R_OK), SyscallFailsWithErrno(EACCES));
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

// faccessat() with AT_EACCESS for non-root users should check against the effective capability set.
TEST_F(AccessTest, NonRootAccessCheckWithAtEaccessUsesEffectiveCaps) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_THAT(setresuid(0, kNonOwnerUid, 0), SyscallSucceeds());
    test_helper::SetCapabilityEffective(CAP_DAC_OVERRIDE);
    ASSERT_TRUE(test_helper::HasCapabilityEffective(CAP_DAC_OVERRIDE));
    EXPECT_THAT(faccessat(AT_FDCWD, only_user_file_.c_str(), R_OK, AT_EACCESS), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

std::optional<std::string> MountOverlayFs(const std::string &temp_dir) {
  EXPECT_FALSE(temp_dir.empty());

  std::string overlay = temp_dir + "/overlay";
  EXPECT_THAT(mkdir(overlay.c_str(), S_IRWXU), SyscallSucceeds());

  std::string lower = temp_dir + "/lower";
  EXPECT_THAT(mkdir(lower.c_str(), S_IRWXU), SyscallSucceeds());

  std::string upper = temp_dir + "/upper";
  EXPECT_THAT(mkdir(upper.c_str(), S_IRWXU), SyscallSucceeds());

  std::string work = temp_dir + "/work";
  EXPECT_THAT(mkdir(work.c_str(), S_IRWXU), SyscallSucceeds());

  std::string options = fxl::StringPrintf("lowerdir=%s,upperdir=%s,workdir=%s", lower.c_str(),
                                          upper.c_str(), work.c_str());

  int res = mount(nullptr, overlay.c_str(), "overlay", 0, options.c_str());
  EXPECT_EQ(res, 0) << "mount: " << std::strerror(errno);

  if (res != 0) {
    return std::nullopt;
  }

  return overlay;
}

std::optional<std::string> MountTmpFs(const std::string &temp_dir) {
  std::string temp = temp_dir + "/tmp";
  EXPECT_THAT(mkdir(temp.c_str(), S_IRWXU), SyscallSucceeds());

  int res = mount(nullptr, temp.c_str(), "tmpfs", 0, "");
  EXPECT_EQ(res, 0) << "mount: " << std::strerror(errno);

  if (res != 0) {
    return std::nullopt;
  }

  return temp;
}

class FsMountTest
    : public testing::TestWithParam<std::optional<std::string> (*)(const std::string &)> {
 protected:
  void SetUp() override {
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
    }
    auto mounter = GetParam();
    auto mounted = mounter(temp_dir_.path());
    ASSERT_TRUE(mounted.has_value()) << "failed to mount fs";
    mount_path_ = mounted.value();

    // Directory Permissions: owner can do everything, user and other can search.
    constexpr int kDirPerms = S_IRWXU | S_IXGRP | S_IXOTH;

    ASSERT_THAT(chmod(mount_path_.c_str(), kDirPerms), SyscallSucceeds());
    ASSERT_THAT(chmod(temp_dir_.path().c_str(), kDirPerms), SyscallSucceeds());
  }

  test_helper::ScopedTempDir temp_dir_;
  std::string mount_path_;
};

INSTANTIATE_TEST_SUITE_P(TmpFs, FsMountTest, ::testing::Values(MountTmpFs));
INSTANTIATE_TEST_SUITE_P(OverlayFs, FsMountTest, ::testing::Values(MountOverlayFs));

TEST_P(FsMountTest, CantBypassDirectoryPermissions) {
  std::string user1_folder = mount_path_ + "/user1";
  ASSERT_THAT(mkdir(user1_folder.c_str(), S_IRWXU), SyscallSucceeds());
  ASSERT_THAT(chown(user1_folder.c_str(), kUser1Uid, kUser1Gid), SyscallSucceeds());

  std::string user2_folder = mount_path_ + "/user2";
  ASSERT_THAT(mkdir(user2_folder.c_str(), S_IRWXU), SyscallSucceeds());
  ASSERT_THAT(chown(user2_folder.c_str(), kUser2Uid, kUser2Gid), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_TRUE(change_ids(kUser2Uid, kUser2Gid));
    test_helper::DropAllCapabilities();

    // We should be able to create files in user2's directory.
    std::string file_path = user2_folder + "/test_file";
    int fd = open(file_path.c_str(), O_RDWR | O_CREAT | O_EXCL, S_IRUSR | S_IWUSR);
    EXPECT_NE(fd, -1) << "open " << file_path << ": " << std::strerror(errno);
    if (fd != -1) {
      close(fd);
      EXPECT_EQ(unlink(file_path.c_str()), 0);
    }

    // We shouldn't be able to create files in user1's directory.
    file_path = user1_folder + "/test_file";
    fd = open(file_path.c_str(), O_RDWR | O_CREAT | O_EXCL, S_IRUSR | S_IWUSR);
    EXPECT_EQ(fd, -1);
    EXPECT_EQ(errno, EACCES);
    if (fd != -1) {
      close(fd);
      EXPECT_EQ(unlink(file_path.c_str()), 0);
    }
  });
}

TEST_P(FsMountTest, CreateWithDifferentModes) {
  std::string user1_folder = mount_path_ + "/user1";
  ASSERT_THAT(mkdir(user1_folder.c_str(), S_IRWXU), SyscallSucceeds());
  ASSERT_THAT(chown(user1_folder.c_str(), kUser1Uid, kUser1Gid), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([user1_folder] {
    ASSERT_TRUE(change_ids(kUser1Uid, kUser1Gid));
    test_helper::DropAllCapabilities();

    const mode_t old_umask = umask(0);
    constexpr mode_t kModeMask = 0777;
    auto clean_umask = fit::defer([old_umask]() { umask(old_umask); });

    for (mode_t mode = 0000; mode <= 0777; mode++) {
      SCOPED_TRACE(fxl::StringPrintf("Mode: %o", mode));

      std::string file_path = fxl::StringPrintf("%s/create.%o", user1_folder.c_str(), mode);
      {
        fbl::unique_fd fd(open(file_path.c_str(), O_RDWR | O_CREAT | O_EXCL, mode));
        EXPECT_TRUE(fd.is_valid()) << "open: " << std::strerror(errno);
      }

      auto cleanup =
          fit::defer([file_path]() { EXPECT_THAT(unlink(file_path.c_str()), SyscallSucceeds()); });

      struct stat file_stat;
      EXPECT_THAT(stat(file_path.c_str(), &file_stat), SyscallSucceeds());
      EXPECT_TRUE(file_stat.st_mode & S_IFREG) << "not a regular file";
      EXPECT_EQ(file_stat.st_mode & kModeMask, mode) << "wrong permissions";
    }
  });
}

TEST_P(FsMountTest, ChmodWithDifferentModes) {
  std::string user1_folder = mount_path_ + "/user1";
  ASSERT_THAT(mkdir(user1_folder.c_str(), S_IRWXU), SyscallSucceeds());
  ASSERT_THAT(chown(user1_folder.c_str(), kUser1Uid, kUser1Gid), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([user1_folder] {
    ASSERT_TRUE(change_ids(kUser1Uid, kUser1Gid));
    test_helper::DropAllCapabilities();
    const mode_t old_umask = umask(0);
    constexpr mode_t kModeMask = 0777;
    auto clean_umask = fit::defer([old_umask]() { umask(old_umask); });

    for (mode_t mode = 0000; mode <= 0777; mode++) {
      SCOPED_TRACE(fxl::StringPrintf("Mode: %o", mode));

      std::string file_path = fxl::StringPrintf("%s/chmod.%o", user1_folder.c_str(), mode);
      {
        fbl::unique_fd fd(open(file_path.c_str(), O_RDWR | O_CREAT | O_EXCL, S_IRUSR | S_IWUSR));
        EXPECT_TRUE(fd.is_valid()) << "open: " << std::strerror(errno);
      }
      auto cleanup =
          fit::defer([file_path]() { EXPECT_THAT(unlink(file_path.c_str()), SyscallSucceeds()); });

      EXPECT_THAT(chmod(file_path.c_str(), mode), SyscallSucceeds());

      struct stat file_stat;
      EXPECT_THAT(stat(file_path.c_str(), &file_stat), SyscallSucceeds());
      EXPECT_TRUE(file_stat.st_mode & S_IFREG) << "not a regular file";
      EXPECT_EQ(file_stat.st_mode & kModeMask, mode) << "wrong permissions";
    }
  });
}

TEST_P(FsMountTest, ChownMinusOneSucceeds) {
  // Executing chown(file, -1, -1) should almost always work.
  std::string user1_file = files::JoinPath(mount_path_, "user1_file");
  close(SAFE_SYSCALL(creat(user1_file.c_str(), S_IRWXU)));
  SAFE_SYSCALL(chown(user1_file.c_str(), kUser1Uid, kUser1Gid));

  test_helper::ForkHelper helper;

  // Running as the same user.
  helper.RunInForkedProcess([user1_file] {
    ASSERT_TRUE(change_ids(kUser1Uid, kUser1Gid));
    test_helper::DropAllCapabilities();

    EXPECT_THAT(chown(user1_file.c_str(), -1, -1), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Running as a different user.
  helper.RunInForkedProcess([user1_file] {
    ASSERT_TRUE(change_ids(kUser2Uid, kUser2Gid));
    test_helper::DropAllCapabilities();

    EXPECT_THAT(chown(user1_file.c_str(), -1, -1), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());

  SAFE_SYSCALL(unlink(user1_file.c_str()));
}

TEST_P(FsMountTest, ChownMinusOneNoPathAccessFails) {
  // Executing chown(file, -1, -1) fails if we can't resolve the path.
  std::string user1_folder = files::JoinPath(mount_path_, "user1_folder");
  std::string user1_file = files::JoinPath(user1_folder, "user1_file");
  SAFE_SYSCALL(mkdir(user1_folder.c_str(), S_IRWXU));  // user2 can't access.

  SAFE_SYSCALL(chown(user1_folder.c_str(), kUser1Uid, kUser1Gid));
  close(SAFE_SYSCALL(creat(user1_file.c_str(), S_IRWXU)));
  SAFE_SYSCALL(chown(user1_file.c_str(), kUser1Uid, kUser1Gid));

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([user1_folder, user1_file] {
    ASSERT_TRUE(change_ids(kUser2Uid, kUser2Gid));
    test_helper::DropAllCapabilities();

    EXPECT_THAT(chown(user1_folder.c_str(), -1, -1), SyscallSucceeds());
    EXPECT_THAT(chown(user1_file.c_str(), -1, -1), SyscallFailsWithErrno(EACCES));
  });
  EXPECT_TRUE(helper.WaitForChildren());

  SAFE_SYSCALL(unlink(user1_file.c_str()));
}

TEST_P(FsMountTest, ChownMinusOneOnSIDFileFails) {
  // Executing chown(file, -1, -1) fails if the file is set-ID.
  std::string user1_file = files::JoinPath(mount_path_, "user1_file");
  close(SAFE_SYSCALL(creat(user1_file.c_str(), 0)));
  SAFE_SYSCALL(chown(user1_file.c_str(), kUser1Uid, kUser1Gid));

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([user1_file] {
    SAFE_SYSCALL(chmod(user1_file.c_str(), S_ISUID));
    ASSERT_TRUE(change_ids(kUser2Uid, kUser2Gid));
    test_helper::DropAllCapabilities();

    EXPECT_THAT(chown(user1_file.c_str(), -1, -1), SyscallFailsWithErrno(EPERM));
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // The file should still be set-user-ID even after failure.
  struct stat file_stat{};
  SAFE_SYSCALL(stat(user1_file.c_str(), &file_stat));
  EXPECT_NE(file_stat.st_mode & S_ISUID, 0U);

  helper.RunInForkedProcess([user1_file] {
    SAFE_SYSCALL(chmod(user1_file.c_str(), S_ISGID | S_IXGRP));
    ASSERT_TRUE(change_ids(kUser2Uid, kUser2Gid));
    test_helper::DropAllCapabilities();

    EXPECT_THAT(chown(user1_file.c_str(), -1, -1), SyscallFailsWithErrno(EPERM));
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // The file should still be set-group-ID even after failure.
  SAFE_SYSCALL(stat(user1_file.c_str(), &file_stat));
  EXPECT_EQ(file_stat.st_mode & (S_ISGID | S_IXGRP), (unsigned int)(S_ISGID | S_IXGRP));

  // But not if we are the owners.
  helper.RunInForkedProcess([user1_file] {
    ASSERT_TRUE(change_ids(kUser1Uid, kUser1Gid));
    test_helper::DropAllCapabilities();

    EXPECT_THAT(chown(user1_file.c_str(), -1, -1), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());

  // Doing a successful chown should have dropped the set-user-ID bit of the file.
  SAFE_SYSCALL(stat(user1_file.c_str(), &file_stat));
  EXPECT_EQ(file_stat.st_mode & S_ISUID, 0U);

  SAFE_SYSCALL(unlink(user1_file.c_str()));
}

TEST_P(FsMountTest, ChownSameOwnerAndGroupFails) {
  // Executing chown explicitly specifying owner and gid (instead of -1), fails
  // if we are not owners.
  std::string user1_file = files::JoinPath(mount_path_, "user1_file");
  close(SAFE_SYSCALL(creat(user1_file.c_str(), S_IRWXU)));
  SAFE_SYSCALL(chmod(user1_file.c_str(), S_IRWXU));
  SAFE_SYSCALL(chown(user1_file.c_str(), kUser1Uid, kUser1Gid));

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([user1_file] {
    ASSERT_TRUE(change_ids(kUser2Uid, kUser2Gid));
    test_helper::DropAllCapabilities();

    EXPECT_THAT(chown(user1_file.c_str(), kUser1Uid, kUser1Gid), SyscallFailsWithErrno(EPERM));
    EXPECT_THAT(chown(user1_file.c_str(), -1, kUser1Gid), SyscallFailsWithErrno(EPERM));
    EXPECT_THAT(chown(user1_file.c_str(), kUser1Uid, -1), SyscallFailsWithErrno(EPERM));
  });
  EXPECT_TRUE(helper.WaitForChildren());

  SAFE_SYSCALL(unlink(user1_file.c_str()));
}

TEST_P(FsMountTest, ChownOnSUIDFileDropsSUIDBit) {
  std::string user1_file = files::JoinPath(mount_path_, "user1_file");
  close(SAFE_SYSCALL(creat(user1_file.c_str(), 0)));
  SAFE_SYSCALL(chown(user1_file.c_str(), kUser1Uid, kUser1Gid));

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([user1_file] {
    ASSERT_TRUE(change_ids(kUser1Uid, kUser1Gid));
    test_helper::DropAllCapabilities();

    for (mode_t mode = 0000; mode <= 0777; mode++) {
      SCOPED_TRACE(fxl::StringPrintf("Mode: %o", mode));
      SAFE_SYSCALL(chmod(user1_file.c_str(), S_ISUID | mode));
      SAFE_SYSCALL(chown(user1_file.c_str(), -1, -1));

      struct stat file_stat{};
      SAFE_SYSCALL(stat(user1_file.c_str(), &file_stat));
      EXPECT_EQ(file_stat.st_mode & S_ISUID, 0U);
    }
  });

  EXPECT_TRUE(helper.WaitForChildren());
}
TEST_P(FsMountTest, ChownOnSGIDFileDropsSGIDBit) {
  std::string user1_file = files::JoinPath(mount_path_, "user1_file");
  close(SAFE_SYSCALL(creat(user1_file.c_str(), 0)));
  SAFE_SYSCALL(chown(user1_file.c_str(), kUser1Uid, kUser1Gid));

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([user1_file] {
    ASSERT_TRUE(change_ids(kUser1Uid, kUser1Gid));
    test_helper::DropAllCapabilities();

    for (mode_t mode = 0000; mode <= 0777; mode++) {
      SCOPED_TRACE(fxl::StringPrintf("Mode: %o", mode));
      SAFE_SYSCALL(chmod(user1_file.c_str(), S_ISGID | mode));
      SAFE_SYSCALL(chown(user1_file.c_str(), -1, -1));

      struct stat file_stat{};
      SAFE_SYSCALL(stat(user1_file.c_str(), &file_stat));
      if (mode & S_IXGRP) {
        // The set-group-ID bit only takes effect if the file is
        // group-executable. Otherwise it has other meaning and should not drop
        // that bit upon chown.
        EXPECT_EQ(file_stat.st_mode & S_ISGID, 0U);
      } else {
        EXPECT_NE(file_stat.st_mode & S_ISGID, 0U);
      }
    }
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_P(FsMountTest, OpenWithTruncAndCreatOnReadOnlyFsReturnsEROFS) {
  std::string lock_file = mount_path_ + "/lock";
  int fd = SAFE_SYSCALL(open(lock_file.c_str(), O_CREAT | O_RDWR, 0600));
  close(fd);

  SAFE_SYSCALL(chown(lock_file.c_str(), kUser1Uid, kUser1Gid));

  // Remount filesystem as read-only.
  SAFE_SYSCALL(
      mount(nullptr, mount_path_.c_str(), "ignored", MS_REMOUNT | MS_BIND | MS_RDONLY, ""));
  auto cleanup = fit::defer([this]() {
    SAFE_SYSCALL(mount(nullptr, mount_path_.c_str(), "ignored", MS_REMOUNT | MS_BIND, ""));
  });

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([lock_file] {
    ASSERT_TRUE(change_ids(kUser1Uid, kUser1Gid));
    test_helper::DropAllCapabilities();

    int fd = open(lock_file.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0666);
    int saved_errno = errno;
    EXPECT_EQ(fd, -1);
    EXPECT_EQ(saved_errno, EROFS) << std::strerror(saved_errno);

    if (fd != -1) {
      SAFE_SYSCALL(close(fd));
    }
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_P(FsMountTest, OpenSpecialFilesOnReadOnlyFs) {
  std::string reg_file = mount_path_ + "/reg";
  std::string fifo_file = mount_path_ + "/fifo";
  std::string dev_file = mount_path_ + "/dev";
  std::string sock_file = mount_path_ + "/sock";

  {
    // Create regular file.
    fbl::unique_fd fd(open(reg_file.c_str(), O_CREAT | O_RDWR, 0600));
    ASSERT_TRUE(fd.is_valid()) << "failed to create reg file: " << strerror(errno);

    // Create a /devnull device node (major 1, minor 3).
    ASSERT_THAT(mkfifo(fifo_file.c_str(), 0600), SyscallSucceeds());

    // Create a FIFO.
    ASSERT_THAT(mknod(dev_file.c_str(), S_IFCHR | 0600, makedev(1, 3)), SyscallSucceeds());

    // Create a Unix-domain socket.
    fbl::unique_fd sock_fd(socket(AF_UNIX, SOCK_STREAM, 0));
    ASSERT_THAT(sock_fd.get(), SyscallSucceeds());
    struct sockaddr_un addr = {.sun_family = AF_UNIX};
    strncpy(addr.sun_path, sock_file.c_str(), sizeof(addr.sun_path) - 1);
    ASSERT_THAT(bind(sock_fd.get(), (struct sockaddr *)&addr, sizeof(addr)), SyscallSucceeds());
  }

  // Remount filesystem as read-only.
  SAFE_SYSCALL(
      mount(nullptr, mount_path_.c_str(), "ignored", MS_REMOUNT | MS_BIND | MS_RDONLY, nullptr));

  // Write access to regular file should fail with EROFS.
  EXPECT_THAT(access(reg_file.c_str(), W_OK), SyscallFailsWithErrno(EROFS));
  fbl::unique_fd reg_fd(open(reg_file.c_str(), O_WRONLY));
  EXPECT_THAT(reg_fd.get(), SyscallFailsWithErrno(EROFS));

  // Write access to FIFOs should not be affected by the filesystem MS_RDONLY flag.
  EXPECT_THAT(access(fifo_file.c_str(), W_OK), SyscallSucceeds());
  fbl::unique_fd fifo_fd(open(fifo_file.c_str(), O_RDWR));
  EXPECT_THAT(fifo_fd.get(), SyscallSucceeds());

  // Write access to devices should not be affected by the filesystem MS_RDONLY flag.
  EXPECT_THAT(access(dev_file.c_str(), W_OK), SyscallSucceeds());
  fbl::unique_fd dev_fd(open(dev_file.c_str(), O_WRONLY));
  EXPECT_THAT(dev_fd.get(), SyscallSucceeds());

  // Write access to sockets should not be affected by the filesystem MS_RDONLY flag, but opening
  // the socket will fail with "no such device" because the listening end has been closed.
  EXPECT_THAT(access(sock_file.c_str(), W_OK), SyscallSucceeds());
  fbl::unique_fd sock_fd(open(sock_file.c_str(), O_WRONLY));
  EXPECT_THAT(sock_fd.get(), SyscallFailsWithErrno(ENXIO));
}

TEST_P(FsMountTest, OpenWithTruncAndCreatWithExistingFileSucceeds) {
  std::string lock_file = mount_path_ + "/lock";
  int fd = SAFE_SYSCALL(open(lock_file.c_str(), O_CREAT | O_RDWR, 0600));
  close(fd);

  SAFE_SYSCALL(chown(lock_file.c_str(), kUser1Uid, kUser1Gid));

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([lock_file] {
    ASSERT_TRUE(change_ids(kUser1Uid, kUser1Gid));
    test_helper::DropAllCapabilities();

    int fd = SAFE_SYSCALL(open(lock_file.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0600));
    SAFE_SYSCALL(close(fd));
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_P(FsMountTest, OpenWithTruncAndCreatWithNoPermsReturnsEACCES) {
  std::string lock_file = mount_path_ + "/lock";
  int fd = SAFE_SYSCALL(open(lock_file.c_str(), O_CREAT | O_RDWR, 0600));
  close(fd);

  SAFE_SYSCALL(chown(lock_file.c_str(), kUser1Uid, kUser1Gid));

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([lock_file] {
    ASSERT_TRUE(change_ids(kUser2Uid, kUser2Gid));
    test_helper::DropAllCapabilities();

    int fd = open(lock_file.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0600);
    int saved_errno = errno;
    EXPECT_EQ(fd, -1);
    EXPECT_EQ(saved_errno, EACCES) << std::strerror(saved_errno);
    if (fd != -1) {
      SAFE_SYSCALL(close(fd));
    }
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_P(FsMountTest, CreateAndRenameDirectory) {
  std::string old_name = mount_path_ + "/old";
  std::string new_name = mount_path_ + "/new";

  ASSERT_THAT(mkdir(old_name.c_str(), 0700), SyscallSucceeds());
  EXPECT_THAT(rename(old_name.c_str(), new_name.c_str()), SyscallSucceeds());
}

TEST(MknodTest, MknodChrZeroDoesNotRequireCapMknod) {
  test_helper::ScopedTempDir temp;
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([temp = temp.path()] {
    // Verify that zero-Id device nodes can be created while holding CAP_MKNOD.
    std::string chr_dev_file_control = temp + "/zero_chr_dev-control";
    ASSERT_THAT(mknod(chr_dev_file_control.c_str(), S_IFCHR | 0600, 0), SyscallSucceeds());
    std::string blk_dev_file_control = temp + "/zero_blk_dev-control";
    ASSERT_THAT(mknod(blk_dev_file_control.c_str(), S_IFBLK | 0600, 0), SyscallSucceeds());

    test_helper::DropAllCapabilities();

    // Attempting to create a normal character device node will fail without CAP_MKNOD.
    std::string null_file = temp + "/null_dev";
    EXPECT_THAT(mknod(null_file.c_str(), S_IFCHR | 0600, makedev(1, 3)),
                SyscallFailsWithErrno(EPERM));

    // The zero device-Id is never assigned, so userspace can create them without restriction.
    std::string chr_dev_file = temp + "/zero_chr_dev";
    EXPECT_THAT(mknod(chr_dev_file.c_str(), S_IFCHR | 0600, 0), SyscallSucceeds());
    std::string blk_dev_file = temp + "/zero_blk_dev";
    EXPECT_THAT(mknod(blk_dev_file.c_str(), S_IFBLK | 0600, 0), SyscallFailsWithErrno(EPERM));
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

class OtmpfileTest : public ::testing::Test {
 protected:
  void SetUp() override {
    char *dir = getenv("MUTABLE_STORAGE");
    test_folder_ = dir == nullptr ? "/tmp/XXXXXX" : std::string(dir) + "/XXXXXX";
    ASSERT_NE(mkdtemp(test_folder_.data()), nullptr)
        << "failed to create test folder: " << std::strerror(errno);

    test_file1_ = test_folder_ + "/testfile1";
    test_file2_ = test_folder_ + "/testfile2";
  }

  void TearDown() override {
    if (tmpfile_fd_ != -1) {
      tmpfile_fd_.reset();
    }
    // These files may have been created, attempt to remove them in case they were.
    remove(test_file1_.c_str());
    remove(test_file2_.c_str());
    if (test_folder_.length() != 0) {
      ASSERT_EQ(rmdir(test_folder_.c_str()), 0);
    }
  }

  fbl::unique_fd tmpfile_fd_;
  std::string test_folder_;
  std::string test_file1_;
  std::string test_file2_;
};

void CheckLinkCount(int fd, unsigned count) {
  uint64_t nlink = 0;
  struct stat s;
  if (fstat(fd, &s) == 0) {
    nlink = s.st_nlink;
  } else {
    ASSERT_EQ(errno, EOVERFLOW);
    struct stat64 s;
    ASSERT_EQ(fstat64(fd, &s), 0);
    nlink = s.st_nlink;
  }
  ASSERT_EQ(nlink, count);
}

TEST_F(OtmpfileTest, TmpFileLinkIntoAfter) {
  // CAP_DAC_READ_SEARCH capability is required to use AT_EMPTY_PATH with linkat
  if (!test_helper::HasCapability(CAP_DAC_READ_SEARCH)) {
    GTEST_SKIP() << "Not running with CAP_DAC_READ_SEARCH capabilities, skipping.";
  }
  tmpfile_fd_ = fbl::unique_fd(open(test_folder_.c_str(), O_RDWR | O_TMPFILE));
  ASSERT_TRUE(tmpfile_fd_.is_valid()) << "open() with O_TMPFILE failed:" << strerror(errno);
  ASSERT_NO_FATAL_FAILURE(CheckLinkCount(tmpfile_fd_.get(), 0));

  // Write to file. The contents are used later to verify that linkat worked.
  ASSERT_EQ(write(tmpfile_fd_.get(), "hello", 5), 5)
      << "Write to tmpfile failed:" << strerror(errno);

  // Test that we can link.
  SAFE_SYSCALL(linkat(tmpfile_fd_.get(), "", AT_FDCWD, test_file1_.c_str(), AT_EMPTY_PATH));
  ASSERT_NO_FATAL_FAILURE(CheckLinkCount(tmpfile_fd_.get(), 1));

  // Test that we can link again.
  SAFE_SYSCALL(linkat(tmpfile_fd_.get(), "", AT_FDCWD, test_file2_.c_str(), AT_EMPTY_PATH));
  ASSERT_NO_FATAL_FAILURE(CheckLinkCount(tmpfile_fd_.get(), 2));

  // Verify contents.
  fbl::unique_fd test_file_fd(open(test_file1_.c_str(), O_RDONLY));
  ASSERT_TRUE(test_file_fd.is_valid()) << "Failed to open file:" << strerror(errno);
  char buffer[10];
  ASSERT_EQ(read(test_file_fd.get(), buffer, 10), 5)
      << "Failed to read from file:" << strerror(errno);
  ASSERT_EQ(strncmp(buffer, "hello", 5), 0)
      << "Contents do not match the contents written to the tmpfile.";

  // If we try to link into a path that is already used, this should fail with EEXIST.
  int result = linkat(tmpfile_fd_.get(), "", AT_FDCWD, test_file1_.c_str(), AT_EMPTY_PATH);
  int saved_errno = errno;
  ASSERT_EQ(result, -1);
  EXPECT_EQ(saved_errno, EEXIST) << "Link to an existing path should fail with EEXIST:"
                                 << std::strerror(saved_errno);
}

TEST_F(OtmpfileTest, TmpFileWithOExclShouldFailLinkInto) {
  // CAP_DAC_READ_SEARCH capability is required to use AT_EMPTY_PATH with linkat
  if (!test_helper::HasCapability(CAP_DAC_READ_SEARCH)) {
    GTEST_SKIP() << "Not running with CAP_DAC_READ_SEARCH capabilities, skipping.";
  }

  tmpfile_fd_ = fbl::unique_fd(open(test_folder_.c_str(), O_RDWR | O_TMPFILE | O_EXCL));
  ASSERT_TRUE(tmpfile_fd_.is_valid()) << "open() with O_TMPFILE failed:" << strerror(errno);

  int result = linkat(tmpfile_fd_.get(), "", AT_FDCWD, test_file1_.c_str(), AT_EMPTY_PATH);
  int saved_errno = errno;
  ASSERT_EQ(result, -1);
  EXPECT_EQ(saved_errno, ENOENT)
      << "linkat() should fail when file was opened with O_TMPFILE | O_EXCL with ENOENT:"
      << std::strerror(saved_errno);
}

TEST_F(OtmpfileTest, TmpFileFailWithRdOnlyAccessMode) {
  tmpfile_fd_ = fbl::unique_fd(open(test_folder_.c_str(), O_RDONLY | O_TMPFILE));
  int saved_errno = errno;
  ASSERT_FALSE(tmpfile_fd_.is_valid());
  EXPECT_EQ(saved_errno, EINVAL)
      << "open() with O_TMPFILE not specified with O_RDWR and O_WRONLY should fail with EINVAL:"
      << std::strerror(saved_errno);
}

TEST_F(OtmpfileTest, TmpFileWithOCreatShouldFail) {
  tmpfile_fd_ = fbl::unique_fd(open(test_folder_.c_str(), O_RDWR | O_CREAT | O_TMPFILE));
  int saved_errno = errno;
  ASSERT_FALSE(tmpfile_fd_.is_valid());
  EXPECT_EQ(saved_errno, EINVAL)
      << "open() with O_TMPFILE and O_CREAT are not compatible. Should fail with EINVAL:"
      << std::strerror(saved_errno);
}

TEST(LinkTest, FileLinkCount) {
  // Create a temporary directory, store its absolute path and chdir to it.
  char *dir = getenv("MUTABLE_STORAGE");
  std::string test_folder =
      dir == nullptr ? "/tmp/linkcount.XXXXXX" : std::string(dir) + "/linkcount.XXXXXX";
  ASSERT_NE(mkdtemp(test_folder.data()), nullptr)
      << "failed to create test folder: " << std::strerror(errno);

  std::string test_file = test_folder + "/foo";
  fbl::unique_fd foo_fd(creat(test_file.c_str(), S_IRWXU));
  ASSERT_TRUE(foo_fd.is_valid()) << "Failed to open file:" << strerror(errno);
  ASSERT_NO_FATAL_FAILURE(CheckLinkCount(foo_fd.get(), 1));

  // Create link to the file. We should see link count increment.
  std::string bar = test_folder + "/bar";
  SAFE_SYSCALL(linkat(AT_FDCWD, test_file.c_str(), AT_FDCWD, bar.c_str(), 0));
  ASSERT_NO_FATAL_FAILURE(CheckLinkCount(foo_fd.get(), 2));

  // Unlink should decrement the link count.
  EXPECT_EQ(unlink(bar.c_str()), 0);
  ASSERT_NO_FATAL_FAILURE(CheckLinkCount(foo_fd.get(), 1));
  EXPECT_EQ(unlink(test_file.c_str()), 0);
  ASSERT_NO_FATAL_FAILURE(CheckLinkCount(foo_fd.get(), 0));

  // Clean up.
  ASSERT_EQ(rmdir(test_folder.c_str()), 0);
}

TEST(FsTest, DeepPathLookup) {
  char *dir = getenv("MUTABLE_STORAGE");
  test_helper::ScopedTempDir test_folder(dir == nullptr ? "/tmp" : dir);
  std::string base_path = test_folder.path();

  std::string deep_path = base_path + "/a/b/c/d/e";

  // Create deep directory structure
  ASSERT_EQ(mkdir((base_path + "/a").c_str(), 0777), 0);
  ASSERT_EQ(mkdir((base_path + "/a/b").c_str(), 0777), 0);
  ASSERT_EQ(mkdir((base_path + "/a/b/c").c_str(), 0777), 0);
  ASSERT_EQ(mkdir((base_path + "/a/b/c/d").c_str(), 0777), 0);
  ASSERT_EQ(mkdir((base_path + "/a/b/c/d/e").c_str(), 0777), 0);

  // Create a symlink
  ASSERT_EQ(symlink("d/e", (base_path + "/a/b/c/symlink_to_e").c_str()), 0);

  // Create a symlink that uses ..
  ASSERT_EQ(symlink("../d/e", (base_path + "/a/b/c/d/symlink_dotdot").c_str()), 0);

  // Create a symlink loop
  ASSERT_EQ(symlink("symlink_loop", (base_path + "/a/b/c/symlink_loop").c_str()), 0);

  // Create a file at the end
  std::string file_path = deep_path + "/f";
  fbl::unique_fd fd(creat(file_path.c_str(), S_IRWXU));
  ASSERT_TRUE(fd.is_valid());
  fd.reset();

  // Test we can open the file via the deep path directly
  fbl::unique_fd fd2(open(file_path.c_str(), O_RDONLY));
  EXPECT_TRUE(fd2.is_valid()) << "Failed to open deep file path: " << strerror(errno);

  // Test opening a directory
  fbl::unique_fd dir_fd(open(deep_path.c_str(), O_RDONLY | O_DIRECTORY));
  EXPECT_TRUE(dir_fd.is_valid()) << "Failed to open deep directory path: " << strerror(errno);

  // Test stat on the deep file
  struct stat st;
  EXPECT_EQ(stat(file_path.c_str(), &st), 0);
  EXPECT_TRUE(S_ISREG(st.st_mode));

  // Test stat on the deep dir
  EXPECT_EQ(stat(deep_path.c_str(), &st), 0);
  EXPECT_TRUE(S_ISDIR(st.st_mode));

  // Test ".."
  std::string dotdot_path = deep_path + "/../e/f";
  fbl::unique_fd fd3(open(dotdot_path.c_str(), O_RDONLY));
  EXPECT_TRUE(fd3.is_valid()) << "Failed to open path with ..: " << strerror(errno);

  // Test symlink
  std::string symlink_path = base_path + "/a/b/c/symlink_to_e/f";
  fbl::unique_fd fd4(open(symlink_path.c_str(), O_RDONLY));
  EXPECT_TRUE(fd4.is_valid()) << "Failed to open path with symlink: " << strerror(errno);

  // Test symlink with ..
  std::string symlink_dotdot_path = base_path + "/a/b/c/d/symlink_dotdot/f";
  fbl::unique_fd fd5(open(symlink_dotdot_path.c_str(), O_RDONLY));
  EXPECT_TRUE(fd5.is_valid()) << "Failed to open path with symlink using ..: " << strerror(errno);

  // Test symlink loop
  std::string symlink_loop_path = base_path + "/a/b/c/symlink_loop/f";
  EXPECT_EQ(open(symlink_loop_path.c_str(), O_RDONLY), -1);
  EXPECT_EQ(errno, ELOOP);

  // Test missing path component
  std::string missing_path = base_path + "/a/b/missing/d/e/f";
  EXPECT_EQ(open(missing_path.c_str(), O_RDONLY), -1);
  EXPECT_EQ(errno, ENOENT);

  // Test path ending in missing component
  std::string missing_end_path = deep_path + "/g";
  EXPECT_EQ(open(missing_end_path.c_str(), O_RDONLY), -1);
  EXPECT_EQ(errno, ENOENT);
}

fit::result<int, test_helper::ScopedLoopDevice> CreateLoopDeviceForImage(
    const std::string &img_path, std::string_view fs_type) {
  constexpr size_t kDefaultImageSizeBytes = 64 * 1024 * 1024;

  fbl::unique_fd img_fd(open(img_path.c_str(), O_RDWR | O_CREAT | O_TRUNC, 0666));
  if (!img_fd.is_valid() || ftruncate(img_fd.get(), kDefaultImageSizeBytes) != 0) {
    return fit::error(errno);
  }
  img_fd.reset();

  std::string mkfs_cmd;
  if (fs_type == "ext4") {
    mkfs_cmd =
        "/sbin/mkfs.ext4 -F -O casefold -E encoding=utf8 \"" + img_path + "\" >/dev/null 2>&1";
  } else if (fs_type == "f2fs") {
    mkfs_cmd = "/sbin/mkfs.f2fs \"" + img_path + "\" -O casefold -C utf8 >/dev/null 2>&1";
  } else {
    return fit::error(EINVAL);
  }

  if (system(mkfs_cmd.c_str()) != 0) {
    return fit::error(ENOENT);
  }

  img_fd = fbl::unique_fd(open(img_path.c_str(), O_RDWR));
  if (!img_fd.is_valid()) {
    return fit::error(errno);
  }
  return test_helper::ScopedLoopDevice::Create(img_fd.get());
}

fit::result<int, std::string> CreateCasefoldDir(const std::string &path) {
  if (mkdir(path.c_str(), 0777) != 0) {
    return fit::error(errno);
  }
  auto result = test_helper::SetCasefold(path, true);
  if (result.is_error()) {
    return result.take_error();
  }
  return fit::ok(path);
}

class FsCasefoldTest : public ::testing::TestWithParam<std::string_view> {
 protected:
  void SetUp() override {
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "Not running with sysadmin capabilities";
    }

    std::string_view fs_type = GetParam();
    std::string base_dir;

    if (fs_type == "fxfs") {
      if (!test_helper::IsStarnix()) {
        GTEST_SKIP() << "Fxfs is only available on Starnix";
      }
      const char *dir = getenv("MUTABLE_STORAGE");
      if (dir == nullptr) {
        GTEST_SKIP() << "MUTABLE_STORAGE environment variable is not set";
      }
      temp_dir_.emplace(dir);
      base_dir = temp_dir_->path();
    } else {
      temp_dir_.emplace();
      base_dir = temp_dir_->path() + "/mount";

      std::string source = "none";
      if (fs_type != "tmpfs") {
        auto loop = CreateLoopDeviceForImage(temp_dir_->path() + "/fs.img", fs_type);
        if (loop.is_error()) {
          GTEST_SKIP() << "mkfs." << fs_type << " not available";
        }
        loop_device_ = std::move(loop.value());
        source = loop_device_->path();
      }

      const char *data = (fs_type == "tmpfs") ? "casefold" : nullptr;
      auto mount = test_helper::ScopedMount::CreateDirAndMount(source, base_dir,
                                                               std::string(fs_type), 0, data);
      if (mount.is_error()) {
        int err = mount.error_value();
        if (err == EINVAL || err == ENODEV || err == EOPNOTSUPP) {
          GTEST_SKIP() << "Mount " << fs_type << " failed: " << strerror(err);
        }
        FAIL() << "Mount " << fs_type << " failed: " << strerror(err);
      }
      scoped_mount_ = std::move(mount.value());
    }

    base_dir_ = base_dir;
    auto casefold_dir = CreateCasefoldDir(base_dir_ + "/casefold_dir");
    ASSERT_THAT(casefold_dir, SyscallResultIsOk());
    casefold_dir_ = std::move(casefold_dir.value());
  }

  const std::string &base_dir() const { return base_dir_; }
  const std::string &casefold_dir() const { return casefold_dir_; }

  std::string path(std::string_view relative) const {
    return casefold_dir_ + "/" + std::string(relative);
  }

  std::string foo_lower() const { return path("foo"); }
  std::string foo_upper() const { return path("FOO"); }
  std::string foo_mixed() const { return path("Foo"); }

  std::string bar_lower() const { return path("bar"); }
  std::string bar_upper() const { return path("BAR"); }
  std::string bar_mixed() const { return path("Bar"); }

 private:
  std::optional<test_helper::ScopedTempDir> temp_dir_;
  std::optional<test_helper::ScopedLoopDevice> loop_device_;
  std::optional<test_helper::ScopedMount> scoped_mount_;
  std::string base_dir_;
  std::string casefold_dir_;
};

INSTANTIATE_TEST_SUITE_P(FsCasefold, FsCasefoldTest,
                         ::testing::Values("fxfs", "tmpfs", "ext4", "f2fs"),
                         [](const testing::TestParamInfo<std::string_view> &info) {
                           return std::string(info.param);
                         });

TEST_P(FsCasefoldTest, LookupMatchesCaseVariants) {
  fbl::unique_fd file_fd(open(foo_mixed().c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(file_fd.get(), SyscallSucceeds());

  EXPECT_THAT(access(foo_lower().c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(foo_upper().c_str(), F_OK), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, CreateExclusiveFailsIfCaseVariantExists) {
  fbl::unique_fd file_fd(open(foo_mixed().c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(file_fd.get(), SyscallSucceeds());

  fbl::unique_fd file_fd2(open(foo_lower().c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  EXPECT_THAT(file_fd2.get(), SyscallFailsWithErrno(EEXIST));
}

TEST_P(FsCasefoldTest, ReaddirPreservesOriginalName) {
  fbl::unique_fd file_fd(open(foo_mixed().c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(file_fd.get(), SyscallSucceeds());

  std::vector<std::string> entries;
  ASSERT_TRUE(files::ReadDirContents(casefold_dir(), &entries));
  EXPECT_THAT(entries, testing::UnorderedElementsAre(".", "..", "Foo"));
}

TEST_P(FsCasefoldTest, UnlinkInvalidatesAllCaseVariants) {
  fbl::unique_fd fd_create(open(foo_mixed().c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd_create.get(), SyscallSucceeds());
  fd_create.reset();

  // Populate dentry cache with lookups for case variants.
  EXPECT_THAT(access(foo_lower().c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(foo_upper().c_str(), F_OK), SyscallSucceeds());

  ASSERT_THAT(unlink(foo_lower().c_str()), SyscallSucceeds());

  // After unlinking "foo", all case variants must fail lookup with ENOENT.
  EXPECT_THAT(open(foo_upper().c_str(), O_RDONLY), SyscallFailsWithErrno(ENOENT));
  EXPECT_THAT(open(foo_mixed().c_str(), O_RDONLY), SyscallFailsWithErrno(ENOENT));
  EXPECT_THAT(access(foo_upper().c_str(), F_OK), SyscallFailsWithErrno(ENOENT));

  // Creating a new file as "foo" allows opening via any case variant.
  fbl::unique_fd fd_new(open(foo_lower().c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd_new.get(), SyscallSucceeds());

  EXPECT_THAT(access(foo_upper().c_str(), F_OK), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, RenameCaseOnlyUpdatesOnDiskName) {
  std::string apple_lower = path("apple");
  std::string apple_upper = path("APPLE");

  fbl::unique_fd fd_create(open(apple_lower.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd_create.get(), SyscallSucceeds());

  // Case-only rename.
  ASSERT_THAT(rename(apple_lower.c_str(), apple_upper.c_str()), SyscallSucceeds());

  // Check readdir output to verify updated casing on disk.
  std::vector<std::string> entries;
  ASSERT_TRUE(files::ReadDirContents(casefold_dir(), &entries));
  // Linux ext4 casefold does not guarantee case-only rename updates directory entries.
  EXPECT_THAT(entries, testing::UnorderedElementsAre(".", "..", testing::AnyOf("apple", "APPLE")));
}

TEST_P(FsCasefoldTest, RenameCaseOnlyNonEmptyDirectory) {
  std::string sub_dir = path("SubDir");
  std::string sub_dir_lower = path("subdir");
  std::string sub_dir_upper = path("SUBDIR");

  ASSERT_THAT(mkdir(sub_dir.c_str(), 0777), SyscallSucceeds());

  std::string nested_file = sub_dir + "/file.txt";
  ASSERT_TRUE(files::WriteFile(nested_file, "content"));

  struct stat stat_parent_before = {};
  ASSERT_THAT(stat(casefold_dir().c_str(), &stat_parent_before), SyscallSucceeds());

  // Renaming a non-empty directory to a case variant of itself succeeds.
  EXPECT_THAT(rename(sub_dir_lower.c_str(), sub_dir_upper.c_str()), SyscallSucceeds());

  EXPECT_THAT(access(path("SUBDIR/file.txt").c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(path("subdir/file.txt").c_str(), F_OK), SyscallSucceeds());

  struct stat stat_parent_after = {};
  ASSERT_THAT(stat(casefold_dir().c_str(), &stat_parent_after), SyscallSucceeds());
  EXPECT_EQ(stat_parent_before.st_nlink, stat_parent_after.st_nlink);

  // Both the child directory and parent directory are non-empty and cannot be removed.
  EXPECT_THAT(rmdir(sub_dir_upper.c_str()), SyscallFailsWithErrno(ENOTEMPTY));
  EXPECT_THAT(rmdir(casefold_dir().c_str()), SyscallFailsWithErrno(ENOTEMPTY));

  // Once the nested file is unlinked, the directory can be removed.
  ASSERT_THAT(unlink(nested_file.c_str()), SyscallSucceeds());
  EXPECT_THAT(rmdir(sub_dir_upper.c_str()), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, NonUtf8NameMatchesExactOpaqueBytes) {
  // Pure non-UTF-8 byte sequence matches exact opaque bytes.
  std::string non_utf8_name = path("\xFF\xFE\xFD");

  fbl::unique_fd fd(open(non_utf8_name.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd.get(), SyscallSucceeds());

  EXPECT_THAT(access(non_utf8_name.c_str(), F_OK), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, NonUtf8BytesSuppressCasefoldingForName) {
  // Mixed invalid UTF-8 and ASCII: invalid UTF-8 bytes suppress casefolding for the name.
  std::string mixed_name = path("Test_\xFF_File");
  fbl::unique_fd fd_mixed(open(mixed_name.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd_mixed.get(), SyscallSucceeds());

  EXPECT_THAT(access(mixed_name.c_str(), F_OK), SyscallSucceeds());
  std::string mixed_lower = path("test_\xFF_file");
  EXPECT_THAT(access(mixed_lower.c_str(), F_OK), SyscallFailsWithErrno(ENOENT));
}

TEST_P(FsCasefoldTest, DirectoryChildInheritsCasefoldFlag) {
  std::string child_dir = path("SubDir");
  ASSERT_THAT(mkdir(child_dir.c_str(), 0777), SyscallSucceeds());

  fbl::unique_fd child_fd(open(child_dir.c_str(), O_RDONLY | O_DIRECTORY));
  ASSERT_THAT(child_fd.get(), SyscallSucceeds());
  int flags = 0;
  ASSERT_THAT(ioctl(child_fd.get(), FS_IOC_GETFLAGS, &flags), SyscallSucceeds());
  EXPECT_TRUE((flags & FS_CASEFOLD_FL) != 0);
}

TEST_P(FsCasefoldTest, DirectoryMultiNamePathResolvesCaseInsensitively) {
  std::string child_dir = path("SubDir");
  ASSERT_THAT(mkdir(child_dir.c_str(), 0777), SyscallSucceeds());

  std::string file_mixed = child_dir + "/NestedFile";
  std::string file_lower = path("subdir/nestedfile");
  std::string file_upper = path("SUBDIR/NESTEDFILE");

  fbl::unique_fd file_fd(open(file_mixed.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(file_fd.get(), SyscallSucceeds());

  EXPECT_THAT(access(file_lower.c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(file_upper.c_str(), F_OK), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, HardLinkCreateFailsIfLinkNameCaseVariantExists) {
  fbl::unique_fd fd(open(foo_mixed().c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd.get(), SyscallSucceeds());

  // Creating a hard link whose link name collides in case with an existing entry must fail with
  // EEXIST.
  EXPECT_THAT(link(foo_mixed().c_str(), foo_lower().c_str()), SyscallFailsWithErrno(EEXIST));
  EXPECT_THAT(link(foo_lower().c_str(), foo_upper().c_str()), SyscallFailsWithErrno(EEXIST));
}

TEST_P(FsCasefoldTest, HardLinkResolvesSourceCaseInsensitively) {
  fbl::unique_fd fd_create(open(foo_mixed().c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd_create.get(), SyscallSucceeds());

  // Create hard link "Bar" pointing to source "FOO" (resolved case-insensitively).
  ASSERT_THAT(link(foo_upper().c_str(), bar_mixed().c_str()), SyscallSucceeds());

  struct stat stat_foo = {};
  struct stat stat_bar = {};
  ASSERT_THAT(stat(foo_upper().c_str(), &stat_foo), SyscallSucceeds());
  ASSERT_THAT(stat(bar_lower().c_str(), &stat_bar), SyscallSucceeds());

  EXPECT_EQ(stat_foo.st_ino, stat_bar.st_ino);
  EXPECT_EQ(stat_foo.st_dev, stat_bar.st_dev);
  EXPECT_EQ(stat_foo.st_nlink, 2u);
  EXPECT_EQ(stat_bar.st_nlink, 2u);
}

TEST_P(FsCasefoldTest, SymlinkCreateFailsIfLinkNameCaseVariantExists) {
  std::string link_mixed = path("LinkA");
  std::string link_lower = path("linka");

  ASSERT_THAT(symlink("target_path", link_mixed.c_str()), SyscallSucceeds());
  EXPECT_THAT(symlink("other_target", link_lower.c_str()), SyscallFailsWithErrno(EEXIST));
}

TEST_P(FsCasefoldTest, SymlinkLookupMatchesCaseVariants) {
  std::string link_mixed = path("LinkA");
  std::string link_upper = path("LINKA");

  ASSERT_THAT(symlink("target_path", link_mixed.c_str()), SyscallSucceeds());

  char buf[64] = {};
  ssize_t len = readlink(link_upper.c_str(), buf, sizeof(buf) - 1);
  ASSERT_THAT(len, SyscallSucceedsWithValue(11));
  EXPECT_EQ(std::string(buf, len), "target_path");
}

TEST_P(FsCasefoldTest, SymlinkTraversalResolvesTargetCaseInsensitively) {
  // Create an on-disk target file "TargetFile".
  std::string target_file = path("TargetFile");
  fbl::unique_fd fd(open(target_file.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd.get(), SyscallSucceeds());

  // Symlink points to "targetfile" (lowercase).
  std::string symlink_file = path("SymlinkToTarget");
  ASSERT_THAT(symlink("targetfile", symlink_file.c_str()), SyscallSucceeds());

  // Opening the symlink follows it and resolves "targetfile" to "TargetFile" case-insensitively.
  EXPECT_THAT(access(symlink_file.c_str(), F_OK), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, RenameRemovesSourceCaseVariants) {
  std::string src_mixed = path("Source");
  std::string src_lower = path("source");
  std::string src_upper = path("SOURCE");
  std::string dst_mixed = path("Target");
  std::string dst_lower = path("target");

  ASSERT_TRUE(files::WriteFile(src_mixed, "source_data"));

  ASSERT_THAT(rename(src_lower.c_str(), dst_mixed.c_str()), SyscallSucceeds());

  EXPECT_THAT(open(src_mixed.c_str(), O_RDONLY), SyscallFailsWithErrno(ENOENT));
  EXPECT_THAT(open(src_lower.c_str(), O_RDONLY), SyscallFailsWithErrno(ENOENT));
  EXPECT_THAT(open(src_upper.c_str(), O_RDONLY), SyscallFailsWithErrno(ENOENT));

  EXPECT_THAT(access(dst_lower.c_str(), F_OK), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, RenameOverwritesDestinationCaseVariant) {
  std::string src_mixed = path("Source");
  std::string src_lower = path("source");
  std::string dst_mixed = path("Target");
  std::string dst_upper = path("TARGET");

  ASSERT_TRUE(files::WriteFile(src_mixed, "source_data"));
  ASSERT_TRUE(files::WriteFile(dst_mixed, "target_data"));

  // Rename source (addressed as lower) over target (addressed as upper).
  ASSERT_THAT(rename(src_lower.c_str(), dst_upper.c_str()), SyscallSucceeds());

  EXPECT_THAT(open(src_mixed.c_str(), O_RDONLY), SyscallFailsWithErrno(ENOENT));
  EXPECT_THAT(open(src_lower.c_str(), O_RDONLY), SyscallFailsWithErrno(ENOENT));

  EXPECT_THAT(access(dst_mixed.c_str(), F_OK), SyscallSucceeds());

  std::string content;
  ASSERT_TRUE(files::ReadFileToString(dst_mixed, &content));
  EXPECT_EQ(content, "source_data");
}

TEST_P(FsCasefoldTest, RenameNoReplaceFailsIfDestinationCaseVariantExists) {
  ASSERT_TRUE(files::WriteFile(foo_mixed(), "foo_data"));
  ASSERT_TRUE(files::WriteFile(bar_mixed(), "bar_data"));

  // RENAME_NOREPLACE must fail with EEXIST when destination exists as a case variant.
  EXPECT_THAT(
      renameat2(AT_FDCWD, bar_mixed().c_str(), AT_FDCWD, foo_lower().c_str(), RENAME_NOREPLACE),
      SyscallFailsWithErrno(EEXIST));

  EXPECT_THAT(access(foo_mixed().c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(bar_mixed().c_str(), F_OK), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, RenameExchangeSwapsEntriesAcrossCaseVariants) {
  std::string file1_mixed = path("FileOne");
  std::string file1_lower = path("fileone");
  std::string file2_mixed = path("FileTwo");
  std::string file2_upper = path("FILETWO");

  ASSERT_TRUE(files::WriteFile(file1_mixed, "payload_one"));
  ASSERT_TRUE(files::WriteFile(file2_mixed, "payload_two"));

  // Exchange FileOne (as lower) and FileTwo (as upper).
  ASSERT_THAT(
      renameat2(AT_FDCWD, file1_lower.c_str(), AT_FDCWD, file2_upper.c_str(), RENAME_EXCHANGE),
      SyscallSucceeds());

  EXPECT_THAT(access(file1_mixed.c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(file2_mixed.c_str(), F_OK), SyscallSucceeds());

  std::string content1;
  ASSERT_TRUE(files::ReadFileToString(file1_mixed, &content1));
  EXPECT_EQ(content1, "payload_two");

  std::string content2;
  ASSERT_TRUE(files::ReadFileToString(file2_mixed, &content2));
  EXPECT_EQ(content2, "payload_one");
}

TEST_P(FsCasefoldTest, RenameExchangeSameEntryCaseVariantsSucceeds) {
  ASSERT_TRUE(files::WriteFile(foo_mixed(), "foo_data"));

  // Exchanging an entry with a case variant of itself is a no-op that succeeds per Linux VFS.
  EXPECT_THAT(
      renameat2(AT_FDCWD, foo_lower().c_str(), AT_FDCWD, foo_upper().c_str(), RENAME_EXCHANGE),
      SyscallSucceeds());

  EXPECT_THAT(access(foo_mixed().c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(foo_lower().c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(foo_upper().c_str(), F_OK), SyscallSucceeds());

  // Check readdir output to verify the original creation name is preserved.
  std::vector<std::string> entries;
  ASSERT_TRUE(files::ReadDirContents(casefold_dir(), &entries));
  EXPECT_THAT(entries, testing::UnorderedElementsAre(".", "..", "Foo"));
}

TEST_P(FsCasefoldTest, RenameCrossCasefoldBoundary) {
  std::string normal_dir = base_dir() + "/normal_dir";
  ASSERT_THAT(mkdir(normal_dir.c_str(), 0777), SyscallSucceeds());

  std::string normal_file = normal_dir + "/hello.txt";
  ASSERT_TRUE(files::WriteFile(normal_file, "hello_content"));

  // Move from normal (case-sensitive) directory to casefolded directory as "Hello.Txt".
  std::string moved_mixed = path("Hello.Txt");
  std::string moved_lower = path("hello.txt");
  std::string moved_upper = path("HELLO.TXT");

  ASSERT_THAT(rename(normal_file.c_str(), moved_mixed.c_str()), SyscallSucceeds());

  EXPECT_THAT(access(normal_file.c_str(), F_OK), SyscallFailsWithErrno(ENOENT));
  EXPECT_THAT(access(moved_lower.c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(moved_upper.c_str(), F_OK), SyscallSucceeds());

  // Move back from casefolded directory (addressed as uppercase) to normal directory as "back.txt".
  std::string back_file = normal_dir + "/back.txt";
  ASSERT_THAT(rename(moved_upper.c_str(), back_file.c_str()), SyscallSucceeds());

  EXPECT_THAT(access(moved_lower.c_str(), F_OK), SyscallFailsWithErrno(ENOENT));
  EXPECT_THAT(access(back_file.c_str(), F_OK), SyscallSucceeds());
  std::string back_file_upper = normal_dir + "/BACK.TXT";
  EXPECT_THAT(access(back_file_upper.c_str(), F_OK), SyscallFailsWithErrno(ENOENT));

  std::string content;
  ASSERT_TRUE(files::ReadFileToString(back_file, &content));
  EXPECT_EQ(content, "hello_content");
}

TEST_P(FsCasefoldTest, UnicodeBasicEquivalence) {
  // 1. Latin NFC vs NFD with case variations.
  // Precomposed NFC "café" (4 bytes: 'c', 'a', 'f', U+00E9 "\xC3\xA9")
  std::string cafe_nfc_lower = path("caf\xC3\xA9");
  // Precomposed NFC "CAFÉ" (4 bytes: 'C', 'A', 'F', U+00C9 "\xC3\x89")
  std::string cafe_nfc_upper = path("CAF\xC3\x89");
  // Decomposed NFD "café" (5 bytes: 'c', 'a', 'f', 'e', U+0301 "\xCC\x81")
  std::string cafe_nfd_lower = path("cafe\xCC\x81");
  // Decomposed NFD "CAFÉ" (5 bytes: 'C', 'A', 'F', 'E', U+0301 "\xCC\x81")
  std::string cafe_nfd_upper = path("CAFE\xCC\x81");

  fbl::unique_fd fd(open(cafe_nfc_lower.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd.get(), SyscallSucceeds());

  // Access checks must succeed across all 4 NFC/NFD case variants.
  EXPECT_THAT(access(cafe_nfc_upper.c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(cafe_nfd_lower.c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(access(cafe_nfd_upper.c_str(), F_OK), SyscallSucceeds());

  // Inode identity must match across NFC and NFD.
  struct stat stat_nfc = {};
  struct stat stat_nfd = {};
  ASSERT_THAT(stat(cafe_nfc_lower.c_str(), &stat_nfc), SyscallSucceeds());
  ASSERT_THAT(stat(cafe_nfd_upper.c_str(), &stat_nfd), SyscallSucceeds());
  EXPECT_EQ(stat_nfc.st_ino, stat_nfd.st_ino);
  EXPECT_EQ(stat_nfc.st_dev, stat_nfd.st_dev);

  // 2. Non-Latin script: Greek uppercase "ΔΟΚΙΜΗ" vs lowercase "δοκιμη".
  std::string greek_upper = path("\xCE\x94\xCE\x9F\xCE\x9A\xCE\x99\xCE\x9C\xCE\x97");
  std::string greek_lower = path("\xCE\xB4\xCE\xBF\xCE\xBA\xCE\xB9\xCE\xBC\xCE\xB7");

  fbl::unique_fd fd_greek(open(greek_upper.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd_greek.get(), SyscallSucceeds());

  EXPECT_THAT(access(greek_lower.c_str(), F_OK), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, UnicodeCreateExclusiveFailsAcrossCanonicalEquivalence) {
  std::string cafe_nfc_lower = path("caf\xC3\xA9");
  std::string cafe_nfd_lower = path("cafe\xCC\x81");

  fbl::unique_fd fd(open(cafe_nfc_lower.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd.get(), SyscallSucceeds());

  // Creating with O_EXCL using decomposed form must fail with EEXIST.
  fbl::unique_fd fd_nfd(open(cafe_nfd_lower.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  EXPECT_THAT(fd_nfd.get(), SyscallFailsWithErrno(EEXIST));
}

TEST_P(FsCasefoldTest, UnicodeUnlinkRemovesAllCanonicalEquivalenceVariants) {
  std::string cafe_nfc_lower = path("caf\xC3\xA9");
  std::string cafe_nfd_lower = path("cafe\xCC\x81");
  std::string cafe_nfd_upper = path("CAFE\xCC\x81");

  ASSERT_TRUE(files::WriteFile(cafe_nfc_lower, "cafe_data"));

  // Unlink using NFD must succeed and remove the entry for all variants.
  ASSERT_THAT(unlink(cafe_nfd_lower.c_str()), SyscallSucceeds());
  EXPECT_THAT(open(cafe_nfc_lower.c_str(), O_RDONLY), SyscallFailsWithErrno(ENOENT));
  EXPECT_THAT(open(cafe_nfd_upper.c_str(), O_RDONLY), SyscallFailsWithErrno(ENOENT));
}

TEST_P(FsCasefoldTest, UnicodeLookupGivenNameExceedingNameMaxMatchesNormalizedName) {
  // Construct a 200-byte precomposed NFC name (100 repetitions of U+00E9 "\xC3\xA9", 2 bytes each).
  // This is <= NAME_MAX (255 bytes) in NFC form, but expands to 300 bytes in decomposed NFD form
  // (100 repetitions of 'e' + U+0301 "\xCC\x81", 3 bytes each).
  std::string nfc_name;
  std::string nfd_name;
  for (int i = 0; i < 100; ++i) {
    nfc_name += "\xC3\xA9";
    nfd_name += "e\xCC\x81";
  }
  ASSERT_EQ(nfc_name.size(), 200u);
  ASSERT_EQ(nfd_name.size(), 300u);

  std::string nfc_path = path(nfc_name);
  std::string nfd_path = path(nfd_name);

  // Creating via the 200-byte NFC name succeeds (raw length <= NAME_MAX).
  fbl::unique_fd fd(open(nfc_path.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd.get(), SyscallSucceeds());

  EXPECT_THAT(access(nfc_path.c_str(), F_OK), SyscallSucceeds());

  // Looking up via the 300-byte NFD name succeeds in Linux casefold filesystems because
  // the given name normalizes to the 200-byte NFC name on disk.
  EXPECT_THAT(access(nfd_path.c_str(), F_OK), SyscallSucceeds());
  EXPECT_THAT(open(nfd_path.c_str(), O_RDONLY), SyscallSucceeds());
}

TEST_P(FsCasefoldTest, UnicodeCanonicalEquivalenceWithMultiByteExpansion) {
  // Construct an unexpanded 160-byte name (80 repetitions of Latin Capital Letter I with Dot Above,
  // U+0130, "\xC4\xB0", 2 bytes each).
  // Under Unicode casefolding, each character expands to 3 bytes ('i' + combining dot
  // "\x69\xCC\x87"), resulting in a 240-byte expanded name (still <= NAME_MAX = 255 bytes).
  std::string dotted_i_upper;
  std::string dotted_i_lower;
  for (int i = 0; i < 80; ++i) {
    dotted_i_upper += "\xC4\xB0";
    dotted_i_lower += "\x69\xCC\x87";
  }
  ASSERT_EQ(dotted_i_upper.size(), 160u);
  ASSERT_EQ(dotted_i_lower.size(), 240u);

  std::string dotted_i_upper_path = path(dotted_i_upper);
  std::string dotted_i_lower_path = path(dotted_i_lower);

  // Creating via the 160-byte name succeeds.
  fbl::unique_fd fd(open(dotted_i_upper_path.c_str(), O_WRONLY | O_CREAT | O_EXCL, 0666));
  ASSERT_THAT(fd.get(), SyscallSucceeds());

  // Accessing via the expanded 240-byte casefolded representation succeeds across the length
  // expansion.
  EXPECT_THAT(access(dotted_i_lower_path.c_str(), F_OK), SyscallSucceeds());
}

}  // namespace
