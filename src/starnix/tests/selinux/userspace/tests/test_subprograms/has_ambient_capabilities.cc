// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/prctl.h>

#include <cstdlib>
#include <set>

#include <gtest/gtest.h>
#include <linux/capability.h>

// Verifies that the process's ambient capability set matches the capability numbers in argv.
int main(int argc, char** argv) {
  std::set<int> actual;
  for (int cap = 0; cap <= CAP_LAST_CAP; cap++) {
    int res = prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, cap, 0, 0);
    EXPECT_GE(res, 0);
    if (res == 1) {
      actual.insert(cap);
    }
  }

  std::set<int> expected;
  for (int i = 1; i < argc; i++) {
    char* end = nullptr;
    long cap = std::strtol(argv[i], &end, 10);
    EXPECT_NE(end, argv[i]);
    EXPECT_EQ(*end, '\0');
    expected.insert(static_cast<int>(cap));
  }

  EXPECT_EQ(actual, expected);

  return ::testing::Test::HasFailure() ? 1 : 0;
}
