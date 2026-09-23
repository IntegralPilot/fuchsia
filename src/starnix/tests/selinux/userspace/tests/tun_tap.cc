// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <net/if.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/sysmacros.h>

#include <filesystem>

#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/if_tun.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {
const char kTunPath[] = "/dev/net/tun";

template <typename T>
class TunTapTestBase : public ::testing::TestWithParam<T> {
 protected:
  test_helper::ScopedMount mount_;

  void SetUp() {
    mount_ = ASSERT_RESULT_SUCCESS_AND_RETURN(test_helper::ScopedMount::Mount(
        "tmpfs", "/dev", "tmpfs", MS_NOEXEC | MS_NOSUID | MS_STRICTATIME,
        "fscontext=system_u:object_r:devfs_t:s0"));

    ASSERT_THAT(mkdir("/dev/net", 0755), SyscallSucceeds());
    ASSERT_THAT(mknod(kTunPath, S_IFCHR | 0666, makedev(10, 200)), SyscallSucceeds());
    ASSERT_THAT(chmod(kTunPath, 0666), SyscallSucceeds());
  }
};

struct TunTapTestCase {
  std::string_view label;
  fit::result<int> expected_result;
  short tun_or_tap;
};

class TunTapCreateTest : public TunTapTestBase<TunTapTestCase> {};

INSTANTIATE_TEST_SUITE_P(
    TunTapTests, TunTapCreateTest,
    ::testing::Values(
        TunTapTestCase{"test_u:test_r:tun_test_create_t:s0", fit::ok(), IFF_TUN},
        TunTapTestCase{"test_u:test_r:tun_test_create_t:s0", fit::ok(), IFF_TAP},
        TunTapTestCase{"test_u:test_r:tun_test_no_create_t:s0", fit::error(EACCES), IFF_TUN},
        TunTapTestCase{"test_u:test_r:tun_test_no_create_t:s0", fit::error(EACCES), IFF_TAP}));

TEST_P(TunTapCreateTest, CheckCreateAccess) {
  const TunTapTestCase& test_case = GetParam();
  auto enforce = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs(test_case.label, [&]() {
    fbl::unique_fd fd(open(kTunPath, O_RDWR));
    ASSERT_TRUE(fd) << strerror(errno);
    EXPECT_THAT(GetLinkLabel(fd.get()), SyscallResultIsOk(std::string(test_case.label).c_str()));

    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    ifr.ifr_flags = test_case.tun_or_tap | IFF_NO_PI;

    int result = ioctl(fd.get(), TUNSETIFF, &ifr);
    if (test_case.expected_result.is_ok()) {
      EXPECT_THAT(result, SyscallSucceeds());
      EXPECT_THAT(GetLinkLabel(fd.get()), SyscallResultIsOk(std::string(test_case.label).c_str()));
    } else {
      EXPECT_THAT(result, SyscallFailsWithErrno(test_case.expected_result.error_value()));
    }
  }));
}

class TunTapSockcreateTest : public TunTapTestBase<bool> {};

INSTANTIATE_TEST_SUITE_P(TunTapTests, TunTapSockcreateTest, testing::Bool());

TEST_P(TunTapSockcreateTest, SockcreateIgnored) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:tun_test_sockcreate_t:s0", [&]() {
    auto sockcreate =
        ScopedTaskAttrResetter::SetTaskAttr("sockcreate", "test_u:test_r:tun_test_create_t:s0");

    fbl::unique_fd fd(open(kTunPath, O_RDWR));
    ASSERT_TRUE(fd) << strerror(errno);

    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    ifr.ifr_flags = (GetParam() ? IFF_TUN : IFF_TAP) | IFF_NO_PI;

    // Check that creating a TUN socket does not use the "sockcreate" label, leading to a denial as
    // `tun_test_t` does not grant `tun_socket` `create` on self.
    EXPECT_THAT(ioctl(fd.get(), TUNSETIFF, &ifr), SyscallFailsWithErrno(EACCES));
  }));
}

class TunTapTransitionTest : public TunTapTestBase<bool> {};

INSTANTIATE_TEST_SUITE_P(TunTapTests, TunTapTransitionTest, testing::Bool());

TEST_P(TunTapTransitionTest, TypeTransitionIgnored) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:tun_test_trans_t:s0", [&]() {
    fbl::unique_fd fd(open(kTunPath, O_RDWR));
    ASSERT_TRUE(fd) << strerror(errno);

    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    ifr.ifr_flags = (GetParam() ? IFF_TUN : IFF_TAP) | IFF_NO_PI;

    // Check that creating a TUN socket does not follow the type_transition rule from
    // `tun_test_trans_t` to `tun_test_yes_t`, leading to a denial.
    EXPECT_THAT(ioctl(fd.get(), TUNSETIFF, &ifr), SyscallFailsWithErrno(EACCES));
  }));
}

class TunTapAttachTest : public TunTapTestBase<TunTapTestCase> {};

