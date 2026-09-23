// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <thread>

#include <gtest/gtest.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/syscalls/cpp/proc_test_base.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

std::string ProcFilePath(pid_t pid, const char* name) {
  return fxl::StringPrintf("/proc/%d/%s", pid, name);
}

class ZombieProcTest : public ProcTestBase {
 protected:
  void SetUp() override {
    ProcTestBase::SetUp();
    SpawnZombie();
  }

  void TearDown() override {
    ReapZombie();
    ProcTestBase::TearDown();
  }

  void SpawnZombie() {
    test_helper::Rendezvous ready = test_helper::MakeRendezvous();
    test_helper::Rendezvous complete = test_helper::MakeRendezvous();
    leader_pid_ = fork_helper_.RunInForkedProcess(
        [ready = std::move(ready.poker), complete = std::move(complete.holder)]() mutable {
          // Spawn a control thread. This thread will block until the test signals that it is done
          // inspecting the zombie task. At that point, the control thread will exit, causing the
          // entire forked thread group to exit.
          std::thread([&] { complete.hold(); }).detach();

          // Signal to the test that the leader is entering the zombie state. Then, exit the leader
          // thread while the control thread blocks, keeping the forked thread group alive.
          ready.poke();
          syscall(SYS_exit, 0);
        });

    // Wait for the leader thread to exit, entering the zombie state.
    complete_ = std::move(complete.poker);
    ready.holder.hold();
    ASSERT_TRUE(test_helper::WaitUntilZombie(leader_pid_));
  }

  void ReapZombie() {
    // Signal for the forked thread group to exit.
    complete_.poke();
    ASSERT_TRUE(fork_helper_.WaitForChildren());
  }

  bool ReadZombieProcFile(const char* name, std::string& contents) const {
    std::string path = ProcFilePath(leader_pid_, name);
    return files::ReadFileToString(path, &contents);
  }

  testing::AssertionResult AssertZombieProcFileOpenError(const char* name,
                                                         int expected_errno) const {
    std::string path = ProcFilePath(leader_pid_, name);
    fbl::unique_fd fd(open(path.c_str(), O_RDONLY));
    // Being able to open the file is an error. For debugging purposes, log the contents of the file
    // if it can be opened.
    if (fd) {
      std::string contents;
      if (files::ReadFileDescriptorToString(fd.get(), &contents)) {
        return testing::AssertionFailure()
               << "Opening " << path
               << " succeeded, expected failure, but got contents: " << contents;
      }
      return testing::AssertionFailure()
             << "Opening " << path << " succeeded, expected failure, but could not read contents";
    }
    if (errno != expected_errno) {
      return testing::AssertionFailure() << "Expected open(" << path << ") to fail with "
                                         << expected_errno << ", but got " << errno;
    }
    return testing::AssertionSuccess();
  }

  testing::AssertionResult AssertZombieProcFileReadError(const char* name,
                                                         int expected_errno) const {
    std::string path = ProcFilePath(leader_pid_, name);
    fbl::unique_fd fd(open(path.c_str(), O_RDONLY));
    if (!fd) {
      return testing::AssertionFailure() << "Failed to open " << path << ": " << errno;
    }

    char buf[1];
    ssize_t res = read(fd.get(), buf, sizeof(buf));
    if (res >= 0) {
      return testing::AssertionFailure() << "Read from " << path << " succeeded, expected failure";
    }
    if (errno != expected_errno) {
      return testing::AssertionFailure() << "Expected read(" << path << ") to fail with "
                                         << expected_errno << ", but got " << errno;
    }
    return testing::AssertionSuccess();
  }

  testing::AssertionResult AssertZombieProcFileEmpty(const char* name) const {
    std::string contents;
    if (!ReadZombieProcFile(name, contents)) {
      return testing::AssertionFailure() << "Failed to read file " << name;
    }
    if (!contents.empty()) {
      return testing::AssertionFailure()
             << "Expected file " << name << " to be empty, but got contents: " << contents;
    }
    return testing::AssertionSuccess();
  }

  testing::AssertionResult AssertZombieProcFileNotEmpty(const char* name) const {
    std::string contents;
    if (!ReadZombieProcFile(name, contents)) {
      return testing::AssertionFailure() << "Failed to read file " << name;
    }
    if (contents.empty()) {
      return testing::AssertionFailure() << "Expected file " << name << " to be non-empty";
    }
    return testing::AssertionSuccess();
  }

  testing::AssertionResult AssertZombieProcDirEmpty(const char* name) const {
    std::string path = ProcFilePath(leader_pid_, name);
    std::vector<std::string> contents;
    if (!files::ReadDirContents(path, &contents)) {
      return testing::AssertionFailure() << "Failed to read directory " << path;
    }
    const std::vector<std::string> empty_contents = {".", ".."};
    if (contents != empty_contents) {
      return testing::AssertionFailure() << "Expected directory " << path << " to be empty";
    }
    return testing::AssertionSuccess();
  }

  testing::AssertionResult AssertZombieProcDirNotEmpty(const char* name) const {
    std::string path = ProcFilePath(leader_pid_, name);
    std::vector<std::string> contents;
    if (!files::ReadDirContents(path, &contents)) {
      return testing::AssertionFailure() << "Failed to read directory " << path;
    }
    std::vector<std::string> empty_contents = {".", ".."};
    if (contents == empty_contents) {
      return testing::AssertionFailure() << "Expected directory " << path << " to be non-empty";
    }
    return testing::AssertionSuccess();
  }

  test_helper::ForkHelper fork_helper_;
  test_helper::Poker complete_;
  pid_t leader_pid_;
};

