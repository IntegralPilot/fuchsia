// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/syscall.h>

#include <gtest/gtest.h>
#include <linux/kcmp.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

int main(int argc, char** argv) {
  int fd_1 = atoi(argv[1]);
  int fd_2 = atoi(argv[2]);

  int pid = getpid();
  // Use 'KCMP_FILE' to check that the two file descriptors are duplicates of
  // one another; i.e., that they refer to the same underlying file description.
  EXPECT_THAT(syscall(SYS_kcmp, pid, pid, KCMP_FILE, fd_1, fd_2), SyscallSucceedsWithValue(0));

  return ::testing::Test::HasFailure() ? 1 : 0;
}
