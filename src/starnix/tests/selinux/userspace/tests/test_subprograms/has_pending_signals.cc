// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <signal.h>

#include <set>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

// Compares a set of signal numbers passed as arguments to the set of pending
// signals.
int main(int argc, char** argv) {
  sigset_t pending;
  EXPECT_THAT(sigpending(&pending), SyscallSucceeds());
  if (::testing::Test::HasFailure()) {
    return 1;
  }

  std::set<int> actual;
  for (int i = 1; i < NSIG; i++) {
    if (sigismember(&pending, i)) {
      actual.insert(i);
    }
  }

  std::set<int> expected;
  for (int i = 1; i < argc; i++) {
    expected.insert(std::atoi(argv[i]));
  }

  EXPECT_EQ(actual, expected);

  return ::testing::Test::HasFailure() ? 1 : 0;
}
