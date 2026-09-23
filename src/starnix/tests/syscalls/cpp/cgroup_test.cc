// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <poll.h>
#include <sys/mount.h>
#include <unistd.h>

#include <algorithm>
#include <cerrno>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

constexpr char CONTROLLERS_FILE[] = "cgroup.controllers";
constexpr char SUBTREE_CONTROL_FILE[] = "cgroup.subtree_control";
constexpr char TYPE_FILE[] = "cgroup.type";
constexpr char PROCS_FILE[] = "cgroup.procs";
constexpr char FREEZE_FILE[] = "cgroup.freeze";
constexpr char EVENTS_FILE[] = "cgroup.events";
constexpr char KILL_FILE[] = "cgroup.kill";
constexpr char EVENTS_POPULATED[] = "populated 1";
constexpr char EVENTS_NOT_POPULATED[] = "populated 0";
constexpr char PROC_CGROUP_PREFIX[] = "0::";

MATCHER_P2(HasUidAndGid, expected_uid, expected_gid, "") {
  struct stat st;
  if (stat(arg.c_str(), &st) != 0) {
    *result_listener << "stat failed with errno " << errno;
    return false;
  }
  *result_listener << "actual UID is " << st.st_uid << ", GID is " << st.st_gid;
  return st.st_uid == expected_uid && st.st_gid == expected_gid;
}

// Mounts cgroup2 in a temporary directory for each test case, and deletes all cgroups created by
// `CreateCgroup` at the end of each test, and all mountpoints of the cgroup.
class CgroupTest : public ::testing::Test {
 public:
  void SetUp() override {
    if (!test_helper::HasSysAdmin()) {
      // From https://docs.kernel.org/admin-guide/cgroup-v2.html#interaction-with-other-namespaces
      // mounting cgroup requires CAP_SYS_ADMIN.
      GTEST_SKIP() << "requires CAP_SYS_ADMIN to mount cgroup";
    }
    MountCgroup2();
  }

  void TearDown() override {
    if (!test_helper::HasSysAdmin()) {
      // `TearDown` is still called for skipped tests, and the below assertions can fail.
      return;
    }

    // Remove paths created by the test in reverse creation order.
    // cgroup2 filesystem persists on the system after umounting, and lingering subdirectories can
    // cause subsequent tests to fail.
    for (auto path = cgroup_paths_.rbegin(); path != cgroup_paths_.rend(); path++) {
      ASSERT_THAT(rmdir(path->c_str()), SyscallSucceeds()) << "Could not delete " << *path << "";
    }

    for (auto mountpoint = cgroup_mountpoints_.rbegin(); mountpoint != cgroup_mountpoints_.rend();
         mountpoint++) {
      ASSERT_THAT(umount((mountpoint->path()).c_str()), SyscallSucceeds());
    }
  }

  // Returns the path of the first mountpoint.
  std::string root_path() { return cgroup_mountpoints_[0].path(); }

  // Creates a temp directory and mount cgroup2 on it. Returns the mountpoint path.
  std::string MountCgroup2() {
    auto& mountpoint = cgroup_mountpoints_.emplace_back();
    EXPECT_THAT(mount(nullptr, mountpoint.path().c_str(), "cgroup2", 0, nullptr),
                SyscallSucceeds());
    return mountpoint.path();
  }

  static void CheckInterfaceFilesExist(const std::string& path, bool is_root) {
    std::string controllers_path = path + "/" + CONTROLLERS_FILE;
    std::string subtree_control_path = path + "/" + SUBTREE_CONTROL_FILE;
    std::string type_path = path + "/" + TYPE_FILE;
    std::string procs_path = path + "/" + PROCS_FILE;
    std::string freeze_path = path + "/" + FREEZE_FILE;
    std::string events_path = path + "/" + EVENTS_FILE;

    struct stat buffer;
    ASSERT_THAT(stat(controllers_path.c_str(), &buffer), SyscallSucceeds());
    ASSERT_THAT(stat(subtree_control_path.c_str(), &buffer), SyscallSucceeds());
    ASSERT_THAT(stat(procs_path.c_str(), &buffer), SyscallSucceeds());
    if (is_root) {
      ASSERT_THAT(stat(freeze_path.c_str(), &buffer), SyscallFailsWithErrno(ENOENT));
      ASSERT_THAT(stat(events_path.c_str(), &buffer), SyscallFailsWithErrno(ENOENT));
      ASSERT_THAT(stat(type_path.c_str(), &buffer), SyscallFailsWithErrno(ENOENT));
    } else {
      ASSERT_THAT(stat(freeze_path.c_str(), &buffer), SyscallSucceeds());
      ASSERT_THAT(stat(events_path.c_str(), &buffer), SyscallSucceeds());
      ASSERT_THAT(stat(type_path.c_str(), &buffer), SyscallSucceeds());
    }
  }

