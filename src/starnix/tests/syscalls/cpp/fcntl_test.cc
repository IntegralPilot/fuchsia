// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <stdio.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/types.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

bool CheckLock(int fd, short type, off_t start, off_t length, pid_t pid) {
  test_helper::ForkHelper helper;
  // Fork a process to be able to check the state of locks in fd.
  helper.RunInForkedProcess([&] {
    struct flock fl;
    fl.l_type = F_WRLCK;
    fl.l_whence = SEEK_SET;
    fl.l_start = start;
    fl.l_len = length;
    SAFE_SYSCALL(fcntl(fd, F_GETLK, &fl));

    ASSERT_EQ(fl.l_type, type);
    if (type != F_UNLCK) {
      ASSERT_EQ(fl.l_whence, SEEK_SET);
      ASSERT_EQ(fl.l_start, start);
      ASSERT_EQ(fl.l_len, length);
      ASSERT_EQ(fl.l_pid, pid);
    }
  });
  return helper.WaitForChildren();
}

// Open a file to test. It will be of size 3000, and the position will be at
// 2000.
test_helper::ScopedTempFD OpenTestFile() {
  test_helper::ScopedTempFD temp_file;
  EXPECT_TRUE(temp_file.is_valid());
  int fd = temp_file.fd();
  SAFE_SYSCALL(lseek(fd, 2999, SEEK_SET));
  // Make the file 3000 bytes longs
  SAFE_SYSCALL(write(fd, &fd, 1));
  // Move to 2000
  SAFE_SYSCALL(lseek(fd, 2000, SEEK_SET));
  return temp_file;
}

// Test that exiting a processes releases locks on a file.
TEST(FcntlLockTest, ChildProcessReleaseLock) {
  for (int i = 0; i < 10; ++i) {
    test_helper::ForkHelper helper;
    helper.RunInForkedProcess([] {
      auto temp_file = OpenTestFile();
      int fd = temp_file.fd();

      struct flock fl;
      fl.l_type = F_WRLCK;
      fl.l_whence = SEEK_SET;
      fl.l_start = 0;
      fl.l_len = 3000;
      // This should succeed since the previous process that held the lock exited (as reported by
      // wait(2)) and thus should no longer be holding a lock on the file.
      SAFE_SYSCALL(fcntl(fd, F_SETLK, &fl));
    });
  }
}

TEST(FcntlLockTest, ReleaseLockInMiddleOfAnotherLock) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    auto temp_file = OpenTestFile();
    int fd = temp_file.fd();

    struct flock fl;
    fl.l_type = F_WRLCK;
    fl.l_whence = SEEK_CUR;
    fl.l_start = -2000;
    fl.l_len = 3000;
    SAFE_SYSCALL(fcntl(fd, F_SETLK, &fl));

    fl.l_type = F_UNLCK;
    fl.l_whence = SEEK_END;
    fl.l_start = -2000;
    fl.l_len = 1000;
    SAFE_SYSCALL(fcntl(fd, F_SETLK, &fl));

    // Check that we have a lock between [0, 1000[ and [2000, 3000[.
    ASSERT_TRUE(CheckLock(fd, F_WRLCK, 0, 1000, getpid()));
    ASSERT_TRUE(CheckLock(fd, F_UNLCK, 1000, 1000, 0));
    ASSERT_TRUE(CheckLock(fd, F_WRLCK, 2000, 1000, getpid()));
  });
}

TEST(FcntlLockTest, ChangeLockTypeInMiddleOfAnotherLock) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    auto temp_file = OpenTestFile();
    int fd = temp_file.fd();

    struct flock fl;
    fl.l_type = F_WRLCK;
    fl.l_whence = SEEK_SET;
    fl.l_start = 0;
    fl.l_len = 3000;
    SAFE_SYSCALL(fcntl(fd, F_SETLK, &fl));

    fl.l_type = F_RDLCK;
    fl.l_whence = SEEK_END;
    fl.l_start = -2000;
    fl.l_len = 1000;
    SAFE_SYSCALL(fcntl(fd, F_SETLK, &fl));

    // Check that we have a write lock between [0, 1000[ and [2000, 3000[ and a
    // read lock between [1000, 2000[.
    ASSERT_TRUE(CheckLock(fd, F_WRLCK, 0, 1000, getpid()));
    ASSERT_TRUE(CheckLock(fd, F_RDLCK, 1000, 1000, getpid()));
    ASSERT_TRUE(CheckLock(fd, F_WRLCK, 2000, 1000, getpid()));
  });
}

