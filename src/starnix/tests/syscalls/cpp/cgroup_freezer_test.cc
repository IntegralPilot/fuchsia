// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <signal.h>
#include <sys/mount.h>
#include <sys/wait.h>

#include <deque>
#include <filesystem>
#include <fstream>
#include <string>

#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

constexpr char PROCS_FILE[] = "cgroup.procs";
constexpr char FREEZE_FILE[] = "cgroup.freeze";
constexpr char EVENTS_FILE[] = "cgroup.events";

std::string procs_path(const std::string& cgroup_path) { return cgroup_path + "/" + PROCS_FILE; }
std::string freeze_path(const std::string& cgroup_path) { return cgroup_path + "/" + FREEZE_FILE; }
std::string events_path(const std::string& cgroup_path) { return cgroup_path + "/" + EVENTS_FILE; }

bool IsCgroupFrozen(const std::string& cgroup_path) {
  std::ifstream events_file(events_path(cgroup_path));
  if (!events_file.is_open()) {
    return false;
  }

  std::string line;
  while (std::getline(events_file, line)) {
    if (line.starts_with("frozen ")) {
      return !!std::atoi(line.substr(7).c_str());
    }
  }
  return false;
}

bool IsCgroupSelfFrozen(const std::string& cgroup_path) {
  std::string freeze_str;
  EXPECT_TRUE(files::ReadFileToString(freeze_path(cgroup_path), &freeze_str));
  return freeze_str == "1\n";
}

void FreezeCgroup(const std::string& cgroup_path) {
  ASSERT_TRUE(files::WriteFile(freeze_path(cgroup_path), "1"));
}

void ThawCgroup(const std::string& cgroup_path) {
  ASSERT_TRUE(files::WriteFile(freeze_path(cgroup_path), "0"));
}

void AddProcToCgroup(pid_t pid, const std::string& cgroup_path) {
  ASSERT_TRUE(files::WriteFile(procs_path(cgroup_path), std::to_string(pid)));
}

void ChildWaitSignal(int signal, int times, pid_t parent_pid) {
  // Set up a signal set to wait for `signal` which will be sent by the parent process
  test_helper::SignalMaskHelper mask_helper;
  mask_helper.blockSignal(signal);

  for (int i = 0; i < times; i++) {
    mask_helper.waitForSignal(signal);
    kill(parent_pid, signal);
  }
}

}  // namespace
class CgroupFreezerTest : public ::testing::Test {
 public:
  void SetUp() override {
    if (!test_helper::HasSysAdmin()) {
      // From https://docs.kernel.org/admin-guide/cgroup-v2.html#interaction-with-other-namespaces
      // mounting cgroup requires CAP_SYS_ADMIN.
      GTEST_SKIP() << "requires CAP_SYS_ADMIN to mount cgroup";
    }
    CreateTestCgroup();
  }

  void TearDown() override { RemoveTestCgroup(); }

 protected:
  std::vector<int> test_pids_;

  void CreateChildCgroup(const std::string& child_path) {
    ASSERT_TRUE(std::filesystem::create_directories(child_path));
    cgroups_.push_front(child_path);
  }

  std::string root_cgroup_path() { return temp_dir_.path() + "/cgroup"; }
  std::string test_cgroup_path() { return root_cgroup_path() + "/test"; }

 private:
  void CreateTestCgroup() {
    ASSERT_FALSE(std::filesystem::exists(root_cgroup_path()));
    ASSERT_TRUE(std::filesystem::create_directories(root_cgroup_path()));
    ASSERT_THAT(mount(nullptr, root_cgroup_path().c_str(), "cgroup2", 0, nullptr),
                SyscallSucceeds());
    ASSERT_TRUE(std::filesystem::create_directories(test_cgroup_path()));
    cgroups_.push_front(test_cgroup_path());
  }

  void RemoveTestCgroup() {
    // Kill the child processes
    for (int pid : test_pids_) {
      kill(pid, SIGKILL);
      waitpid(pid, nullptr, 0);
    }
    for (const auto& cgroup : cgroups_) {
      if (std::filesystem::exists(cgroup)) {
        ASSERT_TRUE(std::filesystem::remove(cgroup));
      }
    }
    if (std::filesystem::exists(root_cgroup_path())) {
      if (test_helper::HasSysAdmin()) {
        ASSERT_THAT(umount(root_cgroup_path().c_str()), SyscallSucceeds());
      }
      ASSERT_TRUE(std::filesystem::remove(root_cgroup_path()));
    }
  }