// Nodes that fail with ENOENT on open()
TEST_F(ZombieProcTest, Cwd) { ASSERT_TRUE(AssertZombieProcFileOpenError("cwd", ENOENT)); }
TEST_F(ZombieProcTest, Exe) { ASSERT_TRUE(AssertZombieProcFileOpenError("exe", ENOENT)); }
TEST_F(ZombieProcTest, Root) { ASSERT_TRUE(AssertZombieProcFileOpenError("root", ENOENT)); }

// Nodes that fail with EINVAL on open()
TEST_F(ZombieProcTest, MountInfo) {
  ASSERT_TRUE(AssertZombieProcFileOpenError("mountinfo", EINVAL));
}
TEST_F(ZombieProcTest, Mounts) { ASSERT_TRUE(AssertZombieProcFileOpenError("mounts", EINVAL)); }

// Nodes that succeed on open() but fail with ESRCH on read()
// Linux 6.6 allows open() without an mm and fails on read() with ESRCH (Linux 6.11+ fails on
// open()).
TEST_F(ZombieProcTest, SmapsRollup) {
  ASSERT_TRUE(AssertZombieProcFileReadError("smaps_rollup", ESRCH));
}

// Nodes that succeed on open() but fail with EINVAL on read()
TEST_F(ZombieProcTest, AttrExec) {
  ASSERT_TRUE(AssertZombieProcFileReadError("attr/exec", EINVAL));
}
TEST_F(ZombieProcTest, AttrFscreate) {
  ASSERT_TRUE(AssertZombieProcFileReadError("attr/fscreate", EINVAL));
}
TEST_F(ZombieProcTest, AttrKeycreate) {
  ASSERT_TRUE(AssertZombieProcFileReadError("attr/keycreate", EINVAL));
}
TEST_F(ZombieProcTest, AttrPrev) {
  ASSERT_TRUE(AssertZombieProcFileReadError("attr/prev", EINVAL));
}
TEST_F(ZombieProcTest, AttrSockcreate) {
  ASSERT_TRUE(AssertZombieProcFileReadError("attr/sockcreate", EINVAL));
}
TEST_F(ZombieProcTest, ClearRefs) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN";
  }
  ASSERT_TRUE(AssertZombieProcFileReadError("clear_refs", EINVAL));
}

// Nodes that are readable and contain no data
TEST_F(ZombieProcTest, Auxv) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN";
  }
  ASSERT_TRUE(AssertZombieProcFileEmpty("auxv"));
}
TEST_F(ZombieProcTest, Cmdline) { ASSERT_TRUE(AssertZombieProcFileEmpty("cmdline")); }
TEST_F(ZombieProcTest, Environ) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN";
  }
  ASSERT_TRUE(AssertZombieProcFileEmpty("environ"));
}
TEST_F(ZombieProcTest, Fd) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN";
  }
  ASSERT_TRUE(AssertZombieProcDirEmpty("fd"));
}
TEST_F(ZombieProcTest, FdInfo) { ASSERT_TRUE(AssertZombieProcDirEmpty("fdinfo")); }
TEST_F(ZombieProcTest, Maps) { ASSERT_TRUE(AssertZombieProcFileEmpty("maps")); }
TEST_F(ZombieProcTest, Mem) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN";
  }
  ASSERT_TRUE(AssertZombieProcFileEmpty("mem"));
}
TEST_F(ZombieProcTest, PageMap) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN";
  }
  ASSERT_TRUE(AssertZombieProcFileEmpty("pagemap"));
}
TEST_F(ZombieProcTest, Smaps) { ASSERT_TRUE(AssertZombieProcFileEmpty("smaps")); }

// Nodes that are readable and contain general data
TEST_F(ZombieProcTest, AttrCurrent) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("attr/current")); }
TEST_F(ZombieProcTest, Cgroup) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("cgroup")); }
TEST_F(ZombieProcTest, Comm) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("comm")); }
TEST_F(ZombieProcTest, Io) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN";
  }
  ASSERT_TRUE(AssertZombieProcFileNotEmpty("io"));
}
TEST_F(ZombieProcTest, Limits) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("limits")); }
TEST_F(ZombieProcTest, Ns) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN";
  }
  ASSERT_TRUE(AssertZombieProcDirNotEmpty("ns"));
}
TEST_F(ZombieProcTest, OomAdj) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("oom_adj")); }
TEST_F(ZombieProcTest, OomScore) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("oom_score")); }
TEST_F(ZombieProcTest, OomScoreAdj) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("oom_score_adj")); }
TEST_F(ZombieProcTest, Sched) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("sched")); }
TEST_F(ZombieProcTest, Schedstat) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("schedstat")); }
TEST_F(ZombieProcTest, Statm) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("statm")); }
TEST_F(ZombieProcTest, TimerslackNs) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Requires CAP_SYS_ADMIN";
  }
  ASSERT_TRUE(AssertZombieProcFileNotEmpty("timerslack_ns"));
}
TEST_F(ZombieProcTest, Wchan) { ASSERT_TRUE(AssertZombieProcFileNotEmpty("wchan")); }

// Nodes that are readable and contain specific data
TEST_F(ZombieProcTest, Stat) {
  std::string stat;
  ASSERT_TRUE(ReadZombieProcFile("stat", stat));
  char state;
  ASSERT_EQ(sscanf(stat.c_str(), "%*d %*s %c", &state), 1) << stat;
  ASSERT_EQ(state, 'Z');
}

TEST_F(ZombieProcTest, Status) {
  std::string status;
  ASSERT_TRUE(ReadZombieProcFile("status", status));
  ASSERT_THAT(status, testing::ContainsRegex("\nState:[[:space:]]+Z \\(zombie\\)\n"));
}

}  // namespace
