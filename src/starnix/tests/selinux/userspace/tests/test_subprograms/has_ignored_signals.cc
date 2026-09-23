// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <signal.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

void ValidateSigactions(std::set<int>& expect_ignored) {
  for (int signum = 1; signum < NSIG; signum++) {
    struct sigaction action{};
    int sigaction_result = sigaction(signum, nullptr, &action);

    // If the signal number is in the `expect_ignored` set, then expect
    // `sigaction` to succeed. Otherwise, allow it to fail with EINVAL.
    //
    // If `sigaction` succeeded, expect signals whose numbers were passed
    // as arguments to be ignored. Expect all other signals to have their
    // default handlers.
    if (expect_ignored.contains(signum)) {
      ASSERT_EQ(sigaction_result, 0) << "signal number: " << signum;
      EXPECT_EQ(action.sa_handler, SIG_IGN) << "signal number: " << signum;
    } else {
      ASSERT_TRUE(sigaction_result == 0 || errno == EINVAL) << "signal number: " << signum;
      if (sigaction_result == 0) {
        EXPECT_EQ(action.sa_handler, SIG_DFL) << "signal number: " << signum;
      }
    }
  }
}

int main(int argc, char** argv) {
  std::set<int> expect_ignored;
  for (int i = 1; i < argc; i++) {
    expect_ignored.insert(std::atoi(argv[i]));
  }
  ValidateSigactions(expect_ignored);

  return ::testing::Test::HasFailure() ? 1 : 0;
}
