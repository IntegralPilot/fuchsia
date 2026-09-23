// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/fsuid.h>
#include <sys/inotify.h>
#include <sys/prctl.h>
#include <sys/signalfd.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/timerfd.h>
#include <unistd.h>

#include <format>
#include <sstream>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/perf_event.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/proc_test_base.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

using testing::AnyOf;
using testing::ContainsRegex;
using testing::Eq;

/// Check if the procfs status file shows the correct fsuid number.
void AssertFsuidInProcfsStatus(uid_t fsuid) {
  std::string status_string;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/status", &status_string));
  ASSERT_THAT(status_string,
              testing::ContainsRegex(std::format("Uid:\t[0-9]+\t[0-9]+\t[0-9]+\t{}\n", fsuid)));
}

std::string GetExecChildBinaryPath() {
  return test_helper::GetTestResourcePath("procfs_test_exec_child");
}

class ProcUptimeTest : public ProcTestBase {
 protected:
  void SetUp() override {
    ProcTestBase::SetUp();
    Open();
  }

  void Close() { fd_.reset(); }

  void Open() {
    std::string path = proc_path() + "/uptime";
    fd_.reset(open(path.c_str(), O_RDONLY));
    ASSERT_TRUE(fd_.is_valid());
  }

  double Parse(const char* buf) {
    double uptime, idle;
    int s = sscanf(buf, "%lf %lf\n", &uptime, &idle);
    EXPECT_EQ(s, 2);

    // On Linux `idle` value may decrease, i.e. we cannot expect it to
    // increase with `uptime` (see https://fxbug.dev/42080772). Ignore it.

    return uptime;
  }

  double Read() {
    char buf[100];
    long r = read(fd_.get(), buf, sizeof(buf));
    EXPECT_GT(r, 0);

    return Parse(buf);
  }

  ~ProcUptimeTest() override { Close(); }

  fbl::unique_fd fd_;
};

TEST_F(ProcUptimeTest, UptimeRead) { Read(); }

TEST_F(ProcUptimeTest, UptimeProgressReopen) {
  auto v1 = Read();
  Close();
  sleep(1);
  Open();
  auto v2 = Read();
  EXPECT_GT(v2, v1);
}

// Verify that the reported value is updated after seeking /proc/uptime to the beginning.
TEST_F(ProcUptimeTest, UptimeProgressSeek) {
  auto v1 = Read();

  off_t pos = lseek(fd_.get(), 0, SEEK_SET);
  ASSERT_EQ(pos, 0);

  sleep(1);
  auto v2 = Read();

  EXPECT_GT(v2, v1);
}

// Verify that a valid value is produced even when reading by single char.
TEST_F(ProcUptimeTest, UptimeByChar) {
  auto v1 = Read();
  Open();

  std::string buf;
  char c;
  ssize_t r = read(fd_.get(), &c, 1);
  ASSERT_EQ(r, 1);
  buf.push_back(c);

  // Keep the FD, and then read from a new FD.
  fbl::unique_fd old_fd = std::move(fd_);
  Open();
  auto v2 = Read();

  // Wait for a bit and then read the old FD to the end.
  sleep(1);
  while ((r = read(old_fd.get(), &c, 1)) == 1) {
    buf.push_back(c);
  }

  auto v3 = Parse(buf.c_str());

  // `v3` should be between `v1` and `v2`.
  EXPECT_LE(v1, v3);
  EXPECT_LE(v3, v2);
}

class ProcSysNetTest : public ProcTestBase,
                       public ::testing::WithParamInterface<std::tuple<const char*, const char*>> {
 protected:
  void SetUp() override {
    ProcTestBase::SetUp();
    // Required to open the path below for writing.
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
    }

    auto const& [dev, path_fmt] = GetParam();
    char buf[100] = {};
    sprintf(buf, path_fmt, dev);
    std::string path = proc_path() + "/sys/net" + buf;
    fd_.reset(open(path.c_str(), O_RDWR));
    ASSERT_TRUE(fd_.is_valid()) << "path: " + path + "; " + strerror(errno);
  }

  fbl::unique_fd fd_;
};

TEST_P(ProcSysNetTest, Write) {
  // Notably does not include a null terminator, which the kernel will reject.
  const char kWriteBuf[] = {'1'};
  ssize_t n = write(fd_.get(), kWriteBuf, sizeof(kWriteBuf));
  ASSERT_EQ(n, static_cast<ssize_t>(sizeof(kWriteBuf))) << "Failed to write: " << strerror(errno);
}

INSTANTIATE_TEST_SUITE_P(
    ProcSysNetTest, ProcSysNetTest,
    ::testing::Combine(
        ::testing::Values("all", "default"),
        ::testing::Values("/ipv4/neigh/%s/ucast_solicit", "/ipv4/neigh/%s/retrans_time_ms",
                          "/ipv4/neigh/%s/mcast_resolicit", "/ipv6/conf/%s/accept_ra",
                          "/ipv6/conf/%s/dad_transmits", "/ipv6/conf/%s/use_tempaddr",
                          "/ipv6/conf/%s/addr_gen_mode", "/ipv6/conf/%s/stable_secret",
                          "/ipv6/conf/%s/disable_ipv6", "/ipv6/neigh/%s/ucast_solicit",
                          "/ipv6/neigh/%s/retrans_time_ms", "/ipv6/neigh/%s/mcast_resolicit")));

using ProcTest = ProcTestBase;

