// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/mman.h>
#include <sys/xattr.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace {

int g_before_policy_fd = -1;

class MemFdClassTest : public ::testing::Test {
  void SetUp() override {
    constexpr char kMemfdClassPolicyCap[] = "memfd_class";
    ASSERT_TRUE(IsPolicyCapSupported(kMemfdClassPolicyCap));
    ASSERT_TRUE(IsPolicyCapEnabled(kMemfdClassPolicyCap));
  }
};

TEST_F(MemFdClassTest, MemFdPrePolicyLoadGetsTmpFsSid) {
  auto enforcing = ScopedEnforcement::SetEnforcing();
  EXPECT_THAT(GetLabel(g_before_policy_fd), SyscallResultIsOk("system_u:object_r:tmpfs_t:s0"));
}

TEST_F(MemFdClassTest, MemFdTransition) {
  auto enforcing = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_memfd_transition_t:s0", []() {
    int fd;
    EXPECT_THAT((fd = test_helper::MemFdCreate("test", 0)), SyscallSucceeds());
    EXPECT_THAT(GetLabel(fd), SyscallResultIsOk("test_u:object_r:test_memfd_transitioned_t:s0"));
  }));
}

TEST_F(MemFdClassTest, MemFdNoTransition) {
  auto enforcing = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_memfd_no_transition_t:s0", []() {
    int fd;
    EXPECT_THAT((fd = test_helper::MemFdCreate("test", 0)), SyscallSucceeds());
    EXPECT_THAT(GetLabel(fd), SyscallResultIsOk("test_u:object_r:test_memfd_no_transition_t:s0"));
  }));
}

TEST_F(MemFdClassTest, MemFdNoPermissions) {
  auto enforcing = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_memfd_no_create_permission_t:s0", []() {
    int fd;
    EXPECT_THAT((fd = test_helper::MemFdCreate("test", 0)), SyscallFailsWithErrno(EACCES));
  }));
}

}  // namespace

extern std::string DoPrePolicyLoadWork() {
  // Create a memfd prior to policy load, to allow the test to validate the post-policy label.
  EXPECT_THAT((g_before_policy_fd = test_helper::MemFdCreate("test", 0)), SyscallSucceeds());

  // Until a policy is loaded, no file label is provided.
  EXPECT_THAT(GetLabel(g_before_policy_fd), SyscallResultIsErrno(ENODATA));

  return "memfd_class_policy";
}
