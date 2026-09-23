// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/syscall.h>

#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "capabilities_policy"; }

namespace {

/// Returns a header and data struct of the type required by `capget` and `capset`
/// populated with the Linux capability version preferred by Starnix.
/// The caps are properly zeroed out.
std::pair<__user_cap_header_struct, std::array<__user_cap_data_struct, _LINUX_CAPABILITY_U32S_3>>
NewCapStructs() {
  __user_cap_header_struct header;
  memset(&header, 0, sizeof(header));
  header.version = _LINUX_CAPABILITY_VERSION_3;

  std::array<__user_cap_data_struct, _LINUX_CAPABILITY_U32S_3> caps;
  memset(caps.data(), 0, sizeof(caps));

  return {header, caps};
}

/// When the `getcap` process class permission is granted, the `capget` syscall succeeds
/// when the header is valid and the user data argument is non-null.
TEST(CapabilitiesTest, GetCapAllowed) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_allow_getcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto [header, caps] = NewCapStructs();
    EXPECT_THAT(syscall(SYS_capget, &header, caps.data()), SyscallSucceeds());
  }));
}

/// When the `getcap` process class permission is denied, the `capget` syscall fails
/// with `EACCES` when the header is valid and the user data argument is non-null.
TEST(CapabilitiesTest, GetCapDenied) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_deny_getcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto [header, caps] = NewCapStructs();
    EXPECT_THAT(syscall(SYS_capget, &header, caps.data()), SyscallFailsWithErrno(EACCES));
  }));
}

/// When the `getcap` process class permission is denied, the `capget` syscall succeeds
/// when the header is valid and the user data argument is null. The syscall returns
/// without checking the `getcap` permission.
TEST(CapabilitiesTest, GetCapDeniedNullData) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_deny_getcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto [header, _] = NewCapStructs();
    EXPECT_THAT(syscall(SYS_capget, &header, NULL), SyscallSucceeds());
  }));
}

/// When the `getcap` process class permission is denied, the `capget` syscall fails
/// with `EINVAL` when the provided header struct contains an invalid capability version
/// and the user data argument is non-null. The syscall returns without checking the
/// `getcap` permission.
TEST(CapabilitiesTest, GetCapDeniedInvalidVersion) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_deny_getcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto [header, caps] = NewCapStructs();
    header.version = 0;
    EXPECT_THAT(syscall(SYS_capget, &header, caps.data()), SyscallFailsWithErrno(EINVAL));
  }));
}

/// When the `getcap` process class permission is denied, the `capget` syscall fails
/// with `EINVAL` when the provided header struct contains an invalid PID and the
/// arguments are otherwise valid. The syscall returns without checking the `getcap`
/// permission.
TEST(CapabilitiesTest, GetCapDeniedInvalidPid) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_deny_getcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto [header, caps] = NewCapStructs();
    header.pid = -1;
    EXPECT_THAT(syscall(SYS_capget, &header, caps.data()), SyscallFailsWithErrno(EINVAL));
  }));
}

/// When the `setcap` process class permission is granted, the `capset` syscall succeeds.
TEST(CapabilitiesTest, SetCapAllowed) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_allow_setcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto [header, caps] = NewCapStructs();
    // Attempt to drop all capabilities.
    caps.fill({0, 0, 0});
    EXPECT_THAT(syscall(SYS_capset, &header, caps.data()), SyscallSucceeds());
  }));
}

/// When the `setcap` process class permission is denied, the `capset` syscall fails
/// with `EACCES` when the provided arguments are valid.
TEST(CapabilitiesTest, SetCapDenied) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_deny_setcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto [header, caps] = NewCapStructs();
    // Attempt to drop all capabilities.
    EXPECT_THAT(syscall(SYS_capset, &header, caps.data()), SyscallFailsWithErrno(EACCES));
  }));
}

/// When the `setcap` process class permission is denied, the `capset` syscall fails
/// with `EFAULT` when the user data argument is null and the header is valid. The
/// syscall returns without checking the `setcap` permission.
TEST(CapabilitiesTest, SetCapDeniedNullData) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_deny_setcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto [header, _] = NewCapStructs();
    EXPECT_THAT(syscall(SYS_capset, &header, NULL), SyscallFailsWithErrno(EFAULT));
  }));
}

/// When the `setcap` process class permission is denied, the `capset` syscall fails
/// with `EINVAL` when the provided header struct contains an invalid capability version.
/// The syscall returns without checking the `setcap` permission.
TEST(CapabilitiesTest, SetCapDeniedInvalidVersion) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_deny_setcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto [header, caps] = NewCapStructs();
    header.version = 0;
    // Attempt to drop all capabilities.
    EXPECT_THAT(syscall(SYS_capset, &header, caps.data()), SyscallFailsWithErrno(EINVAL));
  }));
}

/// When the `setcap` process class permission is denied, the `capset` syscall fails
/// with EPERM when the target PID is different from the caller's PID. The syscall
/// returns without checking the `setcap` permission.
TEST(CapabilitiesTest, SetCapDeniedDifferentPid) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_deny_setcap_self_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  pid_t test_pid = getpid();

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    // Attempt to drop all capabilities for the parent process.
    auto [header, caps] = NewCapStructs();
    header.pid = test_pid;
    EXPECT_THAT(syscall(SYS_capset, &header, caps.data()), SyscallFailsWithErrno(EPERM));
  }));
}

/// When the `setcap` process class permission is denied, the `capset` syscall fails
/// with `EPERM` when the header is valid and the request attempts to add a capability
/// to the target's permitted set. The syscall returns without checking the `setcap`
/// permission.
TEST(CapabilitiesTest, SetCapDeniedExpandPermittedSet) {
  constexpr char kTestSecurityContext[] = "test_u:test_r:test_deny_setcap_self_t:s0";

  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    // Prepare for the test by dropping the `CAP_SYS_ADMIN` capability from the
    // effective and permitted sets while running SELinux in permissive mode.
    auto [header, caps] = NewCapStructs();
    ASSERT_THAT(syscall(SYS_capget, &header, caps.data()), SyscallSucceeds());
    caps[CAP_TO_INDEX(CAP_SYS_ADMIN)].effective &= ~CAP_TO_MASK(CAP_SYS_ADMIN);
    caps[CAP_TO_INDEX(CAP_SYS_ADMIN)].permitted &= ~CAP_TO_MASK(CAP_SYS_ADMIN);
    ASSERT_THAT(syscall(SYS_capset, &header, caps.data()), SyscallSucceeds());

    // Start enforcing SELinux permission checks.
    auto enforce = ScopedEnforcement::SetEnforcing();

    // Attempt to add the `CAP_SYS_ADMIN` capability back to the permitted set.
    caps[CAP_TO_INDEX(CAP_SYS_ADMIN)].effective |= CAP_TO_MASK(CAP_SYS_ADMIN);
    EXPECT_THAT(syscall(SYS_capset, &header, caps.data()), SyscallFailsWithErrno(EPERM));
  }));
}

}  // namespace