// Test that after forking without execing, /proc/self/cmdline still works.
TEST_F(ProcTest, CmdlineAfterFork) {
  char cmdline[100];
  int cmdline_fd = open("/proc/self/cmdline", O_RDONLY);
  ASSERT_GT(cmdline_fd, 0) << strerror(errno);
  ssize_t cmdline_len = read(cmdline_fd, cmdline, sizeof(cmdline));
  ASSERT_GT(cmdline_len, 0) << strerror(errno);
  close(cmdline_fd);

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    char child_cmdline[100];
    int cmdline_fd = open("/proc/self/cmdline", O_RDONLY);
    ASSERT_GT(cmdline_fd, 0) << strerror(errno);
    ssize_t child_cmdline_len = read(cmdline_fd, child_cmdline, sizeof(child_cmdline));
    ASSERT_GT(child_cmdline_len, 0) << strerror(errno);
    close(cmdline_fd);

    ASSERT_EQ(cmdline_len, child_cmdline_len);
    ASSERT_TRUE(memcmp(cmdline, child_cmdline, cmdline_len) == 0);
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

class ProcTaskDirTest : public ProcTestBase {
 protected:
  void SetUp() override { ProcTestBase::SetUp(); }
};

bool ends_with(const std::string& haystack, const std::string needle) {
  return haystack.rfind(needle) == (haystack.size() - needle.size());
}

std::string ProcSelfDirName() {
  std::string proc_self = fxl::StringPrintf("/proc/%d", getpid());
  struct stat statbuf;
  SAFE_SYSCALL(stat(proc_self.c_str(), &statbuf));
  return proc_self;
}

// Ensure that entries in /proc/pid/. have the correct ownership.  proc(2) says
// that entries are owned by the effective uid of the task. This doesn't seem to
// be exactly what Linux does - Linux seems to assign most directories to the
// euid, and everything else to the real id - but it's not clear exactly what
// Linux is doing from looking at its behavior, so we hew to the man page.
//
// This test ensures that the proc directories *are* set to be owned by the euid
// of the task.
TEST_F(ProcTaskDirTest, PidDirCorrectUidIsEuid) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::HasSysAdmin() || !test_helper::IsStarnix()) {
    GTEST_SKIP() << "PidDirCorrectUid requires root access (to change euid), "
                 << "and currently only works on Starnix";
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    std::string proc_path = ProcSelfDirName();

    int dirfd;
    struct stat pre_stat;
    struct stat euid_stat;

    // Get the original (presumably real) ownership
    ASSERT_NE(-1, dirfd = open(proc_path.c_str(), O_RDONLY))
        << "Error trying to open " << proc_path << ": " << strerror(errno);
    SAFE_SYSCALL(fstat(dirfd, &pre_stat));
    SAFE_SYSCALL(close(dirfd));

    // Set the effective uid.
    uid_t newuid = geteuid() + 1;
    SAFE_SYSCALL(seteuid(newuid));

    // From proc(5): The files inside each /proc/pid directory are normally
    // owned by the effective user and effective group ID of the process.
    // However, as a security measure, the ownership is made root:root
    // if the process's "dumpable" attribute is set to a value other than 1.
    SAFE_SYSCALL(prctl(PR_SET_DUMPABLE, 1));

    // Make sure the effective uid appears to own /proc/self.
    SAFE_SYSCALL(dirfd = open(proc_path.c_str(), O_RDONLY));
    SAFE_SYSCALL(fstat(dirfd, &euid_stat));
    SAFE_SYSCALL(close(dirfd));

    EXPECT_EQ(euid_stat.st_uid, newuid)
        << "owner for " << proc_path << " did not change to correct value";

    std::vector<std::string> dirs;
    files::ReadDirContents(proc_path, &dirs);
    for (const auto& entry : dirs) {
      if ((entry == ".") || (entry == "..")) {
        continue;
      }
      struct stat info;
      auto fname =
          (entry[0] == '/') ? entry : fxl::StringPrintf("%s/%s", proc_path.c_str(), entry.c_str());

      ASSERT_NE(-1, stat(fname.c_str(), &info))
          << "Error reading " << fname << ": " << strerror(errno);

      // Links to other parts of the fs have their original ownership.  S_ISLNK does
      // not currently work on these files.  See https://fxbug.dev/331990255.
      if (ends_with(fname, "/cwd") || ends_with(fname, "/root") || ends_with(fname, "/exe")) {
        continue;
      }

      EXPECT_EQ(info.st_uid, euid_stat.st_uid) << "Wrong owner for file " << fname;
    }
  });
}

// This test ensures that the proc directories aren't set to be owned by the
// fsuid of the task. This is separate from PidDirCorrectUidIsEuid because
// setting the euid implicitly sets the fsuid. We want to check we're not
// accidentally relying on the fsuid.
TEST_F(ProcTaskDirTest, PidDirSetFsuidDoesntChangeOwnership) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::HasSysAdmin() || !test_helper::IsStarnix()) {
    GTEST_SKIP() << "PidDirCorrectUid requires root access (to change euid), "
                 << "and currently only works on Starnix";
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    std::string proc_path = ProcSelfDirName();

    int dirfd;
    struct stat pre_stat;
    struct stat fsuid_stat;
    uid_t newuid;

    // Get the original (presumably real) ownership
    ASSERT_NE(-1, dirfd = open(proc_path.c_str(), O_RDONLY))
        << "Error trying to open " << proc_path << ": " << strerror(errno);
    SAFE_SYSCALL(fstat(dirfd, &pre_stat));
    SAFE_SYSCALL(close(dirfd));

    // Set the fsuid.
    newuid = pre_stat.st_uid + 1;
    ASSERT_EQ(static_cast<const int>(pre_stat.st_uid), setfsuid(newuid)) << "Unexpected fsuid";
    // This is how you check to see if a call to setfsuid worked correctly.
    ASSERT_EQ(static_cast<const int>(newuid), setfsuid(-1)) << "setfsuid not supported";
    AssertFsuidInProcfsStatus(newuid);

    // Make sure that the current owner has *not* changed to the fsuid
    SAFE_SYSCALL(dirfd = open(proc_path.c_str(), O_RDONLY));
    SAFE_SYSCALL(fstat(dirfd, &fsuid_stat));
    SAFE_SYSCALL(close(dirfd));

    EXPECT_NE(fsuid_stat.st_uid, newuid) << "fsuid seen to change incorrectly";
    // Revert the fsuid
    ASSERT_EQ(static_cast<const int>(newuid), setfsuid(pre_stat.st_uid));
    AssertFsuidInProcfsStatus(pre_stat.st_uid);
  });
}

TEST_F(ProcTaskDirTest, PidDirCorrectIno) {
  const char kProcPath[] = "/proc/self/status";
  int fd;
  struct stat pre_stat;
  struct stat post_stat;

  SAFE_SYSCALL(fd = open(kProcPath, O_RDONLY));
  SAFE_SYSCALL(fstat(fd, &pre_stat));
  SAFE_SYSCALL(close(fd));

  SAFE_SYSCALL(fd = open(kProcPath, O_RDONLY));
  SAFE_SYSCALL(fstat(fd, &post_stat));
  SAFE_SYSCALL(close(fd));

  ASSERT_EQ(pre_stat.st_ino, post_stat.st_ino) << "Inode number incorrectly seen to change";
}

TEST_F(ProcTaskDirTest, SelfAuxvIsNotEmpty) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/auxv", &contents));
  ASSERT_THAT(contents.size(), testing::Gt(0));
}
TEST_F(ProcTaskDirTest, KthreadAuxvIsEmpty) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/2/auxv", &contents));
  ASSERT_EQ(contents.size(), 0ul);
}

TEST_F(ProcTaskDirTest, SelfCmdlineIsNotEmpty) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/cmdline", &contents));
  ASSERT_THAT(contents.size(), testing::Gt(0));
}
TEST_F(ProcTaskDirTest, KthreadCmdlineIsEmpty) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/2/cmdline", &contents));
  ASSERT_EQ(contents.size(), 0ul);
}

TEST_F(ProcTaskDirTest, SelfEnvironIsNotEmpty) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/environ", &contents));
  ASSERT_THAT(contents.size(), testing::Gt(0));
}
TEST_F(ProcTaskDirTest, KthreadEnvironIsEmpty) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/2/environ", &contents));
  ASSERT_EQ(contents.size(), 0ul);
}

