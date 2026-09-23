// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/syscall.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "minimal_policy"; }

namespace {

TEST(SysPowerTest, WriteWakeLockRequiresCapBlockSuspend) {
  if (!test_helper::HasCapability(CAP_BLOCK_SUSPEND)) {
    GTEST_SKIP() << "Need CAP_BLOCK_SUSPEND to run this test";
  }

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    test_helper::UnsetCapabilityEffective(CAP_BLOCK_SUSPEND);

    fbl::unique_fd fd(SAFE_SYSCALL(open("/sys/power/wake_lock", O_WRONLY)));
    const char *val = "test_lock\n";
    EXPECT_THAT(write(fd.get(), val, strlen(val)), SyscallFailsWithErrno(EPERM));
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

TEST(SysPowerTest, WriteWakeUnlockRequiresCapBlockSuspend) {
  if (!test_helper::HasCapability(CAP_BLOCK_SUSPEND)) {
    GTEST_SKIP() << "Need CAP_BLOCK_SUSPEND to run this test";
  }

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&]() {
    test_helper::UnsetCapabilityEffective(CAP_BLOCK_SUSPEND);

    fbl::unique_fd fd(SAFE_SYSCALL(open("/sys/power/wake_unlock", O_WRONLY)));
    const char *val = "test_lock\n";
    EXPECT_THAT(write(fd.get(), val, strlen(val)), SyscallFailsWithErrno(EPERM));
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

}  // namespace
