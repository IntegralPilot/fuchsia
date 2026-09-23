// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <signal.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

void ValidateSigactions(int argc, char** argv) {
  ASSERT_EQ(argc % 2, 1);

  std::vector<int> signums;
  std::vector<struct sigaction> sigactions;
  std::vector<sighandler_t> expectations;

  for (int i = 1; i < (argc - 1); i += 2) {
    int signum = std::atoi(argv[i]);
    signums.push_back(signum);

    struct sigaction sa{};
    ASSERT_THAT(sigaction(signum, nullptr, &sa), SyscallSucceeds()) << "signal number: " << signum;
    sigactions.push_back(sa);

    std::string action = std::string(argv[i + 1]);
    ASSERT_TRUE(action == "default" || action == "ignore") << "signal number: " << signum;
    sighandler_t expectation = action == "default" ? SIG_DFL : SIG_IGN;
    expectations.push_back(expectation);
  }

  for (int i = 0; i < (argc / 2); i++) {
    EXPECT_EQ(sigactions[i].sa_handler, expectations[i]) << "signal number: " << signums[i];
    EXPECT_EQ(sigactions[i].sa_flags, 0) << "signal number: " << signums[i];
    EXPECT_EQ(sigismember(&sigactions[i].sa_mask, SIGINT), 0) << "signal number: " << signums[i];
  }
}

// For a given list of signal numbers, checks whether the signal disposition
// for each one is SIG_DFL or SIG_IGN.
//
// Arguments should be provided in pairs, with each signal number followed by
// either "default" or "ignore". Signal action flags and masks should be cleared.
int main(int argc, char** argv) {
  ValidateSigactions(argc, argv);

  return ::testing::Test::HasFailure() ? 1 : 0;
}
