// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/resource.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

void CheckRlimits(bool expect_soft_rlimit_reset, rlim_t parent_soft_limit,
                  rlim_t parent_hard_limit) {
  struct rlimit child_rlim;
  ASSERT_THAT(getrlimit(RLIMIT_FSIZE, &child_rlim), SyscallSucceeds());

  if (expect_soft_rlimit_reset) {
    struct rlimit init_rlim;
    // Get the init task's rlimit and compute the expected post-reset value for the child's soft
    // rlimit.
    ASSERT_THAT(prlimit(1, RLIMIT_FSIZE, nullptr, &init_rlim), SyscallSucceeds());
    EXPECT_EQ(child_rlim.rlim_cur, std::min(parent_hard_limit, init_rlim.rlim_cur));
  } else {
    EXPECT_EQ(child_rlim.rlim_cur, parent_soft_limit);
  }
  // The hard limit should be inherited from the parent in both cases.
  EXPECT_EQ(child_rlim.rlim_max, parent_hard_limit);
}

// Checks whether the task's RLIMIT_FSIZE soft resource limit was reset during `exec`.
//
// Usage:
//   is_soft_rlimit_reset <expect_reset> <parent_soft_limit> <parent_hard_limit>
//
// Arguments:
//   - expect_soft_limit_reset: true if the soft rlimit is expected to be reset, false if inherited.
//   - parent_soft_limit: the parent task's current limit for the RLIMIT_FSIZE rlimit.
//   - parent_hard_limit: the parent task's max limit for the RLIMIT_FSIZE rlimit.
//
// If `expect_soft_limit_reset` is true, checks that the current task's soft limit is equal to the
// minimum of the parent task's hard limit and the init task's soft limit.
//
// If `expect_soft_limit_reset` is false, checks that the current task's soft limit is equal to the
// parent task's soft limit. In both cases, checks that the current task's hard limit is equal to
// the parent's hard limit.
int main(int argc, char** argv) {
  EXPECT_EQ(argc, 4);
  if (::testing::Test::HasFailure()) {
    return 1;
  }
  bool expect_soft_rlimit_reset = std::atoi(argv[1]);
  rlim_t parent_soft_limit = std::atol(argv[2]);
  rlim_t parent_hard_limit = std::atol(argv[3]);

  CheckRlimits(expect_soft_rlimit_reset, parent_soft_limit, parent_hard_limit);

  return ::testing::Test::HasFailure() ? 1 : 0;
}