  struct ExpectedEntry {
    std::string name;
    unsigned char type;
  };
  static void CheckDirectoryIncludes(const std::string& path,
                                     const std::vector<ExpectedEntry>& expected) {
    DIR* dir = opendir(path.c_str());
    ASSERT_TRUE(dir);

    std::unordered_map<std::string, unsigned char> entry_types;
    while (struct dirent* entry = readdir(dir)) {
      entry_types.emplace(std::string(entry->d_name), entry->d_type);
    }
    closedir(dir);

    for (const ExpectedEntry& entry : expected) {
      auto found = entry_types.find(entry.name);
      ASSERT_NE(found, entry_types.end()) << entry.name << " not found in directory";
      EXPECT_EQ(found->second, entry.type);
    }
  }

  static testing::AssertionResult CheckFileForLine(const std::string& path, const std::string& line,
                                                   const bool should_exist) {
    std::ifstream file(path);
    if (!file.is_open()) {
      return testing::AssertionFailure() << "Unable to open " << path;
    }

    std::string file_line;
    while (std::getline(file, file_line)) {
      if (line == file_line) {
        if (should_exist) {
          return testing::AssertionSuccess();
        }
        return testing::AssertionFailure() << "Unexpectedly found " << line << " in " << path;
      }
    }

    if (should_exist) {
      return testing::AssertionFailure() << "Could not find " << line << " in " << path;
    }
    return testing::AssertionSuccess();
  }

  static testing::AssertionResult CheckFileHasLine(const std::string& path,
                                                   const std::string& line) {
    return CheckFileForLine(path, line, true);
  }

  static testing::AssertionResult CheckFileDoesNotHaveLine(const std::string& path,
                                                           const std::string& line) {
    return CheckFileForLine(path, line, false);
  }

  void CreateCgroup(std::string path) {
    ASSERT_THAT(mkdir(path.c_str(), 0777), SyscallSucceeds()) << "Could not create " << path;
    cgroup_paths_.push_back(std::move(path));
  }

  void DeleteCgroup(const std::string& path) {
    auto it = std::ranges::find(cgroup_paths_, path);
    ASSERT_NE(it, cgroup_paths_.end()) << path << " not found";
    ASSERT_THAT(rmdir(path.c_str()), SyscallSucceeds()) << "Could not delete " << path;
    cgroup_paths_.erase(it);
  }

 private:
  // Paths to be removed after a test has completed.
  std::vector<std::string> cgroup_paths_;

  // Mountpoints to be unmounted after a test has completed.
  std::vector<test_helper::ScopedTempDir> cgroup_mountpoints_;
};

TEST_F(CgroupTest, InterfaceFilesForRoot) { CheckInterfaceFilesExist(root_path(), true); }

// This test checks that nodes created as part of cgroups have the same inode each time it is
// accessed, which is seen on Linux.
TEST_F(CgroupTest, InodeNumbersAreConsistent) {
  std::string controllers_path = root_path() + "/" + CONTROLLERS_FILE;
  struct stat buffer1, buffer2;
  ASSERT_THAT(stat(controllers_path.c_str(), &buffer1), SyscallSucceeds());
  ASSERT_THAT(stat(controllers_path.c_str(), &buffer2), SyscallSucceeds());
  EXPECT_EQ(buffer1.st_ino, buffer2.st_ino);
}

TEST_F(CgroupTest, ReadDir) {
  CheckDirectoryIncludes(root_path(), {
                                          {.name = PROCS_FILE, .type = DT_REG},
                                          {.name = CONTROLLERS_FILE, .type = DT_REG},
                                          {.name = SUBTREE_CONTROL_FILE, .type = DT_REG},
                                      });

  std::string child1 = "child1";
  CreateCgroup(root_path() + "/" + child1);
  CheckDirectoryIncludes(root_path(), {
                                          {.name = PROCS_FILE, .type = DT_REG},
                                          {.name = CONTROLLERS_FILE, .type = DT_REG},
                                          {.name = SUBTREE_CONTROL_FILE, .type = DT_REG},
                                          {.name = child1, .type = DT_DIR},
                                      });

  std::string child2 = "child2";
  CreateCgroup(root_path() + "/" + child2);
  CheckDirectoryIncludes(root_path(), {
                                          {.name = PROCS_FILE, .type = DT_REG},
                                          {.name = CONTROLLERS_FILE, .type = DT_REG},
                                          {.name = SUBTREE_CONTROL_FILE, .type = DT_REG},
                                          {.name = child1, .type = DT_DIR},
                                          {.name = child2, .type = DT_DIR},
                                      });
}

TEST_F(CgroupTest, CreateSubgroups) {
  std::string child1_path = root_path() + "/child1";
  CreateCgroup(child1_path);
  CheckInterfaceFilesExist(child1_path, false);

  std::string child2_path = root_path() + "/child2";
  CreateCgroup(child2_path);
  CheckInterfaceFilesExist(child2_path, false);

  std::string grandchild_path = root_path() + "/child2/grandchild";
  CreateCgroup(grandchild_path);
  CheckInterfaceFilesExist(grandchild_path, false);
}

TEST_F(CgroupTest, CreateSubgroupAlreadyExists) {
  std::string child_path = root_path() + "/child";
  CreateCgroup(child_path);
  ASSERT_THAT(mkdir(child_path.c_str(), 0777), SyscallFailsWithErrno(EEXIST));
}

