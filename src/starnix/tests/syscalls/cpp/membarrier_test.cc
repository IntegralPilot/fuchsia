// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/syscall.h>
#include <unistd.h>

#include <gtest/gtest.h>
#include <linux/membarrier.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

// glibc does not provide a wrapper for this system call.
long membarrier(int cmd, unsigned int flags, int cpu_id) {
  return syscall(SYS_membarrier, cmd, flags, cpu_id);
}

TEST(Membarrier, Global) {
  // The (legacy) global barrier command does not require registration.
  SAFE_SYSCALL(membarrier(MEMBARRIER_CMD_GLOBAL, 0, 0));
}

TEST(Membarrier, Query) {
  // Check that the set of supported commands includes what Starnix supports.
  long cmds = SAFE_SYSCALL(membarrier(MEMBARRIER_CMD_QUERY, 0, 0));
  EXPECT_NE(cmds & MEMBARRIER_CMD_GLOBAL, 0);
  EXPECT_NE(cmds & MEMBARRIER_CMD_GLOBAL_EXPEDITED, 0);
  EXPECT_NE(cmds & MEMBARRIER_CMD_REGISTER_GLOBAL_EXPEDITED, 0);
  EXPECT_NE(cmds & MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0);
  EXPECT_NE(cmds & MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED, 0);
  EXPECT_NE(cmds & MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE, 0);
  EXPECT_NE(cmds & MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE, 0);
}
TEST(Membarrier, PrivateExpedited) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    // Expect this to find no registered processes and fail.
    EXPECT_EQ(membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0, 0), -1);
    EXPECT_EQ(errno, EPERM) << strerror(errno);

    // Register.
    SAFE_SYSCALL(membarrier(MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED, 0, 0));

    // Now a barrier should succeed.
    SAFE_SYSCALL(membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0, 0));
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Membarrier, PrivateExpeditedRegisterThroughFork) {
  // Run this test inside a child process to avoid polluting this process' state for other test
  // cases.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    // Register.
    SAFE_SYSCALL(membarrier(MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED, 0, 0));

    // Fork a child process within this child..
    test_helper::ForkHelper child_helper;
    child_helper.RunInForkedProcess([]() {
      // This succeeds as the registration carries through fork.
      EXPECT_EQ(membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0, 0), 0);
    });
    ASSERT_TRUE(child_helper.WaitForChildren());
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Membarrier, PrivateRegisterMemorySendSyncCore) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    // Register for private, expedited (memory) barriers.
    SAFE_SYSCALL(membarrier(MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED, 0, 0));

    // SYNC_CORE barriers fail since nothing is registered for this type.
    EXPECT_EQ(membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED_SYNC_CORE, 0, 0), -1);
    EXPECT_EQ(errno, EPERM);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Membarrier, PrivateRegisterSyncCoreSendMemory) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    // Register for private, expedited sync_core barriers.
    SAFE_SYSCALL(membarrier(MEMBARRIER_CMD_REGISTER_PRIVATE_EXPEDITED_SYNC_CORE, 0, 0));

    // SYNC_CORE barriers fail since nothing is registered for this type.
    EXPECT_EQ(membarrier(MEMBARRIER_CMD_PRIVATE_EXPEDITED, 0, 0), -1);
    EXPECT_EQ(errno, EPERM);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

}  // namespace
