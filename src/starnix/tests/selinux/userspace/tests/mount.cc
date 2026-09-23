// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/string_view.h>
#include <mntent.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <unistd.h>

#include <map>
#include <string>

#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

std::map<std::string, fit::result<int>> g_pre_policy_mount_results;

const char* kSelinuxMountOptions[] = {"context", "fscontext", "defcontext", "rootcontext"};

std::string MountOptionsFor(std::string_view mount_path) {
  FILE* mounts = setmntent("/proc/mounts", "r");
  std::string result;
  for (struct mntent* entry = 0; (entry = getmntent(mounts));) {
    if (mount_path == entry->mnt_dir) {
      result = entry->mnt_opts;
      break;
    }
  }
  endmntent(mounts);
  return result;
}

class MountOptionValidationTest : public ::testing::TestWithParam<const char*> {};

TEST_P(MountOptionValidationTest, RejectedBeforePolicyLoad) {
  const char* option_name = GetParam();
  auto it = g_pre_policy_mount_results.find(option_name);
  ASSERT_NE(it, g_pre_policy_mount_results.end()) << "Result for " << option_name << " not found";
  const auto& result = it->second;

  EXPECT_THAT(result, SyscallResultIsErrno(EINVAL))
      << "Mount with '" << option_name << "' unexpectedly succeeded or failed with wrong error";
}

TEST_P(MountOptionValidationTest, RejectedWhenInvalid) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
  }

  const char* option_name = GetParam();
  test_helper::ScopedTempDir mount_dir;

  // Providing a security context that is not valid in the loaded policy should result in EINVAL.
  std::string options = std::string(option_name) + "=test_u:object_r:invalid_context_t:s0";
  EXPECT_THAT(mount("tmpfs", mount_dir.path().c_str(), "tmpfs", 0, options.c_str()),
              SyscallFailsWithErrno(EINVAL));
}

INSTANTIATE_TEST_SUITE_P(MountTests, MountOptionValidationTest,
                         ::testing::ValuesIn(kSelinuxMountOptions),
                         [](const testing::TestParamInfo<const char*>& info) {
                           return info.param;
                         });

TEST(MountTest, NoSelinuxMountOptions) {
  test_helper::ScopedTempDir mount_dir;
  ASSERT_THAT(
      mount("tmpfs", mount_dir.path().c_str(), "tmpfs", MS_NOEXEC | MS_NOSUID | MS_NODEV, 0),
      SyscallSucceeds());

  std::string mount_options = MountOptionsFor(mount_dir.path());

  EXPECT_TRUE(cpp23::contains(mount_options, "nosuid"));
  EXPECT_TRUE(cpp23::contains(mount_options, "noexec"));
  EXPECT_TRUE(cpp23::contains(mount_options, "nodev"));
}

TEST(MountTest, WithContextOption) {
  test_helper::ScopedTempDir mount_dir;
  ASSERT_THAT(mount("tmpfs", mount_dir.path().c_str(), "tmpfs", MS_NOEXEC | MS_NOSUID | MS_NODEV,
                    "context=test_u:object_r:test_mount_fscontext_t:s0"),
              SyscallSucceeds());

  std::string mount_options = MountOptionsFor(mount_dir.path());

  EXPECT_FALSE(cpp23::contains(mount_options, "seclabel"));
  EXPECT_TRUE(cpp23::contains(mount_options, "context="));
}

TEST(MountTest, WithSeclabel) {
  // Base policy uses "fs_use_trans" labeling scheme for "tmpfs", which should report "seclabel".
  test_helper::ScopedTempDir mount_dir;
  ASSERT_THAT(
      mount("tmpfs", mount_dir.path().c_str(), "tmpfs", MS_NOEXEC | MS_NOSUID | MS_NODEV, 0),
      SyscallSucceeds());

  std::string mount_options = MountOptionsFor(mount_dir.path());

  EXPECT_TRUE(cpp23::contains(mount_options, "seclabel"));
}