TEST_F(ProcTaskDirTest, SelfExeSymlinkIsValid) {
  char buf[1000];
  ASSERT_THAT(readlink("/proc/self/exe", buf, 1000), testing::Gt(0));
}
TEST_F(ProcTaskDirTest, KthreadExeSymlinkIsNotValid) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  char buf[1000];
  ASSERT_EQ(readlink("/proc/2/exe", buf, 1000), -1);
}

TEST_F(ProcTaskDirTest, SelfSmapsIsNotEmpty) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/smaps", &contents));
  ASSERT_THAT(contents.size(), testing::Gt(0));
}
TEST_F(ProcTaskDirTest, KthreadSmapsIsEmpty) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/2/smaps", &contents));
  ASSERT_EQ(contents.size(), 0ul);
}

TEST_F(ProcTaskDirTest, SelfSmapsRollupIsNotEmpty) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/smaps_rollup", &contents));
  ASSERT_THAT(contents.size(), testing::Gt(0));
  EXPECT_THAT(contents, testing::HasSubstr("[rollup]"));
  EXPECT_THAT(contents, testing::HasSubstr("\nRss:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPss:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPss_Anon:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPss_File:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPss_Shmem:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nShared_Clean:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPrivate_Clean:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPrivate_Dirty:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nAnonymous:"));
}
// Linux 6.6 allows open() without an mm and fails on read() with ESRCH (Linux 6.11+ fails on
// open()).
TEST_F(ProcTaskDirTest, KthreadSmapsRollupReadFails) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  fbl::unique_fd fd(open("/proc/2/smaps_rollup", O_RDONLY));
  ASSERT_TRUE(fd.is_valid());
  char buf[1024];
  EXPECT_THAT(read(fd.get(), buf, sizeof(buf)), SyscallFailsWithErrno(ESRCH));
}

TEST_F(ProcTaskDirTest, SelfStatIsNotEmpty) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/stat", &contents));
  ASSERT_THAT(contents.size(), testing::Gt(0));
}
TEST_F(ProcTaskDirTest, KthreadStatIsNotEmpty) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/2/stat", &contents));
  ASSERT_THAT(contents.size(), testing::Gt(0));
}

// Verify that seeking in a pseudo-dynamic file (which uses lazy seek) and subsequently reading from
// it correctly catches up the sequence generator and returns the expected suffix data.
// Uses `/proc/self/cmdline` as it is static during the lifetime of the process.
TEST_F(ProcTaskDirTest, DynamicFileSeek) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/cmdline", &contents));
  ASSERT_THAT(contents.size(), testing::Gt(5));

  fbl::unique_fd fd(open("/proc/self/cmdline", O_RDONLY));
  ASSERT_TRUE(fd.is_valid());

  off_t offset = lseek(fd.get(), 3, SEEK_SET);
  ASSERT_EQ(offset, 3);

  std::string seeked_contents;
  ASSERT_TRUE(files::ReadFileDescriptorToString(fd.get(), &seeked_contents));
  EXPECT_EQ(seeked_contents, contents.substr(3));
}

// Verify that seeking past the end of a pseudo-dynamic file succeeds (lazy seek) and only returns
// EOF when actually reading.
// Uses `/proc/self/cmdline` as it is static during the lifetime of the process.
TEST_F(ProcTaskDirTest, DynamicFileSeekPastEnd) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/cmdline", &contents));

  fbl::unique_fd fd(open("/proc/self/cmdline", O_RDONLY));
  ASSERT_TRUE(fd.is_valid());

  // Seek past the end of the cmdline file.
  off_t target_offset = static_cast<off_t>(contents.size()) + 1;
  off_t offset = lseek(fd.get(), target_offset, SEEK_SET);
  ASSERT_EQ(offset, target_offset);

  // The seek succeeded. Attempting to read should return 0 bytes (EOF) rather than failing.
  char buf;
  EXPECT_THAT(read(fd.get(), &buf, sizeof(buf)), SyscallSucceedsWithValue(0));
}

// Returns a vector holding the elements of the supplied "/proc/pid/stat" contents, with a zeroeth
// element prepended so that the elements align with the positions documented in the man page.
std::vector<std::string> ProcPidStatElements(const std::string& contents) {
  std::istringstream iss(contents);
  std::vector<std::string> result(std::istream_iterator<std::string>{iss},
                                  std::istream_iterator<std::string>{});
  result.insert(result.begin(), std::string());
  return result;
}

TEST_F(ProcTaskDirTest, ParentFieldsZeroedWithoutCapSysPtrace) {
  if (!test_helper::HasCapabilityPermitted(CAP_SYS_PTRACE)) {
    // Technically any capability is sufficient, so that the parent has greater capabilities than
    // the child, so that the child will fail the ptrace read access check.
    GTEST_SKIP() << "Needs the CAP_SYS_PTRACE capability.";
  }
  pid_t parent_pid = getpid();

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Ensure that we do not have the effective CAP_SYS_PTRACE capability.
    test_helper::DropAllCapabilities();

    std::string path = fxl::StringPrintf("/proc/%d/stat", parent_pid);
    std::string contents;
    ASSERT_TRUE(files::ReadFileToString(path, &contents));
    auto fields = ProcPidStatElements(contents);

    // The fields requiring a ptrace read access check will be zeroed when the check fails.
    EXPECT_EQ(fields[26], "1") << "startcode(26)";
    EXPECT_EQ(fields[27], "1") << "endcode(27)";
    EXPECT_EQ(fields[28], "0") << "startstack(28)";
    EXPECT_EQ(fields[29], "0") << "kstkesp(29)";
    EXPECT_EQ(fields[30], "0") << "kstkeip(30)";

    EXPECT_EQ(fields[35], "0") << "wchan(35)";

    EXPECT_EQ(fields[45], "0") << "start_data(45)";
    EXPECT_EQ(fields[46], "0") << "end_data(46)";
    EXPECT_EQ(fields[47], "0") << "start_brk(47)";
    EXPECT_EQ(fields[48], "0") << "arg_start(48)";
    EXPECT_EQ(fields[49], "0") << "arg_end(49)";
    EXPECT_EQ(fields[50], "0") << "env_start(50)";
    EXPECT_EQ(fields[51], "0") << "env_end(51)";
    EXPECT_EQ(fields[52], "0") << "exit_code(52)";
  });
  ASSERT_TRUE(helper.WaitForChildren());
}
TEST_F(ProcTaskDirTest, ParentFieldsValidWithCapSysPtrace) {
  if (!test_helper::HasCapabilityPermitted(CAP_SYS_PTRACE)) {
    GTEST_SKIP() << "Needs the CAP_SYS_PTRACE capability.";
  }
  pid_t parent_pid = getpid();

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // We need to have the effective CAP_SYS_PTRACE capability.
    test_helper::SetCapabilityEffective(CAP_SYS_PTRACE);

    std::string path = fxl::StringPrintf("/proc/%d/stat", parent_pid);
    std::string contents;
    ASSERT_TRUE(files::ReadFileToString(path, &contents));
    auto fields = ProcPidStatElements(contents);

    // The fields requiring a ptrace read access check will be zeroed when the check fails.
    EXPECT_NE(fields[26], "1") << "startcode(26)";
    EXPECT_NE(fields[27], "1") << "endcode(27)";
    EXPECT_NE(fields[28], "0") << "startstack(28)";

    // kstkesp(29) and kstkeip(30) are typically zero.
    // wchan(35) is zero unless the task is sleeping.

    if (!test_helper::IsStarnix()) {
      // Starnix doesn't yet implement these fields.
      EXPECT_NE(fields[45], "0") << "start_data(45)";
      EXPECT_NE(fields[46], "0") << "end_data(46)";
      EXPECT_NE(fields[47], "0") << "start_brk(47)";
    }

    EXPECT_NE(fields[48], "0") << "arg_start(48)";
    EXPECT_NE(fields[49], "0") << "arg_end(49)";
    EXPECT_NE(fields[50], "0") << "env_start(50)";
    EXPECT_NE(fields[51], "0") << "env_end(51)";

    // exit_code(52) will be zero.
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(ProcTaskDirTest, SelfStatmIsNotEmpty) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/statm", &contents));
  ASSERT_THAT(contents.size(), testing::Gt(0));
}
TEST_F(ProcTaskDirTest, KthreadStatmIsAllZeroes) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/2/statm", &contents));
  ASSERT_EQ(contents, "0 0 0 0 0 0 0\n");
}

