// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>

#include <optional>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"

namespace {
constexpr char kDefaultTmpFsNodeLabel[] = "system_u:object_r:tmpfs_t:s0";
constexpr char kTestSecurityXattr[] = "system_u:object_r:unconfined_t:s0";
constexpr char kTestInvalidSecurityXattr[] = "not a valid context";

constexpr char kTestFileWithoutXattr[] = "/tmp/test-file-without-xattr";
constexpr char kTestFileWithXattr[] = "/tmp/test-file-with-xattr";
constexpr char kTestFileWithInvalidXattr[] = "/tmp/test-file-with-invalid-xattr";

struct PrePolicyProcFsLabels {
  fit::result<int, std::string> meminfo;
  fit::result<int, std::string> stat;
};

std::optional<PrePolicyProcFsLabels> g_pre_policy_procfs_labels;
}  // namespace

extern std::string DoPrePolicyLoadWork() {
  g_pre_policy_procfs_labels = PrePolicyProcFsLabels{
      .meminfo = GetLabel("/proc/meminfo"),
      .stat = GetLabel("/proc/self/stat"),
  };

  // Create a file in "tmpfs" without explicitly setting the "security.selinux" attribute.
  auto fd = fbl::unique_fd(open(kTestFileWithoutXattr, O_CREAT | O_RDWR, 0644));
  EXPECT_TRUE(fd.is_valid()) << "Failed to create test file:" << strerror(errno);

  // Create a file in "tmpfs" with an explicitly-specified label, different from the calculated one.
  fd = fbl::unique_fd(open(kTestFileWithXattr, O_CREAT | O_RDWR, 0644));
  EXPECT_TRUE(fd.is_valid()) << "Failed to create test file:" << strerror(errno);
  EXPECT_THAT(SetLabel(kTestFileWithXattr, kTestSecurityXattr), SyscallResultIsOk());

  // Create a file in "tmpfs" with an invalid security xattr value.
  fd = fbl::unique_fd(open(kTestFileWithInvalidXattr, O_CREAT | O_RDWR, 0644));
  EXPECT_TRUE(fd.is_valid()) << "Failed to create test file:" << strerror(errno);
  EXPECT_THAT(SetLabel(kTestFileWithInvalidXattr, kTestInvalidSecurityXattr), SyscallResultIsOk());

  return "minimal_policy";
}

namespace {

TEST(PolicyLoadTest, TasksUseKernelSid) {
  // All processes created prior to policy loading are labeled with the kernel SID.
  EXPECT_THAT(ReadTaskAttr("current"), SyscallResultIsOk("system_u:unconfined_r:unconfined_t:s0"));
}

TEST(PolicyLoadTest, TmpFsFsUseTransIgnoresSecurityXattr) {
  // "tmpfs" is defined to use the `fs_use_trans` labeling scheme, which ignores the xattr value.
  EXPECT_THAT(GetLabel(kTestFileWithoutXattr), SyscallResultIsOk(kDefaultTmpFsNodeLabel));
  EXPECT_THAT(GetLabel(kTestFileWithXattr), SyscallResultIsOk(kDefaultTmpFsNodeLabel));
  EXPECT_THAT(GetLabel(kTestFileWithInvalidXattr), SyscallResultIsOk(kDefaultTmpFsNodeLabel));
}

TEST(PolicyLoadTest, ProcFsLabels) {
  ASSERT_TRUE(g_pre_policy_procfs_labels.has_value());
  // Procfs does not support xattrs prior to policy load.
  EXPECT_THAT(g_pre_policy_procfs_labels->meminfo, SyscallResultIsErrno(EOPNOTSUPP));
  EXPECT_THAT(g_pre_policy_procfs_labels->stat, SyscallResultIsErrno(EOPNOTSUPP));

  // After policy load, non-PID nodes use the genfscon label and PID nodes use the task label.
  EXPECT_THAT(GetLabel("/proc/meminfo"), SyscallResultIsOk("system_u:object_r:unconfined_t:s0"));
  EXPECT_THAT(GetLabel("/proc/self/stat"),
              SyscallResultIsOk("system_u:unconfined_r:unconfined_t:s0"));
}

}  // namespace
