// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <signal.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

void expectSIGUSR1(int argc, char** argv) {
  ASSERT_EQ(argc, 1);

  // Wait to receive either SIGUSR1 or SIGCHLD
  sigset_t block_mask;
  sigemptyset(&block_mask);
  sigaddset(&block_mask, SIGUSR1);
  sigaddset(&block_mask, SIGCHLD);

  int sig;
  ASSERT_THAT(sigwait(&block_mask, &sig), SyscallSucceeds());

  sigset_t pending_signals;
  ASSERT_THAT(sigpending(&pending_signals), SyscallSucceeds());

  // Clean up
  sigset_t empty_mask;
  sigemptyset(&empty_mask);
  ASSERT_THAT(sigprocmask(SIG_SETMASK, &empty_mask, nullptr), SyscallSucceeds());

  // Expect SIGUSR1 was received and SIGCHLD was not received
  EXPECT_EQ(sig, SIGUSR1);
  EXPECT_FALSE(sigismember(&pending_signals, SIGCHLD));
}

// Expects the SIGUSR1 signal, should inherit a signal mask of SIGCHLD and SIGUSR1
int main(int argc, char** argv) {
  expectSIGUSR1(argc, argv);

  return ::testing::Test::HasFailure() ? 1 : 0;
}
