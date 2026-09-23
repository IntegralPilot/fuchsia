// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "stdio.h"
#include "stdlib.h"

int main(int argc, char** argv) {
  int test_fd = atoi(argv[1]);
  bool expect_null_inode = atoi(argv[2]);

  int null_inode_fd = open("/sys/fs/selinux/null", 0, O_RDONLY);
  EXPECT_TRUE(null_inode_fd > 0);

  EXPECT_THAT(IsSameInode(test_fd, null_inode_fd), SyscallResultIsOk(expect_null_inode));
  return ::testing::Test::HasFailure() ? 1 : 0;
}
