// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

extern std::string DoPrePolicyLoadWork() { return "task_policy"; }

namespace {

// `getsid(0)` reports the caller's own session, so no permission check applies,
// even for a domain that is denied the `getsession` permission on itself.
// Passing an explicit PID does require the permission, and is denied.
TEST(TaskTest, GetSidZeroSkipsGetsessionCheck) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_task_no_getsession_self_t:s0", [&] {
    EXPECT_THAT(getsid(getpid()), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(getsid(0), SyscallSucceeds());
  }));
}

}  // namespace
