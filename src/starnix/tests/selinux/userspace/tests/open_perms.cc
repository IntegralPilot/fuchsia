// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

extern std::string DoPrePolicyLoadWork() { return "open_perms_policy"; }

namespace {
constexpr char kOpenPermsPolicyCap[] = "open_perms";
constexpr char kFileLabel[] = "test_u:object_r:test_open_file_t:s0";

// Verify that open for read succeeds for a context with read and open permissions
// when the open_perms policy capability is enabled.
TEST(OpenPermsTest, ReadAllowed) {
  ASSERT_TRUE(IsPolicyCapSupported(kOpenPermsPolicyCap));
  auto test_file = ScopedTempFDWithLabel(kFileLabel);
  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_open_allow_read_open_t:s0", [&]() {
    fbl::unique_fd fd(open(test_file.name().c_str(), O_RDONLY));
    EXPECT_TRUE(fd.is_valid()) << "errno: " << errno;
  }));
}

// Verify that open for write succeeds for a context with write and open permissions
// when the open_perms policy capability is enabled.
TEST(OpenPermsTest, WriteAllowed) {
  ASSERT_TRUE(IsPolicyCapSupported(kOpenPermsPolicyCap));
  auto test_file = ScopedTempFDWithLabel(kFileLabel);
  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_open_allow_write_open_t:s0", [&]() {
    fbl::unique_fd fd(open(test_file.name().c_str(), O_WRONLY));
    EXPECT_TRUE(fd.is_valid()) << "errno: " << errno;
  }));
}

// Verify that open for read fails for a context with read permission, but without open
// permission, when the open_perms policy capability is enabled.
TEST(OpenPermsTest, ReadDenied) {
  ASSERT_TRUE(IsPolicyCapSupported(kOpenPermsPolicyCap));
  auto test_file = ScopedTempFDWithLabel(kFileLabel);
  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_open_allow_read_deny_open_t:s0", [&]() {
    EXPECT_THAT(open(test_file.name().c_str(), O_RDONLY), SyscallFailsWithErrno(EACCES));
  }));
}

// Verify that open for write fails for a context with write permission, but without open
// permission, when the open_perms policy capability is enabled.
TEST(OpenPermsTest, WriteDenied) {
  ASSERT_TRUE(IsPolicyCapSupported(kOpenPermsPolicyCap));
  auto test_file = ScopedTempFDWithLabel(kFileLabel);
  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_open_allow_write_deny_open_t:s0", [&]() {
    EXPECT_THAT(open(test_file.name().c_str(), O_WRONLY), SyscallFailsWithErrno(EACCES));
  }));
}

}  // namespace