  test_helper::ScopedTempDir temp_dir_;
  std::deque<std::string> cgroups_;
};

TEST_F(CgroupFreezerTest, FreezeFileAccess) {
  EXPECT_FALSE(IsCgroupSelfFrozen(test_cgroup_path()));
  FreezeCgroup(test_cgroup_path());
  EXPECT_TRUE(IsCgroupSelfFrozen(test_cgroup_path()));
  ThawCgroup(test_cgroup_path());
  EXPECT_FALSE(IsCgroupSelfFrozen(test_cgroup_path()));
}

TEST_F(CgroupFreezerTest, FreezeSingleProcess) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper fork_helper;

  // Set up a signal set to wait for SIGUSR1 which will be sent by the child process
  test_helper::SignalMaskHelper mask_helper;
  mask_helper.blockSignal(SIGUSR1);

  pid_t child_pid =
      fork_helper.RunInForkedProcess([parent_pid] { ChildWaitSignal(SIGUSR1, 2, parent_pid); });
  test_pids_.push_back(child_pid);

  AddProcToCgroup(child_pid, test_cgroup_path());

  // Send signal; child should receive it.
  kill(child_pid, SIGUSR1);
  mask_helper.waitForSignal(SIGUSR1);

  FreezeCgroup(test_cgroup_path());

  // Send signal; frozen child should *not* receive it.
  kill(child_pid, SIGUSR1);
  EXPECT_THAT(mask_helper.timedWaitForSignal(SIGUSR1, 100), SyscallFailsWithErrno(EAGAIN));

  ThawCgroup(test_cgroup_path());

  // Child will process the last signal after thawed.
  mask_helper.waitForSignal(SIGUSR1);

  // Wait for the child process to terminate
  EXPECT_TRUE(fork_helper.WaitForChildren());
}

TEST_F(CgroupFreezerTest, SIGKILLAfterFrozen) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper fork_helper;

  // Set up a signal set to wait for SIGUSR1 which will be sent by the child process
  test_helper::SignalMaskHelper mask_helper;
  mask_helper.blockSignal(SIGUSR1);

  pid_t child_pid =
      fork_helper.RunInForkedProcess([parent_pid] { ChildWaitSignal(SIGUSR1, 2, parent_pid); });
  test_pids_.push_back(child_pid);

  // Make sure the child starts running.
  kill(child_pid, SIGUSR1);
  mask_helper.waitForSignal(SIGUSR1);

  AddProcToCgroup(child_pid, test_cgroup_path());
  FreezeCgroup(test_cgroup_path());

  // Send signal; frozen child should *not* receive it.
  kill(child_pid, SIGUSR1);
  EXPECT_THAT(mask_helper.timedWaitForSignal(SIGUSR1, 100), SyscallFailsWithErrno(EAGAIN));

  // Kill the child process without thawing
  EXPECT_EQ(0, kill(child_pid, SIGKILL));
  EXPECT_FALSE(fork_helper.WaitForChildren());
}

TEST_F(CgroupFreezerTest, AddProcAfterFrozen) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper fork_helper;

  // Set up a signal set to wait for SIGUSR1 which will be sent by the child process
  test_helper::SignalMaskHelper mask_helper;
  mask_helper.blockSignal(SIGUSR1);

  FreezeCgroup(test_cgroup_path());

  pid_t child_pid =
      fork_helper.RunInForkedProcess([parent_pid] { ChildWaitSignal(SIGUSR1, 2, parent_pid); });
  test_pids_.push_back(child_pid);

  // Make sure the child starts running.
  kill(child_pid, SIGUSR1);
  mask_helper.waitForSignal(SIGUSR1);

  AddProcToCgroup(child_pid, test_cgroup_path());

  // Send signal; frozen child should *not* receive it.
  kill(child_pid, SIGUSR1);
  EXPECT_THAT(mask_helper.timedWaitForSignal(SIGUSR1, 100), SyscallFailsWithErrno(EAGAIN));

  ThawCgroup(test_cgroup_path());

  // Child will process the last signal after thawed.
  mask_helper.waitForSignal(SIGUSR1);

  // Wait for the child process to terminate
  EXPECT_TRUE(fork_helper.WaitForChildren());
}

