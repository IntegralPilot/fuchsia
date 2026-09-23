// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <errno.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/reboot.h>

#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

TEST(RebootTest, RebootCMDRestart2FailsWithInvalidPointer) {
  if (!test_helper::HasCapability(CAP_SYS_BOOT)) {
    GTEST_SKIP() << "Not running with reboot capabilities. skipping.";
  }

  EXPECT_THAT(syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
                      LINUX_REBOOT_CMD_RESTART2, nullptr),
              SyscallFailsWithErrno(EFAULT));
}

TEST(RebootTest, RebootFailsWithEpermIfMissingCap) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with root capabilities. skipping.";
  }

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([]() {
    test_helper::UnsetCapability(CAP_SYS_BOOT);

    EXPECT_THAT(syscall(SYS_reboot, LINUX_REBOOT_MAGIC1, LINUX_REBOOT_MAGIC2,
                        LINUX_REBOOT_CMD_RESTART, nullptr),
                SyscallFailsWithErrno(EPERM));
  });
}

}  // namespace
