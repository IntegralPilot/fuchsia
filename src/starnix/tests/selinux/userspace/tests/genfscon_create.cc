// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/mount.h>
#include <sys/sysmacros.h>
#include <syscall.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace {

// The FUSE implementation will write to this fd to signal readiness.
constexpr int kFuseReadyFd = 3;

// Returns the path to a binary under the test package's `data` directory.
std::string PathForExec(std::string_view binary_name) {
  return "data/bin/" + std::string(binary_name);
}

TEST(GenfsconCreateTest, Create) {
  // Pipe used to confirm that the filesystem is ready.
  int pipe_ends[2];
  SAFE_SYSCALL(pipe(pipe_ends));
  fbl::unique_fd pipe_in(pipe_ends[1]);
  fbl::unique_fd pipe_out(pipe_ends[0]);

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&] {
    std::string binary_name = "fuse_memfs_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string fuse_dev = "/dev_fuse";
    std::string path = "/fuse";

    // The fuse_memfs program writes to fd 3 to signal that the fs is ready.
    SAFE_SYSCALL(dup2(pipe_in.get(), kFuseReadyFd));
    char* const args[] = {binary_name.data(), fuse_dev.data(), path.data(), nullptr};
    SAFE_SYSCALL(execv(path_for_exec.c_str(), args));
  });

  char buf;
  ASSERT_THAT(read(pipe_out.get(), &buf, 1), SyscallSucceedsWithValue(1));

  auto enforce = ScopedEnforcement::SetEnforcing();
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:genfscon_create_t:s0", [&] {
    int fd;
    EXPECT_THAT((fd = open("/fuse/test", O_WRONLY | O_CREAT, 0755)), SyscallSucceeds());
    EXPECT_THAT(GetLabel(fd), "system_u:object_r:fuse_t:s0");
    close(fd);
    EXPECT_THAT(GetLabel("/fuse/test"), "system_u:object_r:fuse_t:s0");
  }));

  // The child process serving the FUSE filesystem will exit on umount.
  SAFE_SYSCALL(umount("/fuse"));
}

TEST(GenfsconCreateTest, FsCreateCon) {
  // Pipe used to confirm that the filesystem is ready.
  int pipe_ends[2];
  SAFE_SYSCALL(pipe(pipe_ends));
  fbl::unique_fd pipe_in(pipe_ends[1]);
  fbl::unique_fd pipe_out(pipe_ends[0]);

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&] {
    std::string binary_name = "fuse_memfs_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string fuse_dev = "/dev_fuse";
    std::string path = "/fuse";

    // The fuse_memfs program writes to fd 3 to signal that the fs is ready.
    SAFE_SYSCALL(dup2(pipe_in.get(), kFuseReadyFd));
    char* const args[] = {binary_name.data(), fuse_dev.data(), path.data(), nullptr};
    SAFE_SYSCALL(execv(path_for_exec.c_str(), args));
  });

  char buf;
  ASSERT_THAT(read(pipe_out.get(), &buf, 1), SyscallSucceedsWithValue(1));

  auto enforce = ScopedEnforcement::SetEnforcing();
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:genfscon_create_t:s0", [&] {
    auto fscreate = ScopedTaskAttrResetter::SetTaskAttr(
        "fscreate", "test_u:object_r:genfscon_fscreate_file_t:s0");
    int fd;
    EXPECT_THAT((fd = open("/fuse/test", O_WRONLY | O_CREAT, 0755)), SyscallSucceeds());
    EXPECT_THAT(GetLabel(fd), "system_u:object_r:fuse_t:s0");
    close(fd);
    EXPECT_THAT(GetLabel("/fuse/test"), "system_u:object_r:fuse_t:s0");
  }));

  // The child process serving the FUSE filesystem will exit on umount.
  SAFE_SYSCALL(umount("/fuse"));
}

}  // namespace

extern std::string DoPrePolicyLoadWork() {
  EXPECT_THAT(mknod("/dev_fuse", S_IFCHR | 0666, makedev(10, 229)), SyscallSucceeds());
  EXPECT_THAT(mkdir("/fuse", 0755), SyscallSucceeds());
  return "genfscon_create_policy";
}