TEST_F(CgroupFreezerTest, AddProcAfterThawed) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper fork_helper;

  // Set up a signal set to wait for SIGUSR1 which will be sent by the child process
  test_helper::SignalMaskHelper mask_helper;
  mask_helper.blockSignal(SIGUSR1);

  FreezeCgroup(test_cgroup_path());
  ThawCgroup(test_cgroup_path());

  pid_t child_pid =
      fork_helper.RunInForkedProcess([parent_pid] { ChildWaitSignal(SIGUSR1, 1, parent_pid); });
  test_pids_.push_back(child_pid);

  AddProcToCgroup(child_pid, test_cgroup_path());

  // Send signal; child should receive it.
  kill(child_pid, SIGUSR1);
  mask_helper.waitForSignal(SIGUSR1);

  // Wait for the child process to terminate
  EXPECT_TRUE(fork_helper.WaitForChildren());
}

TEST_F(CgroupFreezerTest, FreezeNestedCgroups) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper fork_helper;

  // There is no guaranteed order of SIGUSR1 and SIGUSR2 coming from descendant cgroups. Use two
  // signal sets to catch accordingly.
  test_helper::SignalMaskHelper mask_helper_1;
  mask_helper_1.blockSignal(SIGUSR1);
  test_helper::SignalMaskHelper mask_helper_2;
  mask_helper_2.blockSignal(SIGUSR2);

  // Freeze the parent
  FreezeCgroup(test_cgroup_path());

  pid_t child_pid =
      fork_helper.RunInForkedProcess([parent_pid] { ChildWaitSignal(SIGUSR1, 1, parent_pid); });
  test_pids_.push_back(child_pid);

  std::string child_cgroup = test_cgroup_path() + "/child";
  CreateChildCgroup(child_cgroup);
  AddProcToCgroup(child_pid, child_cgroup);

  pid_t grand_child_pid =
      fork_helper.RunInForkedProcess([parent_pid] { ChildWaitSignal(SIGUSR2, 1, parent_pid); });
  test_pids_.push_back(grand_child_pid);

  std::string grand_child_cgroup = child_cgroup + "/grandchild";
  CreateChildCgroup(grand_child_cgroup);
  AddProcToCgroup(grand_child_pid, grand_child_cgroup);

  // Send signal; frozen child should *not* receive it.
  kill(child_pid, SIGUSR1);
  EXPECT_THAT(mask_helper_1.timedWaitForSignal(SIGUSR1, 100), SyscallFailsWithErrno(EAGAIN));

  // Send signal; frozen grandchild should *not* receive it.
  kill(grand_child_pid, SIGUSR2);
  EXPECT_THAT(mask_helper_2.timedWaitForSignal(SIGUSR2, 100), SyscallFailsWithErrno(EAGAIN));

  // Keep the child frozen, but thaw the parent
  FreezeCgroup(child_cgroup);
  ThawCgroup(test_cgroup_path());
  EXPECT_THAT(mask_helper_1.timedWaitForSignal(SIGUSR1, 100), SyscallFailsWithErrno(EAGAIN));
  EXPECT_THAT(mask_helper_2.timedWaitForSignal(SIGUSR2, 100), SyscallFailsWithErrno(EAGAIN));

  // Thaw the child cgroup
  ThawCgroup(child_cgroup);

  // Child and grandchild will process the last signal after thawed.
  mask_helper_1.waitForSignal(SIGUSR1);
  mask_helper_2.waitForSignal(SIGUSR2);

  // Wait for the child process to terminate
  EXPECT_TRUE(fork_helper.WaitForChildren());
}

TEST_F(CgroupFreezerTest, CheckStateInEventsFile) {
  FreezeCgroup(test_cgroup_path());
  EXPECT_TRUE(IsCgroupFrozen(test_cgroup_path()));
  ThawCgroup(test_cgroup_path());
  EXPECT_FALSE(IsCgroupFrozen(test_cgroup_path()));
}