TEST_F(ProcTaskDirTest, SelfStatusSensibleOutput) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/status", &contents));
  EXPECT_THAT(contents, testing::HasSubstr("Name:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nUmask:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nSigBlk:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nSigPnd:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nShdPnd:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nState:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nTgid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPPid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nUid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nGid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nGroups:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nVmSize:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nVmLck:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nVmRSS:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nRssAnon:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nRssFile:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nRssShmem:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nRssShmem:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nVmData:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nVmStk:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nVmExe:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nVmSwap:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nThreads:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nVmHWM:"));
}

TEST_F(ProcTaskDirTest, KthreadStatusSensibleOutput) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/2/status", &contents));
  EXPECT_THAT(contents, testing::HasSubstr("Name:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nUmask:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nSigBlk:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nSigPnd:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nShdPnd:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nState:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nTgid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nPPid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nUid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nGid:"));
  EXPECT_THAT(contents, testing::HasSubstr("\nGroups:"));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nVmSize:")));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nVmRSS:")));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nRssAnon:")));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nRssFile:")));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nRssShmem:")));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nRssShmem:")));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nVmData:")));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nVmStk:")));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nVmExe:")));
  EXPECT_THAT(contents, Not(testing::HasSubstr("\nVmSwap:")));
  EXPECT_THAT(contents, testing::HasSubstr("\nThreads:"));
}

constexpr char kProcSelfFdPath[] = "/proc/self/fd/%d";

TEST_F(ProcTaskDirTest, FdOpath) {
  test_helper::ScopedTempFD temp_file;
  ASSERT_TRUE(temp_file);

  fbl::unique_fd opath_fd(SAFE_SYSCALL(open(temp_file.name().c_str(), O_RDONLY | O_PATH)));
  ASSERT_TRUE(opath_fd.is_valid());

  std::string opath_fd_path = fxl::StringPrintf(kProcSelfFdPath, opath_fd.get());
  fbl::unique_fd regular_fd(SAFE_SYSCALL(open(opath_fd_path.c_str(), O_RDONLY)));
  ASSERT_TRUE(regular_fd.is_valid());
}

TEST_F(ProcTaskDirTest, FdOpathSymlink) {
  test_helper::ScopedTempFD temp_file;
  ASSERT_TRUE(temp_file);

  test_helper::ScopedTempSymlink temp_symlink(temp_file.name().c_str());
  ASSERT_TRUE(temp_symlink);

  fbl::unique_fd symlink_fd(SAFE_SYSCALL(open(temp_symlink.path().c_str(), O_PATH | O_NOFOLLOW)));
  ASSERT_TRUE(symlink_fd.is_valid());

  std::string symlink_fd_path = fxl::StringPrintf(kProcSelfFdPath, symlink_fd.get());
  ASSERT_THAT(open(symlink_fd_path.c_str(), O_RDONLY), SyscallFailsWithErrno(ELOOP));
}

TEST_F(ProcTaskDirTest, TimerslackNsSelf) {
  // We do not need CAP_SYS_NICE to access our own timerslack_ns file.
  test_helper::UnsetCapabilityEffective(CAP_SYS_NICE);

  std::string initial_contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/timerslack_ns", &initial_contents));

  std::string new_contents = "1" + initial_contents;
  fbl::unique_fd fd(open("/proc/self/timerslack_ns", O_RDWR));
  if (!fd.is_valid()) {
    if (errno == EROFS) {
      GTEST_SKIP() << "/proc is not writable, skipping";
    } else {
      ADD_FAILURE() << "Failed to open /proc/self/timerslack_ns for writing: " << strerror(errno);
      return;
    }
  }
  ASSERT_NE(write(fd.get(), new_contents.c_str(), new_contents.size()), -1) << errno;
  fd.reset();

  std::string read_contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/self/timerslack_ns", &read_contents));
  EXPECT_EQ(read_contents, new_contents);

  // Writing zero resets to the default value.
  ASSERT_TRUE(files::WriteFile("/proc/self/timerslack_ns", "0"));
  ASSERT_TRUE(files::ReadFileToString("/proc/self/timerslack_ns", &read_contents));
  EXPECT_EQ(read_contents, initial_contents);
}

