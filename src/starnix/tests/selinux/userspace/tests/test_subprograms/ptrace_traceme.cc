// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/ptrace.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

// Attempt to initiate tracing by the parent task, and check the result of
// the `ptrace` call against an expectation passed as an argument.
int main(int argc, char** argv) {
  bool expect_success = std::atoi(argv[1]);
  pid_t pid = 0;
  if (expect_success) {
    EXPECT_THAT(ptrace(PTRACE_TRACEME, pid, nullptr, nullptr), SyscallSucceeds());
  } else {
    EXPECT_THAT(ptrace(PTRACE_TRACEME, pid, nullptr, nullptr), SyscallFailsWithErrno(EACCES));
  }

  return ::testing::Test::HasFailure() ? 1 : 0;
}