TEST_F(CgroupTest, WriteToInterfaceFileAfterCgroupIsDeleted) {
  std::string child_path = root_path() + "/child";
  std::string child_procs_path = child_path + "/" + PROCS_FILE;

  CreateCgroup(child_path);

  fbl::unique_fd child_procs_fd(open(child_procs_path.c_str(), O_WRONLY));
  ASSERT_TRUE(child_procs_fd.is_valid());

  DeleteCgroup(child_path);

  std::string pid_string = std::to_string(getpid());
  EXPECT_THAT(write(child_procs_fd.get(), pid_string.c_str(), pid_string.length()),
              SyscallFailsWithErrno(ENODEV));
}

TEST_F(CgroupTest, MoveProcessToCgroup) {
  std::string root_procs_path = root_path() + "/" + PROCS_FILE;
  std::string child_path = root_path() + "/child";
  std::string child_procs_path = child_path + "/" + PROCS_FILE;
  std::string child_events_path = child_path + "/" + EVENTS_FILE;
  std::string pid_string = std::to_string(getpid());

  CreateCgroup(child_path);
  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_NOT_POPULATED));

  {
    // Write pid to /child/cgroup.procs
    fbl::unique_fd child_procs_fd(open(child_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_procs_fd.is_valid());
    EXPECT_THAT(write(child_procs_fd.get(), pid_string.c_str(), pid_string.length()),
                SyscallSucceeds());
  }

  ASSERT_TRUE(CheckFileDoesNotHaveLine(root_procs_path, pid_string));
  ASSERT_TRUE(CheckFileHasLine(child_procs_path, pid_string));
  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_POPULATED));

  {
    // Write pid to /cgroup.procs
    fbl::unique_fd procs_fd(open(root_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(procs_fd.is_valid());
    EXPECT_THAT(write(procs_fd.get(), pid_string.c_str(), pid_string.length()), SyscallSucceeds());
  }

  ASSERT_TRUE(CheckFileDoesNotHaveLine(child_procs_path, pid_string));
  ASSERT_TRUE(CheckFileHasLine(root_procs_path, pid_string));
  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_NOT_POPULATED));
}

TEST_F(CgroupTest, EventsWithPopulatedChild) {
  std::string root_procs_path = root_path() + "/" + PROCS_FILE;
  std::string child_path = root_path() + "/child";
  std::string child_events_path = child_path + "/" + EVENTS_FILE;
  std::string grandchild_path = child_path + "/grandchild";
  std::string grandchild_procs_path = grandchild_path + "/" + PROCS_FILE;
  std::string grandchild_events_path = grandchild_path + "/" + EVENTS_FILE;
  std::string pid_string = std::to_string(getpid());

  CreateCgroup(child_path);
  CreateCgroup(grandchild_path);

  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_NOT_POPULATED));
  ASSERT_TRUE(CheckFileHasLine(grandchild_events_path, EVENTS_NOT_POPULATED));

  {
    // Write pid to /child/grandchild/cgroup.procs
    fbl::unique_fd child_procs_fd(open(grandchild_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_procs_fd.is_valid());
    EXPECT_THAT(write(child_procs_fd.get(), pid_string.c_str(), pid_string.length()),
                SyscallSucceeds());
  }

  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_POPULATED));
  ASSERT_TRUE(CheckFileHasLine(grandchild_events_path, EVENTS_POPULATED));

  {
    // Write pid to /cgroup.procs
    fbl::unique_fd procs_fd(open(root_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(procs_fd.is_valid());
    EXPECT_THAT(write(procs_fd.get(), pid_string.c_str(), pid_string.length()), SyscallSucceeds());
  }

  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_NOT_POPULATED));
  ASSERT_TRUE(CheckFileHasLine(grandchild_events_path, EVENTS_NOT_POPULATED));
}

TEST_F(CgroupTest, PollEvents) {
  std::string child_path = root_path() + "/child";
  std::string child_events_path = child_path + "/" + EVENTS_FILE;
  std::string child_procs_path = child_path + "/" + PROCS_FILE;
  std::string pid_string = std::to_string(getpid());

  CreateCgroup(child_path);

  fbl::unique_fd events_fd(open(child_events_path.c_str(), O_RDONLY));
  ASSERT_TRUE(events_fd.is_valid());

  // Initially, the cgroup should not be populated.
  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_NOT_POPULATED));

  struct pollfd pfd = {.fd = events_fd.get(), .events = POLLPRI};
  fbl::unique_fd procs_fd(open(child_procs_path.c_str(), O_WRONLY));
  ASSERT_TRUE(procs_fd.is_valid());
  EXPECT_THAT(write(procs_fd.get(), pid_string.c_str(), pid_string.length()), SyscallSucceeds());

  // After adding the process, poll should return with POLLPRI as populated changes to true.
  EXPECT_THAT(poll(&pfd, 1, -1), SyscallSucceedsWithValue(1));
  EXPECT_TRUE(pfd.revents & (POLLPRI | POLLERR));

  // Verify the populated state has changed.
  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_POPULATED));

  // Now remove the process from the cgroup.
  std::string root_procs_path = root_path() + "/" + PROCS_FILE;
  procs_fd.reset(open(root_procs_path.c_str(), O_WRONLY));
  ASSERT_TRUE(procs_fd.is_valid());
  EXPECT_THAT(write(procs_fd.get(), pid_string.c_str(), pid_string.length()), SyscallSucceeds());

  // Poll should return with POLLPRI as populated changes back to false.
  EXPECT_THAT(poll(&pfd, 1, -1), SyscallSucceedsWithValue(1));
  EXPECT_TRUE(pfd.revents & (POLLPRI | POLLERR));

  // Verify the populated state has changed.
  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_NOT_POPULATED));
}