TEST_F(ProcTaskDirTest, TimerslackNsOtherNoAccess) {
  pid_t parent_pid = getpid();

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Ensure we don't have CAP_SYS_NICE, otherwise we would have access.
    test_helper::DropAllCapabilities();

    std::string path = fxl::StringPrintf("/proc/%d/timerslack_ns", parent_pid);

    // Open succeeds even though the file will not be readable.
    fbl::unique_fd fd(open(path.c_str(), O_RDONLY));
    ASSERT_TRUE(fd.is_valid());

    // But reading fails.
    char buf[256];
    EXPECT_EQ(read(fd.get(), buf, sizeof(buf)), -1);
    EXPECT_EQ(errno, EPERM);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(ProcTaskDirTest, TimerslackNsOtherAccessWithCap) {
  if (!test_helper::HasCapabilityPermitted(CAP_SYS_NICE)) {
    GTEST_SKIP() << "Needs the CAP_SYS_NICE capability.";
  }
  pid_t parent_pid = getpid();

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // We need to have the effective CAP_SYS_NICE capability.
    test_helper::SetCapabilityEffective(CAP_SYS_NICE);
    std::string path = fxl::StringPrintf("/proc/%d/timerslack_ns", parent_pid);
    std::string contents;
    EXPECT_TRUE(files::ReadFileToString(path, &contents));
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

class ProcfsTest : public ProcTestBase {
 protected:
  // Verifies whether the input string is a valid UUID in hyphenated format
  // Example:
  // af0af413-58e1-4210-bb57-bc9a9d3ca44a
  // Segment lengths:
  //     8   |  4 |  4 |  4 |   12
  void is_valid_uuid(std::string in) {
    size_t pos = 0;
    std::string token;
    std::array<size_t, 5> lengths = {8, 4, 4, 4, 12};  // Lengths of tokens
    // Some Linux kernels have a newline character at the end
    if (in[in.size() - 1] == '\n') {
      in.pop_back();
    }
    in.append("-");

    // For each hyphen-delimited token of the UUID string
    for (size_t length : lengths) {
      // Grab token
      ASSERT_TRUE((pos = in.find("-")) != std::string::npos);
      token = in.substr(0, pos);
      in.erase(0, pos + 1);
      ASSERT_TRUE(length == token.size());

      // All characters are hexadecimal digits
      ASSERT_TRUE(std::all_of(token.begin(), token.end(), [](char c) { return std::isxdigit(c); }));
    }
  }
};

// Verify /proc/sys/kernel/random/boot_id exists and has the boot UUID
TEST_F(ProcfsTest, ProcSysKernelRandomBootIdExists) {
  std::string uuid;
  EXPECT_EQ(0, access("/proc/sys/kernel/random/boot_id", R_OK));
  EXPECT_TRUE(files::ReadFileToString("/proc/sys/kernel/random/boot_id", &uuid));
  is_valid_uuid(uuid);
}

// Verify that /proc/zoneinfo contains something reasonable.
TEST_F(ProcfsTest, ZoneInfo) {
  auto path = "/proc/zoneinfo";
  EXPECT_EQ(0, access(path, R_OK));
  std::string content;
  ASSERT_TRUE(files::ReadFileToString(path, &content));
  // Ensures that one node has `nr_inactive_file` and `nr_inactive_file`.
  EXPECT_THAT(content, ContainsRegex("(\n|^)Node [0-9]+, zone +[a-zA-Z]+\n"
                                     "  per-node stats\n"
                                     "( {6}.*\n)*"
                                     "      nr_inactive_file [0-9]+\n"
                                     "      nr_active_file [0-9]+\n"));
  // Ensures that one node has `free`, `min`, `low`, `high`, `present`, among others.
  EXPECT_THAT(content, ContainsRegex("(\n|^)Node [0-9]+, zone +[a-zA-Z]+\n"
                                     "(  .*\n)*"
                                     "  pages free     [0-9]+\n"
                                     "(    .*\n)*"
                                     "        min      [0-9]+\n"
                                     "(    .*\n)*"
                                     "        low      [0-9]+\n"
                                     "(    .*\n)*"
                                     "        high     [0-9]+\n"
                                     "(    .*\n)*"
                                     "        present  [0-9]+\n"
                                     "(    .*\n)*"
                                     "  pagesets\n"));
}

// Verify that /proc/vmstat shape is reasonable.
TEST_F(ProcfsTest, VmStatFile) {
  auto path = "/proc/vmstat";
  EXPECT_EQ(0, access(path, R_OK));
  std::string content;
  ASSERT_TRUE(files::ReadFileToString(path, &content));
  // Ensures that one node has `nr_inactive_file` and `nr_inactive_file`.
  EXPECT_THAT(content, ContainsRegex("(\n|^)workingset_refault_file [0-9]+\n"));
  EXPECT_THAT(content, ContainsRegex("(\n|^)nr_inactive_file [0-9]+\n"));
  EXPECT_THAT(content, ContainsRegex("(\n|^)nr_active_file [0-9]+\n"));
  EXPECT_THAT(content, ContainsRegex("(\n|^)pgscan_direct [0-9]+\n"));
  EXPECT_THAT(content, ContainsRegex("(\n|^)pgscan_kswapd [0-9]+\n"));
}

// Verify that /proc/net/dev format is correct.
TEST_F(ProcfsTest, ProcNetDev) {
  auto path = "/proc/net/dev";
  EXPECT_EQ(0, access(path, R_OK));
  std::string content;
  ASSERT_TRUE(files::ReadFileToString(path, &content));

  std::istringstream iss(content);
  std::string line;

  // Header 1
  ASSERT_TRUE(std::getline(iss, line));
  EXPECT_THAT(line, testing::HasSubstr("Inter-|"));

  // Header 2
  ASSERT_TRUE(std::getline(iss, line));
  EXPECT_THAT(line, testing::HasSubstr(" face |"));

  // Interfaces
  bool found_lo = false;
  while (std::getline(iss, line)) {
    // Trim leading spaces
    size_t start = line.find_first_not_of(" ");
    if (start == std::string::npos) {
      continue;  // empty line
    }
    std::string trimmed = line.substr(start);
    size_t colon = trimmed.find(':');
    ASSERT_NE(colon, std::string::npos) << "Line: " << line;
    std::string iface = trimmed.substr(0, colon);
    std::string stats_str = trimmed.substr(colon + 1);

    if (iface == "lo") {
      found_lo = true;
    }

    std::istringstream stats_iss(stats_str);
    uint64_t val;
    int count = 0;
    while (stats_iss >> val) {
      count++;
    }
    EXPECT_EQ(count, 16) << "Interface " << iface << " has " << count << " stats instead of 16";
  }
  EXPECT_TRUE(found_lo) << "lo interface not found in /proc/net/dev";
}

// Verify that writing invalid data to a numeric sysctl results in EINVAL.
TEST_F(ProcfsTest, ProcSysNetInvalidWriteReturnsEinval) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN to write to net interfaces, skipping.";
  }

  // This particular interface is an example of one which requires int-only values.
  std::string path = "/proc/sys/net/ipv4/neigh/default/ucast_solicit";
  fbl::unique_fd fd(open(path.c_str(), O_WRONLY));
  ASSERT_TRUE(fd.is_valid());

  // "As she is spoke," the Linux kernel rejects null terminated values. This is true even if the
  // data value itself is an integer. The presence of non-numerical data alone causes rejection.
  const char kBadBuf[] = {'1', '\0'};
  ssize_t n = write(fd.get(), kBadBuf, sizeof(kBadBuf));

  EXPECT_EQ(n, -1);
  EXPECT_EQ(errno, EINVAL) << "Expected EINVAL when writing non-numeric data to " << path
                           << " but got: " << strerror(errno);
}