TEST(FcntlLockTest, CloneFiles) {
  // Do all the test in another process, as it will requires closing the parent
  // process before the child one.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    auto temp_file = OpenTestFile();
    int fd = temp_file.fd();
    pid_t pid = getpid();

    // Lock the file.
    struct flock fl;
    fl.l_type = F_WRLCK;
    fl.l_whence = SEEK_SET;
    fl.l_start = 0;
    fl.l_len = 0;
    SAFE_SYSCALL(fcntl(fd, F_SETLK, &fl));

    // Clone the process, with CLONE_FILES
    int flags = CLONE_FILES | SIGCHLD;
    if (SAFE_SYSCALL(syscall(SYS_clone, flags, nullptr, nullptr, nullptr, nullptr)) > 0) {
      // Parent immediately exit.
      _exit(testing::Test::HasFailure());
    }

    // The child is a new process but with the exact same file table as its
    // parent.
    ASSERT_NE(getpid(), pid);
    // Wait for our parent to finish.
    while (getppid() == pid) {
      usleep(1000);
    }

    // Fork a process to be able to check the state of locks in fd. The returned
    // pid is expected to be the one of the now dead process.
    ASSERT_TRUE(CheckLock(fd, F_WRLCK, 0, 0, pid));

    fbl::unique_fd new_fd(dup(fd));
    // Closing fd should release the lock.
    temp_file.fd_.reset();
    ASSERT_TRUE(CheckLock(new_fd.get(), F_UNLCK, 0, 0, 0));
  });
}

TEST(FcntlLockTest, CheckErrors) {
  auto temp_file = OpenTestFile();
  int fd = temp_file.fd();

  struct flock fl;
  fl.l_type = 42;
  fl.l_whence = SEEK_SET;
  fl.l_start = 0;
  fl.l_len = 0;

  ASSERT_EQ(fcntl(fd, F_SETLK, &fl), -1);
  ASSERT_EQ(errno, EINVAL);

  fl.l_type = F_WRLCK;
  fl.l_whence = 42;

  ASSERT_EQ(fcntl(fd, F_SETLK, &fl), -1);
  ASSERT_EQ(errno, EINVAL);

  fl.l_type = F_WRLCK;
  fl.l_whence = SEEK_END;
  fl.l_start = std::numeric_limits<decltype(fl.l_start)>::max();
  fl.l_len = 0;

  ASSERT_EQ(fcntl(fd, F_SETLK, &fl), -1);
  ASSERT_EQ(errno, EOVERFLOW);

  fl.l_type = F_WRLCK;
  fl.l_whence = SEEK_END;
  fl.l_start = std::numeric_limits<decltype(fl.l_len)>::min();
  fl.l_len = std::numeric_limits<decltype(fl.l_len)>::min();

  ASSERT_EQ(fcntl(fd, F_SETLK, &fl), -1);
  ASSERT_EQ(errno, EINVAL);

  fl.l_type = F_WRLCK;
  fl.l_whence = SEEK_SET;
  fl.l_start = 0;
  fl.l_len = -1;

  ASSERT_EQ(fcntl(fd, F_SETLK, &fl), -1);
  ASSERT_EQ(errno, EINVAL);
}

TEST(FcntlTest, FdDup) {
  auto temp_file = OpenTestFile();
  int fd = temp_file.fd();

  fbl::unique_fd new_fd(SAFE_SYSCALL(fcntl(fd, F_DUPFD, 1000)));
  ASSERT_GE(new_fd.get(), 1000);
  new_fd.reset(SAFE_SYSCALL(fcntl(fd, F_DUPFD, 0)));
  ASSERT_LT(new_fd.get(), 1000);
}

TEST(FcntlTest, SetFdAfterFdDup) {
  auto temp_file = OpenTestFile();
  int fd = temp_file.fd();
  fbl::unique_fd new_fd(SAFE_SYSCALL(dup(fd)));

  int new_fd_flags_before = SAFE_SYSCALL(fcntl(new_fd.get(), F_GETFD));
  int flags = SAFE_SYSCALL(fcntl(fd, F_GETFD));
  ASSERT_EQ(SAFE_SYSCALL(fcntl(fd, F_SETFD, flags ^ FD_CLOEXEC)), 0);

  // `F_SETFD` sets per-FD flags, in contrast to `F_SETFL`, which modifies file "status" flags which
  // are per file-description, and therefore shared by duplicated files. Changing `FD_CLOEXEC` on
  // one of the duplicated descriptors has no effect on the other.
  int new_fd_flags = SAFE_SYSCALL(fcntl(new_fd.get(), F_GETFD));
  EXPECT_EQ(new_fd_flags, new_fd_flags_before);
}

TEST(FcntlTest, SetFlAfterFdDup) {
  auto temp_file = OpenTestFile();
  int fd = temp_file.fd();
  fbl::unique_fd new_fd(SAFE_SYSCALL(dup(fd)));

  int new_fd_flags_before = SAFE_SYSCALL(fcntl(new_fd.get(), F_GETFL));
  int flags = SAFE_SYSCALL(fcntl(fd, F_GETFL));
  EXPECT_EQ(new_fd_flags_before, flags);
  ASSERT_EQ(SAFE_SYSCALL(fcntl(fd, F_SETFL, flags ^ O_NONBLOCK)), 0);

  // `F_SETFL` sets per-description file "status" flags, which are common to all FDs sharing the
  // same file description. Changing `O_NONBLOCK` for an FD affects any duplicates.
  int new_fd_flags = SAFE_SYSCALL(fcntl(new_fd.get(), F_GETFL));
  EXPECT_EQ(new_fd_flags, new_fd_flags_before ^ O_NONBLOCK);
}