INSTANTIATE_TEST_SUITE_P(
    TunTapTests, TunTapAttachTest,
    ::testing::Values(
        TunTapTestCase{"test_u:test_r:tun_test_attach_t:s0", fit::ok(), IFF_TUN},
        TunTapTestCase{"test_u:test_r:tun_test_attach_t:s0", fit::ok(), IFF_TAP},
        TunTapTestCase{"test_u:test_r:tun_test_no_attach_t:s0", fit::error(EACCES), IFF_TUN},
        TunTapTestCase{"test_u:test_r:tun_test_no_attach_t:s0", fit::error(EACCES), IFF_TAP}));

TEST_P(TunTapAttachTest, AttachQueue) {
  const TunTapTestCase& test_case = GetParam();
  auto enforce = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs(test_case.label, [&]() {
    fbl::unique_fd fd(open(kTunPath, O_RDWR));
    ASSERT_TRUE(fd) << strerror(errno);

    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    ifr.ifr_flags = test_case.tun_or_tap | IFF_MULTI_QUEUE;
    ASSERT_THAT(ioctl(fd.get(), TUNSETIFF, &ifr), SyscallSucceeds());

    memset(&ifr, 0, sizeof(ifr));
    ifr.ifr_flags = IFF_DETACH_QUEUE;
    int result = ioctl(fd.get(), TUNSETQUEUE, &ifr);
    if (test_helper::IsStarnix()) {
      // TUNSETQUEUE is not supported in Starnix.
      EXPECT_THAT(result, SyscallFailsWithErrno(ENOTTY));
      return;
    }
    ASSERT_THAT(result, SyscallSucceeds());

    memset(&ifr, 0, sizeof(ifr));
    ifr.ifr_flags = IFF_ATTACH_QUEUE;
    // Trigger an `attach_queue` event.
    result = ioctl(fd.get(), TUNSETQUEUE, &ifr);
    if (test_case.expected_result.is_ok()) {
      EXPECT_THAT(result, SyscallSucceeds());
    } else {
      EXPECT_THAT(result, SyscallFailsWithErrno(test_case.expected_result.error_value()));
    }
  }));
}

class TunTapRelabelTest : public TunTapTestBase<TunTapTestCase> {};

INSTANTIATE_TEST_SUITE_P(
    TunTapTests, TunTapRelabelTest,
    ::testing::Values(
        TunTapTestCase{"test_u:test_r:tun_test_relabel_t:s0", fit::ok(), IFF_TUN},
        TunTapTestCase{"test_u:test_r:tun_test_relabel_t:s0", fit::ok(), IFF_TAP},
        TunTapTestCase{"test_u:test_r:tun_test_no_relabelto_t:s0", fit::error(EACCES), IFF_TUN},
        TunTapTestCase{"test_u:test_r:tun_test_no_relabelto_t:s0", fit::error(EACCES), IFF_TAP},
        TunTapTestCase{"test_u:test_r:tun_test_no_relabelfrom_t:s0", fit::error(EACCES), IFF_TUN},
        TunTapTestCase{"test_u:test_r:tun_test_no_relabelfrom_t:s0", fit::error(EACCES), IFF_TAP}));

TEST_P(TunTapRelabelTest, Relabel) {
  const TunTapTestCase& test_case = GetParam();
  auto enforce = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:tun_test_t:s0", [&]() {
    fbl::unique_fd fd(open(kTunPath, O_RDWR));
    ASSERT_TRUE(fd) << strerror(errno);

    struct ifreq ifr;
    memset(&ifr, 0, sizeof(ifr));
    ifr.ifr_flags = test_case.tun_or_tap | IFF_MULTI_QUEUE;
    ASSERT_THAT(ioctl(fd.get(), TUNSETIFF, &ifr), SyscallSucceeds());
    char tun_name[IFNAMSIZ];
    strcpy(tun_name, ifr.ifr_name);

    // Persist the tun name.
    if (test_helper::IsStarnix()) {
      // TUNSETPERSIST is not supported in Starnix.
      EXPECT_THAT(ioctl(fd.get(), TUNSETPERSIST, 1), SyscallFailsWithErrno(ENOTTY));
      return;
    }
    ASSERT_THAT(ioctl(fd.get(), TUNSETPERSIST, 1), SyscallSucceeds());

    ASSERT_THAT(WriteTaskAttr("current", test_case.label), SyscallResultIsOk());
    fbl::unique_fd fd2(open(kTunPath, O_RDWR));
    ASSERT_TRUE(fd2) << strerror(errno);
    memset(&ifr, 0, sizeof(struct ifreq));
    ifr.ifr_flags = test_case.tun_or_tap | IFF_MULTI_QUEUE;
    // Use the initial tun name to test relabeling.
    strcpy(ifr.ifr_name, tun_name);
    int result = ioctl(fd2.get(), TUNSETIFF, &ifr);
    if (test_case.expected_result.is_ok()) {
      EXPECT_THAT(result, SyscallSucceeds());
    } else {
      EXPECT_THAT(result, SyscallFailsWithErrno(test_case.expected_result.error_value()));
    }
  }));
}

}  // namespace

extern std::string DoPrePolicyLoadWork() { return "tun_policy"; }