// Writes to /proc/sys/kernel/tainted require CAP_SYS_ADMIN; Linux fails them with EPERM,
// before parsing the value.
TEST_F(ProcfsTest, ProcSysKernelTaintedWriteRequiresSysAdmin) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN to open the file for writing, skipping.";
  }

  constexpr char kPath[] = "/proc/sys/kernel/tainted";

  // Writing 0 adds no taint flags, so this is a no-op on both Linux and Starnix.
  {
    fbl::unique_fd fd(open(kPath, O_WRONLY));
    ASSERT_THAT(fd.get(), SyscallSucceeds());
    EXPECT_THAT(write(fd.get(), "0", 1), SyscallSucceedsWithValue(1));
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Open before dropping the capability: the file is mode 0644 and owned by root, so
    // this isolates the write-time capability check from the open-time DAC check.
    fbl::unique_fd fd(open(kPath, O_WRONLY));
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    test_helper::UnsetCapabilityEffective(CAP_SYS_ADMIN);

    EXPECT_THAT(write(fd.get(), "0", 1), SyscallFailsWithErrno(EPERM));
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

class ProcSelfFdTest : public ProcTestBase {
 protected:
  std::string read_fd_link(int fd) {
    constexpr char kProcSelfFdFormat[] = "/proc/self/fd/%d";
    std::string path = fxl::StringPrintf(kProcSelfFdFormat, fd);

    std::string links_to(PATH_MAX, 0);
    ssize_t result = SAFE_SYSCALL(readlink(path.c_str(), links_to.data(), links_to.capacity()));
    if (result < 0) {
      return fxl::StringPrintf("readlink() failed: %s", strerror(errno));
    }

    links_to.resize(result);
    return links_to;
  }

  std::string expected_name(const char* kind, int fd) {
    struct stat stat_buf;
    if (SAFE_SYSCALL(fstat(fd, &stat_buf)) != 0) {
      return fxl::StringPrintf("fstat() failed: %s", strerror(errno));
    }
    return fxl::StringPrintf("%s:[%lu]", kind, stat_buf.st_ino);
  }
};

// Validate naming of anonymous pipes as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, AnonymousPipeFdName) {
  int pipes[2];
  ASSERT_EQ(SAFE_SYSCALL(pipe2(pipes, 0)), 0) << "pipe() failed:" << strerror(errno);
  fbl::unique_fd pipe_a(pipes[0]);
  fbl::unique_fd pipe_b(pipes[1]);

  EXPECT_EQ(read_fd_link(pipe_a.get()), expected_name("pipe", pipe_a.get()));
  EXPECT_EQ(read_fd_link(pipe_b.get()), expected_name("pipe", pipe_b.get()));
}

// Validate naming of sockets as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, SocketFdName) {
  fbl::unique_fd sock(SAFE_SYSCALL(socket(AF_UNIX, SOCK_STREAM, 0)));
  ASSERT_TRUE(sock.is_valid()) << "socket() failed:" << strerror(errno);

  EXPECT_EQ(read_fd_link(sock.get()), expected_name("socket", sock.get()));
}

// Validate naming of memory file descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, MemFdName) {
  constexpr char kMemFdName[] = "just_a_test_mem_fd";
  fbl::unique_fd mem_fd(SAFE_SYSCALL(memfd_create(kMemFdName, 0)));
  ASSERT_TRUE(mem_fd.is_valid()) << "memfd_create() failed:" << strerror(errno);

  EXPECT_EQ(read_fd_link(mem_fd.get()), fxl::StringPrintf("/memfd:%s (deleted)", kMemFdName));
}

// Validate naming of timerfd descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, TimerFdName) {
  fbl::unique_fd timer_fd(SAFE_SYSCALL(timerfd_create(CLOCK_MONOTONIC, 0)));
  ASSERT_TRUE(timer_fd.is_valid()) << "timerfd_create() failed:" << strerror(errno);

  EXPECT_EQ(read_fd_link(timer_fd.get()), "anon_inode:[timerfd]");
}

// Validate naming of signalfd descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, SignalFdName) {
  sigset_t mask;
  ASSERT_EQ(sigemptyset(&mask), 0);
  ASSERT_EQ(sigaddset(&mask, SIGHUP), 0);
  fbl::unique_fd signal_fd(SAFE_SYSCALL(signalfd(-1, &mask, 0)));
  ASSERT_TRUE(signal_fd.is_valid()) << "signalfd_create() failed:" << strerror(errno);

  EXPECT_EQ(read_fd_link(signal_fd.get()), "anon_inode:[signalfd]");
}

// Validate naming of pidfd descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, PidFdName) {
  fbl::unique_fd pid_fd(SAFE_SYSCALL(static_cast<int>(syscall(SYS_pidfd_open, getpid(), 0))));
  ASSERT_TRUE(pid_fd.is_valid()) << "syscall(SYS_pidfd_open) failed:" << strerror(errno);

  std::string result = read_fd_link(pid_fd.get());
  EXPECT_THAT(result, AnyOf(Eq(expected_name("pidfd", pid_fd.get())), Eq("anon_inode:[pidfd]")));
}

// Validate naming of inotify descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, InotifyFdName) {
  fbl::unique_fd inotify_fd(SAFE_SYSCALL(inotify_init()));
  ASSERT_TRUE(inotify_fd.is_valid()) << "inotify_init() failed:" << strerror(errno);

  EXPECT_EQ(read_fd_link(inotify_fd.get()), "anon_inode:inotify");
}

// Validate naming of epoll descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, EpollFdName) {
  fbl::unique_fd epoll_fd(SAFE_SYSCALL(epoll_create1(0)));
  ASSERT_TRUE(epoll_fd.is_valid()) << "epoll_create1() failed:" << strerror(errno);

  EXPECT_EQ(read_fd_link(epoll_fd.get()), "anon_inode:[eventpoll]");
}

// Validate naming of eventfd descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, EventFdName) {
  fbl::unique_fd event_fd(SAFE_SYSCALL(eventfd(0, 0)));
  ASSERT_TRUE(event_fd.is_valid()) << "eventfd() failed:" << strerror(errno);

  EXPECT_EQ(read_fd_link(event_fd.get()), "anon_inode:[eventfd]");
}

// Validate naming of normal (still file-system linked) descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, PathFdName) {
  constexpr char kTmpPath[] = "/tmp";
  fbl::unique_fd tmp_fd(SAFE_SYSCALL(open(kTmpPath, O_RDONLY)));
  ASSERT_TRUE(tmp_fd.is_valid()) << "open(tmp) failed:" << strerror(errno);

  EXPECT_EQ(read_fd_link(tmp_fd.get()), kTmpPath);
}

// Validate naming of O_TMPFILE descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, TmpFileFdName) {
  constexpr char kTmpPath[] = "/tmp";
  fbl::unique_fd tmpfile_fd(SAFE_SYSCALL(open(kTmpPath, O_RDWR | O_TMPFILE)));
  ASSERT_TRUE(tmpfile_fd.is_valid()) << "open(tmpfile) failed:" << strerror(errno);

  std::string result = read_fd_link(tmpfile_fd.get());
  EXPECT_TRUE(result.starts_with(kTmpPath)) << " target: " << result;
  EXPECT_TRUE(result.ends_with(" (deleted)")) << " target: " << result;
}