TEST(MountTest, WithoutSeclabel) {
  // Base policy uses "genfscon" labeling scheme for "proc", which should not report "seclabel".
  test_helper::ScopedTempDir mount_dir;
  ASSERT_THAT(mount("selinuxfs", mount_dir.path().c_str(), "selinuxfs",
                    MS_NOEXEC | MS_NOSUID | MS_NODEV, 0),
              SyscallSucceeds());

  std::string mount_options = MountOptionsFor(mount_dir.path());

  EXPECT_FALSE(cpp23::contains(mount_options, "seclabel"));
}

// Verifies that the relabelfrom and relabelto checks are applied when mounting a filesystem and
// overriding the policy default security label for that filesystem type.
TEST(MountTest, FsContextRequiresRelabelFromAndTo) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running with sysadmin capabilities, skipping suite.";
  }

  const char* fscontext = "fscontext=test_u:object_r:test_mount_fscontext_t:s0";
  test_helper::ScopedTempDir mount_dir;

  auto enforce = ScopedEnforcement::SetEnforcing();

  // 1. Verify that mounting fails when 'relabelto' is denied.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_mount_relabelto_denied_t:s0", [&] {
    EXPECT_THAT(mount("tmpfs", mount_dir.path().c_str(), "tmpfs", 0, fscontext),
                SyscallFailsWithErrno(EACCES));
  }));

  // 2. Verify that mounting fails when 'relabelfrom' is denied.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_mount_relabelfrom_denied_t:s0", [&] {
    EXPECT_THAT(mount("tmpfs", mount_dir.path().c_str(), "tmpfs", 0, fscontext),
                SyscallFailsWithErrno(EACCES));
  }));

  // 3. Verify that mounting succeeds when both 'relabelfrom' and 'relabelto' are allowed.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_mount_relabel_allowed_t:s0", [&] {
    EXPECT_THAT(mount("tmpfs", mount_dir.path().c_str(), "tmpfs", 0, fscontext), SyscallSucceeds());
  }));
}

TEST(MountTest, BindRemountWithContext) {
  test_helper::ScopedTempDir source_dir;
  test_helper::ScopedTempDir target_dir;
  const char* ctx1 = "context=test_u:object_r:test_mount_fscontext_t:s0";
  const char* ctx2 = "context=test_u:object_r:test_mount_relabel_allowed_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_mount_permissive_t:s0", [&] {
    // 1. Initial mount of tmpfs with ctx1
    ASSERT_THAT(mount("tmpfs", source_dir.path().c_str(), "tmpfs", 0, ctx1), SyscallSucceeds());

    // 2. Bind mount source to target
    ASSERT_THAT(mount(source_dir.path().c_str(), target_dir.path().c_str(), nullptr, MS_BIND, 0),
                SyscallSucceeds());

    // 3. Remount the bind mount with SAME context - should succeed
    EXPECT_THAT(mount(nullptr, target_dir.path().c_str(), nullptr, MS_BIND | MS_REMOUNT, ctx1),
                SyscallSucceeds());

    // 4. Remount the bind mount with DIFFERENT context - Linux allows this (options are
    // ignored/no-op)
    EXPECT_THAT(mount(nullptr, target_dir.path().c_str(), nullptr, MS_BIND | MS_REMOUNT, ctx2),
                SyscallSucceeds());

    // 5. Verify the context is still ctx1 (it was not changed by the remount)
    EXPECT_TRUE(cpp23::contains(MountOptionsFor(target_dir.path()), "test_mount_fscontext_t"));
  }));
}

}  // namespace

extern std::string DoPrePolicyLoadWork() {
  for (const char* option : kSelinuxMountOptions) {
    test_helper::ScopedTempDir mount_dir;

    // Before policy load, any SELinux mount option should result in EINVAL because the kernel
    // cannot validate the context (or even know which options are valid).
    std::string options = std::string(option) + "=system_u:object_r:tmp_t:s0";
    if (mount("tmpfs", mount_dir.path().c_str(), "tmpfs", 0, options.c_str()) != -1) {
      g_pre_policy_mount_results.emplace(option, fit::ok());
    } else {
      g_pre_policy_mount_results.emplace(option, fit::error(errno));
    }
  }

  return "mount_policy";
}
