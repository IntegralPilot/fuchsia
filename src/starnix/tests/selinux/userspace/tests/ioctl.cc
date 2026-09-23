// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <net/if.h>
#include <netinet/in.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/xattr.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/fs.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

extern std::string DoPrePolicyLoadWork() { return "ioctl_policy"; }

namespace {

constexpr char kTestFilePathTemplate[] = "/tmp/ioctl_test_file:XXXXXX";
constexpr char kTestFileSecurityContext[] = "test_u:object_r:test_ioctl_file_t:s0";
constexpr char kSelinuxXattr[] = "security.selinux";

fit::result<int, fbl::unique_fd> CreateTestFile() {
  std::string filename(kTestFilePathTemplate);
  fbl::unique_fd fd(mkstemp(filename.data()));
  if (!fd.is_valid()) {
    return fit::error(errno);
  }
  if (fsetxattr(fd.get(), kSelinuxXattr, kTestFileSecurityContext, sizeof(kTestFileSecurityContext),
                0) < 0) {
    return fit::error(errno);
  }
  return fit::ok(std::move(fd));
}

fit::result<int, int> GetBlockSize(int fd) {
  int block_size = 0;
  int result = ioctl(fd, FIGETBSZ, &block_size);
  if (result < 0) {
    return fit::error(errno);
  }
  return fit::ok(result);
}

/// Check that the `ioctl` permission is granted when it is allowed by the policy,
/// and ioctl extended permissions are not filtered.
TEST(IoctlTest, BasicCommandAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  auto test_fd = CreateTestFile();
  ASSERT_TRUE(test_fd.is_ok());

  constexpr char kTestSecurityContext[] = "test_u:test_r:test_ioctl_allowed_t:s0";
  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    size_t value = 0;
    EXPECT_THAT(ioctl(test_fd.value().get(), 0xabcd, &value),
                ::testing::Not(SyscallFailsWithErrno(EACCES)));
  }));
}

/// Check that the `ioctl` permission is denied when it is disallowed by the policy.
TEST(IoctlTest, BasicCommandDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  auto test_fd = CreateTestFile();
  ASSERT_TRUE(test_fd.is_ok());

  constexpr char kTestSecurityContext[] = "test_u:test_r:test_ioctl_denied_t:s0";
  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    size_t value = 0;
    EXPECT_THAT(ioctl(test_fd.value().get(), 0xabcd, &value), SyscallFailsWithErrno(EACCES));
  }));
}

/// Check that the `getattr` permission is granted when it is invoked by a special-cased
/// ioctl command, and is allowed by the policy.
TEST(IoctlTest, GetAttrCommandAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  auto test_fd = CreateTestFile();
  ASSERT_TRUE(test_fd.is_ok());

  constexpr char kTestSecurityContext[] = "test_u:test_r:test_ioctl_figetbsz_allowed_t:s0";
  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto result = GetBlockSize(test_fd.value().get());
    EXPECT_TRUE(result.is_ok());
  }));
}

/// Check that the `getattr` permission is denied when it is invoked by a special-cased
/// ioctl command, and is disallowed by the policy.
TEST(IoctlTest, GetAttrCommandDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  auto test_fd = CreateTestFile();
  ASSERT_TRUE(test_fd.is_ok());

  constexpr char kTestSecurityContext[] = "test_u:test_r:test_ioctl_figetbsz_denied_t:s0";
  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    auto result = GetBlockSize(test_fd.value().get());
    EXPECT_TRUE(result.is_error());
  }));
}

/// Check that the `ioctl` permission is granted when it is allowed by the policy,
/// in the case where ioctl commands are filtered and the specified command is
/// allowed by the filter.
TEST(IoctlTest, FilteredCommandAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  auto test_fd = CreateTestFile();
  ASSERT_TRUE(test_fd.is_ok());

  constexpr char kTestSecurityContext[] = "test_u:test_r:test_ioctl_xperms_filtered_t:s0";
  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    size_t value = 0;
    EXPECT_THAT(ioctl(test_fd.value().get(), 0xabcd, &value),
                ::testing::Not(SyscallFailsWithErrno(EACCES)));
  }));
}

/// Check that the `ioctl` permission is denied when the `ioctl` permission is allowed
/// by the policy but the specified ioctl command is disallowed by extended permission
/// filtering.
TEST(IoctlTest, FilteredCommandDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  auto test_fd = CreateTestFile();
  ASSERT_TRUE(test_fd.is_ok());

  constexpr char kTestSecurityContext[] = "test_u:test_r:test_ioctl_xperms_filtered_t:s0";
  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    size_t value = 0;
    EXPECT_THAT(ioctl(test_fd.value().get(), 0xabcc, &value), SyscallFailsWithErrno(EACCES));
  }));
}

/// Check that the `ioctl` permission is denied in the case where ioctl commands
/// are filtered and the specified command is allowed by the filter, but the `ioctl`
/// permission itself is disallowed by the policy.
TEST(IoctlTest, FilteredCommandDeniedByPermission) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  auto test_fd = CreateTestFile();
  ASSERT_TRUE(test_fd.is_ok());

  constexpr char kTestSecurityContext[] = "test_u:test_r:test_ioctl_xperms_only_t:s0";
  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    size_t value = 0;
    EXPECT_THAT(ioctl(test_fd.value().get(), 0xabcd, &value), SyscallFailsWithErrno(EACCES));
  }));
}

/// Check that a socket ioctl (e.g. SIOCGIFADDR, SIOCGIFFLAGS, SIOCGIFINDEX) is granted when `ioctl`
/// on the socket is allowed by the policy, and that internal kernel emulation (such as Netlink
/// queries) does not require netlink permissions from the calling user task.
TEST(IoctlTest, SocketIoctlAllowedWithoutNetlinkPermission) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  constexpr char kTestSecurityContext[] = "test_u:test_r:test_ioctl_socket_allowed_t:s0";
  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    fbl::unique_fd sock(socket(AF_INET, SOCK_DGRAM, 0));
    ASSERT_TRUE(sock.is_valid()) << strerror(errno);

    ifreq ifr = {};
    ifr.ifr_addr.sa_family = AF_INET;
    strncpy(ifr.ifr_name, "lo", IFNAMSIZ);

    EXPECT_THAT(ioctl(sock.get(), SIOCGIFADDR, &ifr), SyscallSucceeds());
    EXPECT_THAT(ioctl(sock.get(), SIOCGIFFLAGS, &ifr), SyscallSucceeds());
    EXPECT_THAT(ioctl(sock.get(), SIOCGIFINDEX, &ifr), SyscallSucceeds());
  }));
}

/// Check that a socket ioctl is denied when `ioctl` permission on the socket is disallowed by
/// policy.
TEST(IoctlTest, SocketIoctlDeniedWithoutIoctlPermission) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  constexpr char kTestSecurityContext[] = "test_u:test_r:test_ioctl_socket_denied_t:s0";
  ASSERT_TRUE(RunSubprocessAs(kTestSecurityContext, [&] {
    fbl::unique_fd sock(socket(AF_INET, SOCK_DGRAM, 0));
    ASSERT_TRUE(sock.is_valid()) << strerror(errno);

    ifreq ifr = {};
    ifr.ifr_addr.sa_family = AF_INET;
    strncpy(ifr.ifr_name, "lo", IFNAMSIZ);

    EXPECT_THAT(ioctl(sock.get(), SIOCGIFADDR, &ifr), SyscallFailsWithErrno(EACCES));
  }));
}

}  // namespace