// Validate naming of O_TMPFILE descriptors, and the naming of the file that it is linked into.
TEST_F(ProcSelfFdTest, TmpFileLinkIntoAfterFdName) {
  // CAP_DAC_READ_SEARCH capability is required to use AT_EMPTY_PATH with linkat
  if (!test_helper::HasCapability(CAP_DAC_READ_SEARCH)) {
    GTEST_SKIP() << "Not running with CAP_DAC_READ_SEARCH capabilities, skipping.";
  }
  constexpr char kTmpPath[] = "/tmp";
  fbl::unique_fd tmpfile_fd(SAFE_SYSCALL(open(kTmpPath, O_RDWR | O_TMPFILE)));
  ASSERT_TRUE(tmpfile_fd.is_valid()) << "open(tmpfile) failed:" << strerror(errno);

  std::string filename("/tmp/procfs_test_file");
  SAFE_SYSCALL(linkat(tmpfile_fd.get(), "", AT_FDCWD, filename.c_str(), AT_EMPTY_PATH));
  fbl::unique_fd linked_file_fd(open(filename.c_str(), O_RDONLY));
  ASSERT_TRUE(linked_file_fd.is_valid()) << "Failed to open file:" << strerror(errno);

  std::string result_linked_file = read_fd_link(linked_file_fd.get());
  EXPECT_EQ(result_linked_file, filename);

  std::string result_tmp_fd = read_fd_link(tmpfile_fd.get());
  EXPECT_TRUE(result_tmp_fd.starts_with(kTmpPath)) << " target: " << result_tmp_fd;
  EXPECT_TRUE(result_tmp_fd.ends_with(" (deleted)")) << " target: " << result_tmp_fd;
  SAFE_SYSCALL(unlink(filename.c_str()));
}

// Validate naming of a file that is created, and opened, but then unlinked.
TEST_F(ProcSelfFdTest, OpenedAndUnlinkedFileFdName) {
  std::string filename("/tmp/procfs_test_file:XXXXXX");
  fbl::unique_fd fd(SAFE_SYSCALL(mkstemp(filename.data())));
  ASSERT_TRUE(fd.is_valid()) << "mkstemp() failed:" << strerror(errno);

  std::string result = read_fd_link(fd.get());
  EXPECT_EQ(result, filename);

  ASSERT_EQ(SAFE_SYSCALL(unlink(filename.data())), 0) << "unlink() failed:" << strerror(errno);

  result = read_fd_link(fd.get());
  EXPECT_EQ(result, filename + " (deleted)");
}

// Validate naming of a file that is created, and opened, but then unlinked, and is also
// unreachable.
TEST_F(ProcSelfFdTest, DeletedAndUnreachable) {
  if (!test_helper::HasCapability(CAP_SYS_CHROOT)) {
    GTEST_SKIP() << "Not running with chroot capability, skipping";
  }
  std::string filename("/tmp/procfs_test_file:XXXXXX");
  fbl::unique_fd fd(SAFE_SYSCALL(mkstemp(filename.data())));
  ASSERT_TRUE(fd.is_valid()) << "mkstemp() failed:" << strerror(errno);
  unlink(filename.c_str());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(chroot("/proc/self"));

    std::string path = fxl::StringPrintf("/fd/%d", fd.get());
    std::string links_to(PATH_MAX, 0);
    ssize_t result = SAFE_SYSCALL(readlink(path.c_str(), links_to.data(), links_to.capacity()));
    if (result < 0) {
      return;
    }
    links_to.resize(result);
    EXPECT_EQ(links_to, filename + " (deleted)");
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

// Validate naming of perf_event_open descriptors as reported via "/proc/self/fd".
TEST_F(ProcSelfFdTest, PerfEventFdName) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Require CAP_SYS_ADMIN to test perf_event_open(), Skipping.";
  }
  perf_event_attr attr{};
  attr.type = PERF_TYPE_SOFTWARE;
  attr.size = sizeof(attr);
  attr.config = PERF_COUNT_SW_CPU_CLOCK;
  attr.sample_type = PERF_SAMPLE_IP;
  attr.disabled = true;
  attr.exclude_kernel = true;
  attr.exclude_hv = true;
  attr.exclude_idle = true;

  fbl::unique_fd perf_event_fd(SAFE_SYSCALL(static_cast<int>(
      syscall(SYS_perf_event_open, &attr, /*pid=*/0, /*cpu=*/-1, /*group_fd=*/-1, /*flags=*/0))));
  ASSERT_TRUE(perf_event_fd.is_valid()) << "perf_event_open() failed:" << strerror(errno);

  EXPECT_EQ(read_fd_link(perf_event_fd.get()), "anon_inode:[perf_event]");
}

class ProcMmTest : public ProcTestBase, public ::testing::WithParamInterface<const char*> {};

TEST_P(ProcMmTest, ReadBeforeAfterExec) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }

  test_helper::ForkHelper fork_helper;
  test_helper::ScopedPipe child_stdout;
  test_helper::ScopedPipe child_stdin;
  test_helper::Rendezvous fork_ready = test_helper::MakeRendezvous();
  test_helper::Rendezvous exec_ready = test_helper::MakeRendezvous();
  pid_t child_pid = fork_helper.RunInForkedProcess(
      [child_stdout = &child_stdout.WriteSide(), child_stdin = &child_stdin.ReadSide(),
       fork_ready = std::move(fork_ready.poker),
       exec_ready = std::move(exec_ready.holder)]() mutable {
        // Inform the test that the fork succeeded.
        fork_ready.poke();

        // Use the pipes for stdin & stdout. Stderr remains unchanged, to allow error output.
        SAFE_SYSCALL(dup2(child_stdin->get(), STDIN_FILENO));
        child_stdin->reset();
        SAFE_SYSCALL(dup2(child_stdout->get(), STDOUT_FILENO));
        child_stdout->reset();

        // Wait for the test to indicate that it's ready for the child to exec.
        exec_ready.hold();

        // exec the test binary.
        std::string binary_path = GetExecChildBinaryPath();
        char* const argv[] = {const_cast<char*>(binary_path.c_str()), nullptr};
        SAFE_SYSCALL(execve(binary_path.c_str(), argv, nullptr));
        _exit(EXIT_FAILURE);
      });

  // Wait for the child to indicate that it was forked.
  fork_ready.holder.hold();

  // Check that file is not empty before exec.
  std::string path = fxl::StringPrintf("/proc/%d/%s", child_pid, GetParam());
  std::string initial_contents;
  EXPECT_TRUE(files::ReadFileToString(path, &initial_contents));
  EXPECT_THAT(initial_contents.size(), testing::Gt(0));

  // Open the file, keeping it open until after exec.
  fbl::unique_fd fd(SAFE_SYSCALL(open(path.c_str(), O_RDONLY)));
  ASSERT_TRUE(fd.is_valid());

  // Trigger the exec and wait for the child to indicate that it's ready.
  exec_ready.poker.poke();
  test_helper::Rendezvous child_ready = test_helper::MakeRendezvous(std::move(child_stdout));
  child_ready.holder.hold();

  // Check that the existing file descriptor is empty.
  char buf[1];
  EXPECT_EQ(read(fd.get(), buf, sizeof buf), 0);
  fd.reset();

  // Let the child exit.
  test_helper::Rendezvous child_exit = test_helper::MakeRendezvous(std::move(child_stdin));
  child_exit.poker.poke();

  ASSERT_TRUE(fork_helper.WaitForChildren());
}