TEST(FcntlTest, Noatime) {
  auto temp_file = OpenTestFile();
  int fd = temp_file.fd();

  EXPECT_EQ(fcntl(fd, F_SETFL, O_NOATIME), 0);
}

TEST(FcntlTest, NoatimePermission) {
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }

  auto temp_file = OpenTestFile();
  int fd = temp_file.fd();

  // Fork to change UID.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([fd] {
    ASSERT_EQ(setuid(1), 0);

    ASSERT_LT(fcntl(fd, F_SETFL, O_NOATIME), 0);
    ASSERT_EQ(errno, EPERM);
  });
}

TEST(FcntlTest, RenameExchangeLockOrdering) {
  char *tmp = getenv("TEST_TMPDIR");
  std::string root_dir = tmp == nullptr ? "/tmp" : std::string(tmp);

  // This test exercises a niche lock ordering bug. In essence, the rename_exchange
  // operation can muddle with the lock orderning in DirEntry due to the reparenting
  // of nodes. See the following bug for a more detailed description:
  // https://buganizer.corp.google.com/issues/387576826
  std::string first_parent_dir = root_dir + "/first_parent_dir";
  std::string second_parent_dir = root_dir + "/second_parent_dir";
  std::string file = second_parent_dir + "/file";

  // Set up the initial folder and file structure.
  ASSERT_THAT(mkdir(first_parent_dir.c_str(), 0700), SyscallSucceeds());
  ASSERT_THAT(mkdir(second_parent_dir.c_str(), 0700), SyscallSucceeds());
  ASSERT_THAT(open(file.c_str(), O_CREAT | O_WRONLY, 0600), SyscallSucceeds());

  // The rename operation here is irrelevant, except in that it establishes
  // the lock ordering for the parent directories. In other words, the lock
  // ordering is set as first locking the children of "first_parent_dir,"
  // followed by locking the children of "second_parent_dir."
  std::string dummy_first_parent_child = first_parent_dir + "/dummy_file.txt";
  std::string dummy_second_parent_child = second_parent_dir + "/dummy_file.txt";
  // Since these files don't exist, we expect the rename to fail. Once again,
  // we are only doing this to establish the lock ordering for the directories.
  ASSERT_THAT(rename(dummy_first_parent_child.c_str(), dummy_second_parent_child.c_str()),
              SyscallFails());

  // Next, we'll do the rename_exchange operation. This will exchange the nested
  // file with a higher-level directory, which can potentially pollute the
  // lock tracing state of the directory hierarchy.
  ASSERT_THAT(renameat2(0, file.c_str(), 0, first_parent_dir.c_str(), RENAME_EXCHANGE),
              SyscallSucceeds());

  // Lastly, we'll attempt to touch the "first_parent_dir," which we've just
  // exchanged to be nested under "second_parent_dir." This will cause the
  // "second_parent_dir" children to be locked, followed by the "first_parent_dir"
  // that's now nested under it. This could potentially trip our lock ordering
  // which we established in the first rename operation of this test.
  std::string newly_exchanged_node = second_parent_dir + "/file";
  ASSERT_THAT(rmdir(newly_exchanged_node.c_str()), SyscallSucceeds());

  // Clean up, unlinking the `first_parent_dir` which is now a file per the RENAME_EXCHANGE.
  ASSERT_THAT(unlink(first_parent_dir.c_str()), SyscallSucceeds());
  ASSERT_THAT(rmdir(second_parent_dir.c_str()), SyscallSucceeds());
}

TEST(FcntlTest, SetOwnExPidZeroPreservesType) {
  auto temp_file = OpenTestFile();
  int fd = temp_file.fd();

  struct f_owner_ex owner;
  owner.type = F_OWNER_PID;
  owner.pid = 0;

  ASSERT_THAT(fcntl(fd, F_SETOWN_EX, &owner), SyscallSucceeds());

  struct f_owner_ex got_owner;
  memset(&got_owner, 0, sizeof(got_owner));
  ASSERT_THAT(fcntl(fd, F_GETOWN_EX, &got_owner), SyscallSucceeds());

  EXPECT_EQ(got_owner.type, F_OWNER_PID);
  EXPECT_EQ(got_owner.pid, 0);
}

TEST(FcntlTest, GetOwnExDefault) {
  auto temp_file = OpenTestFile();
  int fd = temp_file.fd();

  struct f_owner_ex owner;
  // Initialize with values to make sure they are overwritten.
  owner.type = F_OWNER_PID;
  owner.pid = -1;

  ASSERT_THAT(fcntl(fd, F_GETOWN_EX, &owner), SyscallSucceeds());

  EXPECT_EQ(owner.type, F_OWNER_TID);
  EXPECT_EQ(owner.pid, 0);
}
}  // namespace
