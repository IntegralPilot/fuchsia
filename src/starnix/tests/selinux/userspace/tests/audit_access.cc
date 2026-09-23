// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/xattr.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

extern std::string DoPrePolicyLoadWork() { return "audit_access_policy"; }

namespace {
constexpr char kAuditAccessTestContext[] = "test_u:test_r:test_audit_access_t:s0";
constexpr char kAuditAccessFileLabel[] = "test_u:object_r:test_audit_access_file_t:s0";
constexpr char kDontAuditAccessFileLabel[] = "test_u:object_r:test_audit_access_noaudit_file_t:s0";

TEST(AuditAccessTest, AccessWithoutDontAudit) {
  auto test_file = ScopedTempFDWithLabel(kAuditAccessFileLabel);

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs(kAuditAccessTestContext, [&]() {
    EXPECT_THAT(access(test_file.name().c_str(), W_OK), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(access(test_file.name().c_str(), R_OK), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(AuditAccessTest, OpenWithoutDontAudit) {
  auto test_file = ScopedTempFDWithLabel(kAuditAccessFileLabel);

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs(kAuditAccessTestContext, [&]() {
    EXPECT_THAT(open(test_file.name().c_str(), O_RDONLY), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(open(test_file.name().c_str(), O_WRONLY), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(AuditAccessTest, AccessWithDontAudit) {
  auto test_file = ScopedTempFDWithLabel(kDontAuditAccessFileLabel);

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs(kAuditAccessTestContext, [&]() {
    EXPECT_THAT(access(test_file.name().c_str(), W_OK), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(access(test_file.name().c_str(), R_OK), SyscallFailsWithErrno(EACCES));
  }));

  // This test will fail on audit expectations if "dontaudit ... :file { audit_access}" is broken.
}

TEST(AuditAccessTest, OpenWithDontAudit) {
  auto test_file = ScopedTempFDWithLabel(kDontAuditAccessFileLabel);

  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs(kAuditAccessTestContext, [&]() {
    EXPECT_THAT(open(test_file.name().c_str(), O_RDONLY), SyscallFailsWithErrno(EACCES));
    EXPECT_THAT(open(test_file.name().c_str(), O_WRONLY), SyscallFailsWithErrno(EACCES));
  }));
}

}  // namespace