TEST_F(CgroupFreezerTest, FreezerState) {
  std::string parent_cgroup = test_cgroup_path();
  std::string child_cgroup = parent_cgroup + "/child";
  std::string grand_child_cgroup = child_cgroup + "/grandchild";
  CreateChildCgroup(child_cgroup);
  CreateChildCgroup(grand_child_cgroup);

  FreezeCgroup(parent_cgroup);

  // Frozen parent shouldn't impact the child self state.
  EXPECT_TRUE(IsCgroupSelfFrozen(parent_cgroup));
  EXPECT_FALSE(IsCgroupSelfFrozen(child_cgroup));
  EXPECT_FALSE(IsCgroupSelfFrozen(grand_child_cgroup));

  // The frozen state in the descendants should be changed.
  EXPECT_TRUE(IsCgroupFrozen(parent_cgroup));
  EXPECT_TRUE(IsCgroupFrozen(child_cgroup));
  EXPECT_TRUE(IsCgroupFrozen(grand_child_cgroup));

  // Thaw the child cgroup while the parent is still frozen
  ThawCgroup(child_cgroup);
  EXPECT_TRUE(IsCgroupFrozen(child_cgroup));
  EXPECT_FALSE(IsCgroupSelfFrozen(child_cgroup));

  // Thaw the parent cgroup
  ThawCgroup(parent_cgroup);

  EXPECT_FALSE(IsCgroupFrozen(parent_cgroup));
  EXPECT_FALSE(IsCgroupFrozen(child_cgroup));
  EXPECT_FALSE(IsCgroupFrozen(grand_child_cgroup));

  // Freeze the parent and child cgroups and thaw the parent
  FreezeCgroup(parent_cgroup);
  FreezeCgroup(child_cgroup);
  ThawCgroup(parent_cgroup);
  EXPECT_FALSE(IsCgroupFrozen(parent_cgroup));
  EXPECT_TRUE(IsCgroupFrozen(child_cgroup));
  EXPECT_TRUE(IsCgroupFrozen(grand_child_cgroup));
  EXPECT_TRUE(IsCgroupSelfFrozen(child_cgroup));
  EXPECT_FALSE(IsCgroupSelfFrozen(grand_child_cgroup));
}

TEST_F(CgroupFreezerTest, MoveProcess) {
  // This test verifies that moving a process between frozen and thawed cgroups correctly affects
  // signal delivery.  It creates two frozen child cgroups and starts a child process in one of
  // them.  Signals sent to the child process should be blocked while it's in a frozen cgroup and
  // delivered once it's moved to a thawed cgroup.

  pid_t parent_pid = getpid();
  test_helper::ForkHelper fork_helper;
  test_helper::SignalMaskHelper mask_helper;
  mask_helper.blockSignal(SIGUSR1);

  std::string frozen_child1_cgroup = test_cgroup_path() + "/frozen_child_1";
  CreateChildCgroup(frozen_child1_cgroup);
  FreezeCgroup(frozen_child1_cgroup);
  std::string frozen_child2_cgroup = test_cgroup_path() + "/frozen_child_2";
  CreateChildCgroup(frozen_child2_cgroup);
  FreezeCgroup(frozen_child2_cgroup);

  pid_t child_pid =
      fork_helper.RunInForkedProcess([parent_pid] { ChildWaitSignal(SIGUSR1, 2, parent_pid); });
  test_pids_.push_back(child_pid);

  AddProcToCgroup(child_pid, frozen_child1_cgroup);
  kill(child_pid, SIGUSR1);
  EXPECT_THAT(mask_helper.timedWaitForSignal(SIGUSR1, 100), SyscallFailsWithErrno(EAGAIN));

  // Put the child proc into the root cgroup
  AddProcToCgroup(child_pid, root_cgroup_path());
  mask_helper.waitForSignal(SIGUSR1);

  AddProcToCgroup(child_pid, frozen_child2_cgroup);
  kill(child_pid, SIGUSR1);
  EXPECT_THAT(mask_helper.timedWaitForSignal(SIGUSR1, 100), SyscallFailsWithErrno(EAGAIN));

  AddProcToCgroup(child_pid, test_cgroup_path());
  mask_helper.waitForSignal(SIGUSR1);

  EXPECT_TRUE(fork_helper.WaitForChildren());
}