INSTANTIATE_TEST_SUITE_P(ProcMmTest, ProcMmTest, ::testing::Values("maps", "smaps"),
                         [](const auto& info) { return info.param; });

struct ProcfsAccessParam {
  const char* path;
  bool requires_ptrace;
};

class ProcfsAccessTest : public ProcTestBase,
                         public ::testing::WithParamInterface<ProcfsAccessParam> {};

// Verify that an unprivileged process cannot read sensitive files of another process.
// This reproduces the bug where Starnix allows access without proper checks.
TEST_P(ProcfsAccessTest, AccessDeniedToNonOwner) {
  if (getuid() != 0) {
    GTEST_SKIP() << "Not running as root, skipping.";
  }

  pid_t parent_pid = getpid();
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Switch to a different UID so we are not the owner of the parent process.
    SAFE_SYSCALL(setgid(65534));
    SAFE_SYSCALL(setuid(65534));

    ASSERT_NE(getuid(), 0u);

    std::string path = fxl::StringPrintf("/proc/%d/%s", parent_pid, GetParam().path);

    // Access to the file should be denied, either via permissions, or lack of CAP_SYS_PTRACE.
    EXPECT_THAT(open(path.c_str(), O_RDONLY), SyscallFailsWithErrno(EACCES));
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_P(ProcfsAccessTest, AccessDeniedToNonOwnerWithoutCapPtrace) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  if (getuid() != 0) {
    GTEST_SKIP() << "Not running as root, skipping.";
  }

  pid_t parent_pid = getpid();
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Keep capabilities after setuid to verify access with CAP_SYS_PTRACE.
    SAFE_SYSCALL(prctl(PR_SET_KEEPCAPS, 1));

    SAFE_SYSCALL(setgid(65534));
    SAFE_SYSCALL(setuid(65534));

    // On standard Linux, CAP_SYS_PTRACE alone might not be enough to bypass
    // file permission checks (DAC) for /proc/<pid>/environ if it's 0400 owned by root.
    // We add DAC overrides to ensure we are testing the ptrace check, not DAC.
    test_helper::SetCapabilityEffective(CAP_DAC_OVERRIDE);
    test_helper::SetCapabilityEffective(CAP_DAC_READ_SEARCH);

    ASSERT_FALSE(test_helper::HasCapabilityEffective(CAP_SYS_PTRACE));
    ASSERT_NE(getuid(), 0u);

    // Without CAP_SYS_PTRACE access should be denied if it requires ptrace.
    std::string path = fxl::StringPrintf("/proc/%d/%s", parent_pid, GetParam().path);
    if (GetParam().requires_ptrace) {
      EXPECT_THAT(open(path.c_str(), O_RDONLY), SyscallFailsWithErrno(EACCES));
    } else {
      EXPECT_THAT(open(path.c_str(), O_RDONLY), SyscallSucceeds());
    }
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_P(ProcfsAccessTest, AccessGrantedWithCapPtrace) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping.";
  }
  if (getuid() != 0) {
    GTEST_SKIP() << "Not running as root, skipping.";
  }

  pid_t parent_pid = getpid();
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Keep capabilities after setuid to verify access with CAP_SYS_PTRACE.
    SAFE_SYSCALL(prctl(PR_SET_KEEPCAPS, 1));

    SAFE_SYSCALL(setgid(65534));
    SAFE_SYSCALL(setuid(65534));

    // Explicitly set CAP_SYS_PTRACE effective again (if permitted).
    test_helper::SetCapabilityEffective(CAP_SYS_PTRACE);

    // On standard Linux, CAP_SYS_PTRACE alone might not be enough to bypass
    // file permission checks (DAC) for /proc/<pid>/environ if it's 0400 owned by root.
    // We add DAC overrides to ensure we are testing the ptrace check, not DAC.
    test_helper::SetCapabilityEffective(CAP_DAC_OVERRIDE);
    test_helper::SetCapabilityEffective(CAP_DAC_READ_SEARCH);

    ASSERT_TRUE(test_helper::HasCapabilityEffective(CAP_SYS_PTRACE));
    ASSERT_NE(getuid(), 0u);

    // With CAP_SYS_PTRACE, CAP_DAC_OVERRIDE and CAP_DAC_READ_SEARCH, access should be granted.
    std::string path = fxl::StringPrintf("/proc/%d/%s", parent_pid, GetParam().path);
    EXPECT_THAT(open(path.c_str(), O_RDONLY), SyscallSucceeds());
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_P(ProcfsAccessTest, AccessGrantedToSelf) {
  std::string path = fxl::StringPrintf("/proc/self/%s", GetParam().path);
  EXPECT_THAT(open(path.c_str(), O_RDONLY), SyscallSucceeds());
}

INSTANTIATE_TEST_SUITE_P(
    ProcfsAccessTest, ProcfsAccessTest,
    ::testing::Values(ProcfsAccessParam{"auxv", true}, ProcfsAccessParam{"environ", true},
                      ProcfsAccessParam{"maps", true}, ProcfsAccessParam{"mem", true},
                      ProcfsAccessParam{"pagemap", true}, ProcfsAccessParam{"smaps", true},
                      ProcfsAccessParam{"smaps_rollup", true}, ProcfsAccessParam{"fdinfo", true},
                      ProcfsAccessParam{"fd", false}),
    [](const auto& info) { return info.param.path; });

TEST(ProcFilesystemsTest, Format) {
  std::string contents;
  ASSERT_TRUE(files::ReadFileToString("/proc/filesystems", &contents));
  std::istringstream stream(contents);
  std::string line;
  bool saw_tmpfs = false;
  bool saw_proc = false;
  bool saw_sysfs = false;
  bool saw_ext4 = false;
  while (std::getline(stream, line)) {
    if (line.empty()) {
      continue;
    }
    if (line == "nodev\ttmpfs") {
      saw_tmpfs = true;
    } else if (line == "nodev\tproc") {
      saw_proc = true;
    } else if (line == "nodev\tsysfs") {
      saw_sysfs = true;
    } else if (line == "\text4") {
      saw_ext4 = true;
    }
    EXPECT_TRUE(line.starts_with("nodev\t") || line.starts_with("\t"))
        << "Unexpected line format: " << line;
  }
  EXPECT_TRUE(saw_tmpfs);
  EXPECT_TRUE(saw_proc);
  EXPECT_TRUE(saw_sysfs);
  EXPECT_TRUE(saw_ext4);
}

}  // namespace
