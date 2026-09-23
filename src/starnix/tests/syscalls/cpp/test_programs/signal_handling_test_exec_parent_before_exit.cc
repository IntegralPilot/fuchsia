// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <signal.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

void expectSIGCHLD(int argc, char** argv) {
  ASSERT_EQ(argc, 3);

  // Block signals so as not to miss when they arrive
  sigset_t block_mask, old_mask;
  sigemptyset(&block_mask);
  sigaddset(&block_mask, SIGUSR1);
  sigaddset(&block_mask, SIGCHLD);
  ASSERT_THAT(sigprocmask(SIG_BLOCK, &block_mask, &old_mask), SyscallSucceeds());

  // Tell child to exit
  int event_fd = atoi(argv[1]);
  int child_pid = atoi(argv[2]);
  int64_t val = 1;
  ASSERT_THAT(write(event_fd, &val, sizeof(int64_t)), SyscallSucceeds());

  // Wait for child to exit
  int status;
  ASSERT_THAT(waitpid(child_pid, &status, __WALL), SyscallSucceeds());
  EXPECT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0);

  // Wait to receive either SIGUSR1 or SIGCHLD
  int sig;
  ASSERT_THAT(sigwait(&block_mask, &sig), SyscallSucceeds());

  // Get pending signals
  sigset_t pending_signals;
  ASSERT_THAT(sigpending(&pending_signals), SyscallSucceeds());

  // Cleanup
  ASSERT_THAT(sigprocmask(SIG_SETMASK, &old_mask, nullptr), SyscallSucceeds());
  close(event_fd);

  // Expect SIGCHLD was received and SIGUSR1 was not received
  EXPECT_EQ(sig, SIGCHLD);
  EXPECT_FALSE(sigismember(&pending_signals, SIGUSR1));
}

int main(int argc, char** argv) {
  expectSIGCHLD(argc, argv);
  return ::testing::Test::HasFailure() ? 1 : 0;
}