TEST_F(CgroupTest, UnlinkCgroupWithProcess) {
  std::string root_procs_path = root_path() + "/" + PROCS_FILE;
  std::string child_path = root_path() + "/child";
  std::string child_procs_path = child_path + "/" + PROCS_FILE;
  std::string pid_string = std::to_string(getpid());

  CreateCgroup(child_path);

  {
    fbl::unique_fd child_procs_fd(open(child_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_procs_fd.is_valid());
    EXPECT_THAT(write(child_procs_fd.get(), pid_string.c_str(), pid_string.length()),
                SyscallSucceeds());
  }

  ASSERT_THAT(rmdir(child_path.c_str()), SyscallFailsWithErrno(EBUSY));

  {
    fbl::unique_fd procs_fd(open(root_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(procs_fd.is_valid());
    EXPECT_THAT(write(procs_fd.get(), pid_string.c_str(), pid_string.length()), SyscallSucceeds());
  }
}

TEST_F(CgroupTest, UnlinkCgroupWithChildren) {
  std::string child_path = root_path() + "/child";
  std::string grandchild_path = child_path + "/grandchild";

  CreateCgroup(child_path);
  CreateCgroup(grandchild_path);

  ASSERT_THAT(rmdir(child_path.c_str()), SyscallFailsWithErrno(EBUSY));
}

TEST_F(CgroupTest, EventsFileSeekable) {
  std::string child_path = root_path() + "/child";
  std::string events_path = child_path + "/" + EVENTS_FILE;

  CreateCgroup(child_path);
  fbl::unique_fd events_fd(open(events_path.c_str(), O_RDONLY));
  ASSERT_TRUE(events_fd.is_valid());
  // Seek exactly 10 bytes over, skipping "populated ". The next byte read should be 1 or 0
  // indicating whether the cgroup is populated or not, respectively.
  EXPECT_THAT(lseek(events_fd.get(), 10, SEEK_SET), SyscallSucceeds());

  char buffer;
  EXPECT_THAT(read(events_fd.get(), &buffer, 1), SyscallSucceeds());
  EXPECT_EQ(buffer, '0');
}

TEST_F(CgroupTest, KillEmptyCgroup) {
  std::string child_path = root_path() + "/child";
  std::string child_kill_path = child_path + "/" + KILL_FILE;

  CreateCgroup(child_path);

  {
    fbl::unique_fd child_kill_fd(open(child_kill_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_kill_fd.is_valid());
    EXPECT_THAT(write(child_kill_fd.get(), "1", 1), SyscallSucceeds());
  }
}

TEST_F(CgroupTest, KillCgroupWithProcess) {
  std::string child_path = root_path() + "/child";
  std::string child_procs_path = child_path + "/" + PROCS_FILE;
  std::string child_events_path = child_path + "/" + EVENTS_FILE;
  std::string child_kill_path = child_path + "/" + KILL_FILE;

  CreateCgroup(child_path);

  test_helper::ForkHelper fork_helper;
  fork_helper.OnlyWaitForForkedChildren();
  fork_helper.ExpectSignal(SIGKILL);

  pid_t child_pid = fork_helper.RunInForkedProcess([]() {
    // Child process blocks forever.
    while (true) {
      pause();
    }
  });

  // Move forked child to /child/cgroup.procs
  {
    std::string pid_string = std::to_string(child_pid);
    fbl::unique_fd child_procs_fd(open(child_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_procs_fd.is_valid());
    EXPECT_THAT(write(child_procs_fd.get(), pid_string.c_str(), pid_string.length()),
                SyscallSucceeds());
  }

  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_POPULATED));

  {
    fbl::unique_fd child_kill_fd(open(child_kill_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_kill_fd.is_valid());
    EXPECT_THAT(write(child_kill_fd.get(), "1", 1), SyscallSucceeds());
  }

  EXPECT_TRUE(fork_helper.WaitForChildren());
  ASSERT_TRUE(CheckFileHasLine(child_events_path, EVENTS_NOT_POPULATED));
}

TEST_F(CgroupTest, KillCgroupWithDescendant) {
  std::string child_path = root_path() + "/child";
  std::string grandchild_path = child_path + "/grandchild";
  std::string grandchild_procs_path = grandchild_path + "/" + PROCS_FILE;
  std::string grandchild_events_path = grandchild_path + "/" + EVENTS_FILE;
  std::string grandchild_kill_path = grandchild_path + "/" + KILL_FILE;

  CreateCgroup(child_path);
  CreateCgroup(grandchild_path);

  test_helper::ForkHelper fork_helper;
  fork_helper.OnlyWaitForForkedChildren();
  fork_helper.ExpectSignal(SIGKILL);

  pid_t child_pid = fork_helper.RunInForkedProcess([]() {
    // Child process blocks forever.
    while (true) {
      pause();
    }
  });

  // Move forked child to /child/grandchild/cgroup.procs
  {
    std::string pid_string = std::to_string(child_pid);
    fbl::unique_fd child_procs_fd(open(grandchild_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_procs_fd.is_valid());
    EXPECT_THAT(write(child_procs_fd.get(), pid_string.c_str(), pid_string.length()),
                SyscallSucceeds());
  }

  ASSERT_TRUE(CheckFileHasLine(grandchild_events_path, EVENTS_POPULATED));

  {
    fbl::unique_fd child_kill_fd(open(grandchild_kill_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_kill_fd.is_valid());
    EXPECT_THAT(write(child_kill_fd.get(), "1", 1), SyscallSucceeds());
  }

  EXPECT_TRUE(fork_helper.WaitForChildren());
  ASSERT_TRUE(CheckFileHasLine(grandchild_events_path, EVENTS_NOT_POPULATED));
}

TEST_F(CgroupTest, ProcfsCgroup) {
  std::string root_procs_path = root_path() + "/" + PROCS_FILE;
  std::string child_path_from_root = "/child";
  std::string child_path = root_path() + child_path_from_root;
  std::string child_procs_path = child_path + "/" + PROCS_FILE;
  std::string grandchild_path_from_root = child_path_from_root + "/grandchild";
  std::string grandchild_path = root_path() + grandchild_path_from_root;
  std::string grandchild_procs_path = grandchild_path + "/" + PROCS_FILE;
  std::string procfs_cgroup_path = "/proc/self/cgroup";
  std::string pid_string = std::to_string(getpid());

  ASSERT_TRUE(CheckFileHasLine(procfs_cgroup_path, PROC_CGROUP_PREFIX + std::string("/")));

  CreateCgroup(child_path);
  CreateCgroup(grandchild_path);

  {
    fbl::unique_fd child_procs_fd(open(child_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_procs_fd.is_valid());
    EXPECT_THAT(write(child_procs_fd.get(), pid_string.c_str(), pid_string.length()),
                SyscallSucceeds());
  }

  ASSERT_TRUE(CheckFileHasLine(procfs_cgroup_path, PROC_CGROUP_PREFIX + child_path_from_root));

  {
    fbl::unique_fd grandchild_procs_fd(open(grandchild_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(grandchild_procs_fd.is_valid());
    EXPECT_THAT(write(grandchild_procs_fd.get(), pid_string.c_str(), pid_string.length()),
                SyscallSucceeds());
  }

  ASSERT_TRUE(CheckFileHasLine(procfs_cgroup_path, PROC_CGROUP_PREFIX + grandchild_path_from_root));
  {
    fbl::unique_fd procs_fd(open(root_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(procs_fd.is_valid());
    EXPECT_THAT(write(procs_fd.get(), pid_string.c_str(), pid_string.length()), SyscallSucceeds());
  }
}

// `CgroupTest` mounts a cgroup2 during `SetUp()`. This test case mounts cgroup2 filesystem again at
// another mountpoint, and expects that operations are reflected in both mounts.
TEST_F(CgroupTest, MountCgroup2Twice) {
  const std::string mountpoint = MountCgroup2();

  CheckInterfaceFilesExist(mountpoint, true);

  // Create /child in the first mount and observe from second mount.
  const std::string child = "child";
  const std::string child_path = root_path() + "/" + child;
  const std::string child_path_mirrored = mountpoint + "/" + child;

  CreateCgroup(child_path);
  CheckDirectoryIncludes(mountpoint, {{.name = child, .type = DT_DIR}});
  CheckInterfaceFilesExist(child_path_mirrored, false);

  // Create /child/grandchild in the second mount and observe from first mount.
  const std::string grandchild = "grandchild";
  const std::string grandchild_path = child_path + "/" + grandchild;
  const std::string grandchild_path_mirrored = child_path_mirrored + "/" + grandchild;

  CreateCgroup(grandchild_path_mirrored);
  CheckDirectoryIncludes(child_path, {{.name = grandchild, .type = DT_DIR}});
  CheckInterfaceFilesExist(grandchild_path, false);
}

TEST_F(CgroupTest, ForkedProcessInheritsCgroup) {
  // Create child cgroup and put the current pid into it. Fork a new process which should be
  // automatically added the cgroup
  std::string child_str = "/child";
  std::string child_path = root_path() + child_str;
  std::string child_procs_path = child_path + "/" + PROCS_FILE;
  std::string procfs_cgroup_path = "/proc/self/cgroup";
  std::string procfs_cgroup_str = PROC_CGROUP_PREFIX + child_str;
  std::string pid_string = std::to_string(getpid());

  CreateCgroup(child_path);

  // Move current process to the child cgroup.
  {
    fbl::unique_fd child_procs_fd(open(child_procs_path.c_str(), O_WRONLY));
    ASSERT_TRUE(child_procs_fd.is_valid());
    EXPECT_THAT(write(child_procs_fd.get(), pid_string.c_str(), pid_string.length()),
                SyscallSucceeds());
  }
  ASSERT_TRUE(CheckFileHasLine(procfs_cgroup_path, procfs_cgroup_str));

  test_helper::ForkHelper fork_helper;

  fork_helper.RunInForkedProcess([procfs_cgroup_path, procfs_cgroup_str]() {
    // Child process should be in same cgroup as parent.
    EXPECT_TRUE(CheckFileHasLine(procfs_cgroup_path, procfs_cgroup_str));
  });
  EXPECT_TRUE(fork_helper.WaitForChildren());

  {
    // Move current process back to the root cgroup.
    fbl::unique_fd procs_fd(open((root_path() + "/" + PROCS_FILE).c_str(), O_WRONLY));
    ASSERT_TRUE(procs_fd.is_valid());
    EXPECT_THAT(write(procs_fd.get(), pid_string.c_str(), pid_string.length()), SyscallSucceeds());
  }
}

TEST_F(CgroupTest, NewDirectoryOwnedByCreator) {
  std::string parent_path = root_path() + "/delegate_parent";
  std::string child_path = parent_path + "/child_owner";

  // Create the parent delegation cgroup (automatically cleaned up by TearDown).
  CreateCgroup(parent_path);

  // Make the parent cgroup writable by others so user 1000:1000 can mkdir inside it.
  ASSERT_THAT(chmod(parent_path.c_str(), 0777), SyscallSucceeds());

  test_helper::ForkHelper fork_helper;
  fork_helper.OnlyWaitForForkedChildren();

  fork_helper.RunInForkedProcess([child_path]() {
    // Drop privileges to UID 1000, GID 1000.
    ASSERT_THAT(setgid(1000), SyscallSucceeds());
    ASSERT_THAT(setuid(1000), SyscallSucceeds());

    // Create the child cgroup.
    ASSERT_THAT(mkdir(child_path.c_str(), 0777), SyscallSucceeds());
  });

  ASSERT_TRUE(fork_helper.WaitForChildren());

  // Verify that the new directory inherited the child creator's ownership.
  EXPECT_THAT(child_path, HasUidAndGid(1000u, 1000u));

  // Verify that the prepopulated cgroup.procs also inherited the ownership.
  std::string procs_path = child_path + "/" + PROCS_FILE;
  EXPECT_THAT(procs_path, HasUidAndGid(1000u, 1000u));

  // Clean up the child cgroup manually so that TearDown can clean up the parent.
  EXPECT_THAT(rmdir(child_path.c_str()), SyscallSucceeds());
}

TEST_F(CgroupTest, ChownDirectoryDoesNotPropagate) {
  std::string child_path = root_path() + "/child_chown";
  CreateCgroup(child_path);

  // Initially created by root (0:0).
  EXPECT_THAT(child_path, HasUidAndGid(0u, 0u));

  std::string procs_path = child_path + "/" + PROCS_FILE;
  EXPECT_THAT(procs_path, HasUidAndGid(0u, 0u));

  // Perform a non-recursive chown on the directory.
  ASSERT_THAT(chown(child_path.c_str(), 1000, 1000), SyscallSucceeds());

  // The directory ownership must change.
  EXPECT_THAT(child_path, HasUidAndGid(1000u, 1000u));

  // The prepopulated interface files inside must STILL be owned by root (0:0).
  EXPECT_THAT(procs_path, HasUidAndGid(0u, 0u));
}

class CgroupV1Test : public ::testing::Test {
 public:
  void SetUp() override {
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "requires CAP_SYS_ADMIN to mount cgroup";
    }
  }

  void TearDown() override {
    if (!test_helper::HasSysAdmin()) {
      return;
    }
    // If we moved tasks into our sub-cgroups, move ourselves back to the top-level system root
    // so that our sub-cgroups can be cleanly deleted.
    if (!system_cgroup_root_.empty()) {
      std::string pid_str = std::to_string(getpid());
      int fd = open((system_cgroup_root_ + "/tasks").c_str(), O_WRONLY);
      if (fd >= 0) {
        write(fd, pid_str.c_str(), pid_str.length());
        close(fd);
      } else {
        fd = open((system_cgroup_root_ + "/cgroup.procs").c_str(), O_WRONLY);
        if (fd >= 0) {
          write(fd, pid_str.c_str(), pid_str.length());
          close(fd);
        }
      }
    }

    for (auto path = cgroup_paths_.rbegin(); path != cgroup_paths_.rend(); path++) {
      ASSERT_THAT(rmdir(path->c_str()), SyscallSucceeds()) << "Could not delete " << *path << "";
    }
    for (auto mountpoint = cgroup_mountpoints_.rbegin(); mountpoint != cgroup_mountpoints_.rend();
         mountpoint++) {
      ASSERT_THAT(umount((mountpoint->path()).c_str()), SyscallSucceeds());
    }
  }

  std::string MountCgroupV1(const std::string& options) {
    auto& mountpoint = cgroup_mountpoints_.emplace_back();
    if (mount("none", mountpoint.path().c_str(), "cgroup", 0, options.c_str()) == -1) {
      if (errno == EBUSY) {
        // The controller is already mounted by the system. Let's discover where it is mounted.
        std::ifstream mounts("/proc/mounts");
        std::string line;
        std::string existing_mount;
        while (std::getline(mounts, line)) {
          std::istringstream iss(line);
          std::string spec, path, type, opts;
          if (iss >> spec >> path >> type >> opts) {
            if (type == "cgroup") {
              std::istringstream opts_stream(opts);
              std::string opt;
              while (std::getline(opts_stream, opt, ',')) {
                if (opt == options) {
                  existing_mount = path;
                  break;
                }
              }
            }
          }
          if (!existing_mount.empty())
            break;
        }

        if (existing_mount.empty()) {
          if (options == "cpu" && std::filesystem::exists("/sys/fs/cgroup/cpu")) {
            existing_mount = "/sys/fs/cgroup/cpu";
          }
        }

        if (existing_mount.empty()) {
          cgroup_mountpoints_.pop_back();
          return "";
        }

        if (!existing_mount.empty()) {
          // We found the system's mount point. Remove our ScopedTempDir since we won't use it.
          cgroup_mountpoints_.pop_back();
          system_cgroup_root_ = existing_mount;

          // Create a dedicated sub-cgroup inside the system's mount to serve as our isolated test
          // root.
          std::string test_root = existing_mount + "/starnix_test_" + std::to_string(getpid());
          if (mkdir(test_root.c_str(), 0777) == -1 && errno != EEXIST) {
            ADD_FAILURE() << "Could not create sub-cgroup " << test_root << ": " << strerror(errno);
          } else {
            cgroup_paths_.push_back(test_root);
            return test_root;
          }
        }
      }
      ADD_FAILURE() << "mount failed with " << strerror(errno);
      return mountpoint.path();
    }
    return mountpoint.path();
  }

  void CreateCgroup(std::string path) {
    ASSERT_THAT(mkdir(path.c_str(), 0777), SyscallSucceeds()) << "Could not create " << path;
    cgroup_paths_.push_back(std::move(path));
  }

 private:
  std::vector<std::string> cgroup_paths_;
  std::vector<test_helper::ScopedTempDir> cgroup_mountpoints_;
  std::string system_cgroup_root_;
};

TEST_F(CgroupV1Test, MountAndBasicFiles) {
  std::string root = MountCgroupV1("none,name=starnix_test");
  if (root.empty()) {
    GTEST_SKIP() << "cgroup v1 is unavailable on this Linux system";
    return;
  }

  struct stat buffer;
  ASSERT_THAT(stat((root + "/tasks").c_str(), &buffer), SyscallSucceeds());
  ASSERT_THAT(stat((root + "/cgroup.procs").c_str(), &buffer), SyscallSucceeds());

  // Create child cgroup.
  std::string child = root + "/child";
  CreateCgroup(child);

  ASSERT_THAT(stat((child + "/tasks").c_str(), &buffer), SyscallSucceeds());
  ASSERT_THAT(stat((child + "/cgroup.procs").c_str(), &buffer), SyscallSucceeds());
}

TEST_F(CgroupV1Test, MoveTaskAndProcfs) {
  std::string root = MountCgroupV1("none,name=starnix_test");
  if (root.empty()) {
    GTEST_SKIP() << "cgroup v1 is unavailable on this Linux system";
    return;
  }
  std::string child = root + "/child";
  CreateCgroup(child);

  std::string pid_str = std::to_string(getpid());

  // Move self to child cgroup.
  std::ofstream tasks_file(child + "/tasks");
  tasks_file << pid_str;
  tasks_file.close();

  // Check /proc/self/cgroup.
  std::ifstream cgroup_file("/proc/self/cgroup");
  std::string line;
  bool found = false;
  while (std::getline(cgroup_file, line)) {
    if (line.find("/child") != std::string::npos) {
      found = true;
      break;
    }
  }
  EXPECT_TRUE(found) << "Could not find /child cgroup entry in /proc/self/cgroup";

  // Move self back to root cgroup.
  std::ofstream root_tasks_file(root + "/tasks");
  root_tasks_file << pid_str;
  root_tasks_file.close();
}

TEST_F(CgroupV1Test, InvalidPidFails) {
  std::string root = MountCgroupV1("none,name=starnix_test");
  if (root.empty()) {
    GTEST_SKIP() << "cgroup v1 is unavailable on this Linux system";
    return;
  }

  int fd = open((root + "/tasks").c_str(), O_WRONLY);
  ASSERT_GE(fd, 0);
  EXPECT_THAT(write(fd, "99999999\n", 9), SyscallFailsWithErrno(ESRCH));
  close(fd);

  fd = open((root + "/cgroup.procs").c_str(), O_WRONLY);
  ASSERT_GE(fd, 0);
  EXPECT_THAT(write(fd, "99999999\n", 9), SyscallFailsWithErrno(ESRCH));
  close(fd);
}

TEST_F(CgroupV1Test, RmdirWithTasksFails) {
  std::string root = MountCgroupV1("none,name=starnix_test");
  if (root.empty()) {
    GTEST_SKIP() << "cgroup v1 is unavailable on this Linux system";
    return;
  }
  std::string child = root + "/child";
  CreateCgroup(child);

  std::string pid_str = std::to_string(getpid());

  int fd = open((child + "/tasks").c_str(), O_WRONLY);
  ASSERT_GE(fd, 0);
  EXPECT_EQ(write(fd, pid_str.c_str(), pid_str.length()), static_cast<ssize_t>(pid_str.length()));
  close(fd);

  EXPECT_THAT(rmdir(child.c_str()), SyscallFailsWithErrno(EBUSY));

  // Move back to root so teardown can succeed.
  fd = open((root + "/tasks").c_str(), O_WRONLY);
  ASSERT_GE(fd, 0);
  EXPECT_EQ(write(fd, pid_str.c_str(), pid_str.length()), static_cast<ssize_t>(pid_str.length()));
  close(fd);
}

TEST_F(CgroupV1Test, MultiMountsWithDifferentNaming) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "requires CAP_SYS_ADMIN to mount cgroup";
  }

  test_helper::ScopedTempDir mnt_foo;
  test_helper::ScopedTempDir mnt_bar;

  std::string opt_foo = "none,name=hierarchy_foo";
  std::string opt_bar = "none,name=hierarchy_bar";

  // 1. Mount hierarchy_foo.
  EXPECT_THAT(mount("none", mnt_foo.path().c_str(), "cgroup", 0, opt_foo.c_str()),
              SyscallSucceeds());

  // 2. Mount hierarchy_bar.
  EXPECT_THAT(mount("none", mnt_bar.path().c_str(), "cgroup", 0, opt_bar.c_str()),
              SyscallSucceeds());

  // 3. Verify distinctness: create a group in foo, ensure it does not appear in bar.
  std::string subgroup_foo = mnt_foo.path() + "/subgroup_only_in_foo";
  EXPECT_THAT(mkdir(subgroup_foo.c_str(), 0777), SyscallSucceeds());

  struct stat st;
  std::string should_not_exist_in_bar = mnt_bar.path() + "/subgroup_only_in_foo";
  EXPECT_THAT(stat(should_not_exist_in_bar.c_str(), &st), SyscallFailsWithErrno(ENOENT));

  // Clean up subgroup in foo before umounting.
  EXPECT_THAT(rmdir(subgroup_foo.c_str()), SyscallSucceeds());

  umount(mnt_bar.path().c_str());
  umount(mnt_foo.path().c_str());
}

TEST_F(CgroupV1Test, CpusetController) {
  std::string root = MountCgroupV1("cpuset");
  if (root.empty()) {
    GTEST_SKIP() << "cgroup v1 cpuset is unavailable on this Linux system";
    return;
  }

  std::string cpuset_suffix = "/cpus";
  struct stat buffer;
  if (stat((root + "/cpus").c_str(), &buffer) != 0) {
    if (stat((root + "/cpuset.cpus").c_str(), &buffer) == 0) {
      cpuset_suffix = "/cpuset.cpus";
    } else {
      FAIL() << "Neither cpus nor cpuset.cpus exists in " << root;
    }
  }

  ASSERT_THAT(stat((root + cpuset_suffix).c_str(), &buffer), SyscallSucceeds());

  // Read default CPUs. It should not be empty.
  std::ifstream cpus_file(root + cpuset_suffix);
  std::string cpus_str;
  std::getline(cpus_file, cpus_str);
  cpus_file.close();
  EXPECT_FALSE(cpus_str.empty());

  // Create child cgroup.
  std::string child = root + "/child";
  CreateCgroup(child);
  ASSERT_THAT(stat((child + cpuset_suffix).c_str(), &buffer), SyscallSucceeds());

  // Write to child cpus.
  std::ofstream child_cpus_file_write(child + cpuset_suffix);
  child_cpus_file_write << "0";
  child_cpus_file_write.close();

  // Read back child cpus.
  std::ifstream child_cpus_file_read(child + cpuset_suffix);
  std::string child_cpus_str;
  std::getline(child_cpus_file_read, child_cpus_str);
  child_cpus_file_read.close();
  EXPECT_EQ(child_cpus_str, "0");

  // Write invalid CPU (assuming we don't have 9999 CPUs).
  int fd = open((child + cpuset_suffix).c_str(), O_WRONLY);
  ASSERT_GE(fd, 0);
  EXPECT_THAT(write(fd, "9999\n", 5), SyscallFailsWithErrno(ERANGE));

  // Write invalid CPU range where start > end.
  EXPECT_THAT(write(fd, "2-1\n", 4), SyscallFailsWithErrno(EINVAL));

  // Write invalid CPU range where end is out of range.
  EXPECT_THAT(write(fd, "0-9999\n", 7), SyscallFailsWithErrno(ERANGE));
  close(fd);

  // Write valid range "0-0".
  {
    std::ofstream file(child + cpuset_suffix);
    file << "0-0\n";
  }
  {
    std::ifstream file(child + cpuset_suffix);
    std::string val;
    std::getline(file, val);
    EXPECT_EQ(val, "0");
  }

  // Write valid multi-CPU range if system has multiple CPUs.
  long num_cpus = sysconf(_SC_NPROCESSORS_CONF);
  if (num_cpus >= 2) {
    {
      std::ofstream file(child + cpuset_suffix);
      file << "0-1\n";
    }
    {
      std::ifstream file(child + cpuset_suffix);
      std::string val;
      std::getline(file, val);
      EXPECT_EQ(val, "0-1");
    }
  }
}
