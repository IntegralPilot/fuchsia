// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/prctl.h>
#include <sys/syscall.h>

#include <fstream>
#include <iostream>
#include <string>

#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/prctl.h>
#include <linux/securebits.h>

#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

// These are missing from our sys/prctl.h.
#define PR_CAP_AMBIENT 47
#define PR_CAP_AMBIENT_IS_SET 1
#define PR_CAP_AMBIENT_RAISE 2
#define PR_CAP_AMBIENT_LOWER 3
#define PR_CAP_AMBIENT_CLEAR_ALL 4

namespace {

TEST(PrctlTest, SubReaperTest) {
  // TODO(https://fxbug.dev/42080141): Find out why this test does not work on host in CQ
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }

  test_helper::ForkHelper helper;

  // Reap children.
  prctl(PR_SET_CHILD_SUBREAPER, 1);

  pid_t ancestor_pid = SAFE_SYSCALL(getpid());
  ASSERT_NE(1, ancestor_pid);
  pid_t parent_pid = SAFE_SYSCALL(getppid());
  ASSERT_NE(0, parent_pid);
  ASSERT_NE(ancestor_pid, parent_pid);

  helper.RunInForkedProcess([&] {
    // Fork again
    helper.RunInForkedProcess([&] {
      // Wait to be reparented.
      while (SAFE_SYSCALL(getppid()) != ancestor_pid) {
      }
    });
    // Parent return and makes the child an orphan.
  });

  // Expect that both child ends up being reaped to this process.
  for (size_t i = 0; i < 2; ++i) {
    EXPECT_GT(wait(nullptr), 0);
  }
}

TEST(PrctlTest, SecureBits) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL_SKIP_ON_EPERM(prctl(PR_SET_SECUREBITS, SECBIT_NOROOT));
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_GET_SECUREBITS)), SECBIT_NOROOT);
    SAFE_SYSCALL(prctl(PR_SET_SECUREBITS, SECBIT_KEEP_CAPS));
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_GET_SECUREBITS)), SECBIT_KEEP_CAPS);
  });
}

// Test that setreuid(-1,-1) does not spuriously update saved_uid.
// Bug: Starnix compared prev.uid against the raw euid argument (u32::MAX)
// instead of checking euid != -1 first, causing saved_uid to be set to
// the current euid on every setreuid(-1,-1) call.
TEST(PrctlTest, SetreuidNoopDoesNotCorruptSavedUid) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running as root";
  }
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Setup: uid=1000, euid=0, saved_uid=1000 (like a setuid-root binary)
    ASSERT_EQ(setresuid(1000, 0, 1000), 0);

    uid_t ruid, euid, suid;
    ASSERT_EQ(getresuid(&ruid, &euid, &suid), 0);
    ASSERT_EQ(ruid, 1000u);
    ASSERT_EQ(euid, 0u);
    ASSERT_EQ(suid, 1000u);

    // setreuid(-1, -1) should be a complete no-op
    ASSERT_EQ(setreuid(-1, -1), 0);
    ASSERT_EQ(getresuid(&ruid, &euid, &suid), 0);

    EXPECT_EQ(ruid, 1000u) << "setreuid(-1,-1) should not change ruid";
    EXPECT_EQ(euid, 0u) << "setreuid(-1,-1) should not change euid";
    EXPECT_EQ(suid, 1000u) << "setreuid(-1,-1) should not change saved_uid";
  });
}

// Full chain: setreuid(-1,-1) corrupts saved_uid → privilege escalation.
// Scenario: setuid-root binary with uid=1000, euid=0, saved_uid=1000
// calls setreuid(-1,-1) → saved_uid becomes 0 → drop euid → regain root.
TEST(PrctlTest, SetreuidNoopToPrivilegeEscalation) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running as root";
  }
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    uid_t ruid, euid, suid;

    // Step 1: Setuid-root binary state
    ASSERT_EQ(setresuid(1000, 0, 1000), 0);
    ASSERT_EQ(getresuid(&ruid, &euid, &suid), 0);
    ASSERT_EQ(ruid, 1000u);
    ASSERT_EQ(euid, 0u);
    ASSERT_EQ(suid, 1000u);

    // Step 2: No-op call - should NOT corrupt saved_uid
    ASSERT_EQ(setreuid(-1, -1), 0);
    ASSERT_EQ(getresuid(&ruid, &euid, &suid), 0);
    ASSERT_EQ(ruid, 1000u);
    ASSERT_EQ(euid, 0u);
    ASSERT_EQ(suid, 1000u);

    // Step 3: Drop euid
    ASSERT_EQ(seteuid(1000), 0);
    ASSERT_EQ(getresuid(&ruid, &euid, &suid), 0);
    ASSERT_EQ(ruid, 1000u);
    ASSERT_EQ(euid, 1000u);
    ASSERT_EQ(suid, 1000u);

    // Step 4: Try to regain root - must fail
    seteuid(0);
    ASSERT_EQ(getresuid(&ruid, &euid, &suid), 0);

    EXPECT_NE(euid, 0u) << "Should not regain root after privilege drop";
  });
}

// Test that setreuid() does not allow ruid=saved_uid.
// Linux's __sys_setreuid() only accepts ruid={current_uid, current_euid},
// deliberately excluding saved_uid. This prevents a process that dropped
// privileges from regaining root via setreuid when setresuid would be required.
TEST(PrctlTest, SetreuidRuidCannotUseSavedUid) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running as root";
  }
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Setup: uid=1000, euid=1000, saved_uid=0
    // Standard state after a setuid-root binary drops privileges
    ASSERT_EQ(setresuid(1000, 1000, 0), 0);

    uid_t ruid, euid, suid;
    ASSERT_EQ(getresuid(&ruid, &euid, &suid), 0);
    ASSERT_EQ(ruid, 1000u);
    ASSERT_EQ(euid, 1000u);
    ASSERT_EQ(suid, 0u);

    // setreuid(0, -1) - ruid=saved_uid. Linux denies this with EPERM.
    int ret = setreuid(0, -1);
    EXPECT_EQ(ret, -1) << "setreuid(saved_uid, -1) should be denied";
    if (ret == -1) {
      EXPECT_EQ(errno, EPERM);
    }

    // Verify credentials are unchanged
    ASSERT_EQ(getresuid(&ruid, &euid, &suid), 0);
    EXPECT_EQ(ruid, 1000u);
    EXPECT_EQ(euid, 1000u);
    EXPECT_EQ(suid, 0u);
  });
}

// Same test for setregid: rgid=saved_gid should be denied.
TEST(PrctlTest, SetregidRgidCannotUseSavedGid) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running as root";
  }
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Set up GID state first while still privileged: gid=1000, egid=1000, saved_gid=0.
    ASSERT_EQ(setresgid(1000, 1000, 0), 0);

    // Drop effective capabilities by transitioning euid from 0 to non-zero.
    // This mirrors a setuid-root binary that has dropped privileges, and in
    // particular clears CAP_SETGID so the setregid check below actually
    // exercises the saved_gid rule rather than being short-circuited by caps.
    ASSERT_EQ(setresuid(1000, 1000, 0), 0);

    gid_t rgid, egid, sgid;
    ASSERT_EQ(getresgid(&rgid, &egid, &sgid), 0);
    ASSERT_EQ(rgid, 1000u);
    ASSERT_EQ(egid, 1000u);
    ASSERT_EQ(sgid, 0u);

    // setregid(0, -1) - rgid=saved_gid. Linux denies this with EPERM.
    int ret = setregid(0, -1);
    EXPECT_EQ(ret, -1) << "setregid(saved_gid, -1) should be denied";
    if (ret == -1) {
      EXPECT_EQ(errno, EPERM);
    }

    // Verify credentials are unchanged
    ASSERT_EQ(getresgid(&rgid, &egid, &sgid), 0);
    EXPECT_EQ(rgid, 1000u);
    EXPECT_EQ(egid, 1000u);
    EXPECT_EQ(sgid, 0u);
  });
}

TEST(PrctlTest, Argv0SniffingIsUndetectableInUserspace) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    std::ifstream cmdline_file("/proc/self/cmdline");
    ASSERT_TRUE(cmdline_file.is_open());
    std::string argv0;
    std::getline(cmdline_file, argv0, '\0');

    std::string argv0Basename = argv0;
    argv0Basename.erase(argv0Basename.begin(),
                        argv0Basename.begin() + argv0Basename.find_last_of('/') + 1);

    char name[16];
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_GET_NAME, name)), 0);
    ASSERT_EQ(std::string(name), "starnix_prctl_t");
    // The comm should be a truncation of argv[0] to start
    ASSERT_THAT(argv0Basename, ::testing::StartsWith(name));

    // Set the comm to a suffix of argv0, this will cause Starnix's sniffing to use the full
    // argv[0] as the Fuchsia-side name.
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_SET_NAME, "prctl_test")), 0);
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_GET_NAME, name)), 0);
    // Userspace should still observe just the infix even if the Fuchsia-side has the full string.
    ASSERT_EQ(std::string(name), "prctl_test");
    ASSERT_THAT(argv0Basename, ::testing::HasSubstr(name));
  });
}

TEST(PrctlTest, DropCapabilities) {
  // TODO(https://fxbug.dev/42080141): Find out why this test does not work on host in CQ
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAPBSET_READ, CAP_DAC_OVERRIDE)), true);
    ASSERT_EQ(SAFE_SYSCALL_SKIP_ON_EPERM(prctl(PR_CAPBSET_DROP, CAP_DAC_OVERRIDE)), 0);
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAPBSET_READ, CAP_DAC_OVERRIDE)), false);
  });
}

TEST(PrctlTest, AmbientCapabilitiesBasicOperations) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_LOWER, CAP_CHOWN, 0, 0)), 0);
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, CAP_CHOWN, 0, 0)), 0);

    ASSERT_EQ(
        SAFE_SYSCALL_SKIP_ON_EPERM(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_RAISE, CAP_CHOWN, 0, 0)),
        0);  // Requires CAP_SETPCAP ?
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, CAP_CHOWN, 0, 0)), 1);

    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0)), 0);
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, CAP_CHOWN, 0, 0)), 0);
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, CAP_AUDIT_CONTROL, 0, 0)),
              0);
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, CAP_DAC_OVERRIDE, 0, 0)),
              0);
  });
}

TEST(PrctlTest, Dumpable) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // PR_SET_DUMPABLE accepts 0 (disable) or 1 (user).
    EXPECT_THAT(prctl(PR_SET_DUMPABLE, 0), SyscallSucceedsWithValue(0));
    EXPECT_THAT(prctl(PR_GET_DUMPABLE), SyscallSucceedsWithValue(0));

    EXPECT_THAT(prctl(PR_SET_DUMPABLE, 1), SyscallSucceedsWithValue(0));
    EXPECT_THAT(prctl(PR_GET_DUMPABLE), SyscallSucceedsWithValue(1));
  });
}

TEST(PrctlTest, KeepCaps) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Initial keepcaps should be 0.
    EXPECT_THAT(prctl(PR_GET_KEEPCAPS), SyscallSucceedsWithValue(0));

    // Setting keepcaps to 1 should update both PR_GET_KEEPCAPS and PR_GET_SECUREBITS.
    EXPECT_THAT(prctl(PR_SET_KEEPCAPS, 1), SyscallSucceedsWithValue(0));
    EXPECT_THAT(prctl(PR_GET_KEEPCAPS), SyscallSucceedsWithValue(1));
    int securebits = SAFE_SYSCALL(prctl(PR_GET_SECUREBITS));
    EXPECT_TRUE(securebits & SECBIT_KEEP_CAPS);

    // Setting keepcaps to 0 should clear it.
    EXPECT_THAT(prctl(PR_SET_KEEPCAPS, 0), SyscallSucceedsWithValue(0));
    EXPECT_THAT(prctl(PR_GET_KEEPCAPS), SyscallSucceedsWithValue(0));
    securebits = SAFE_SYSCALL(prctl(PR_GET_SECUREBITS));
    EXPECT_FALSE(securebits & SECBIT_KEEP_CAPS);

    // Any value other than 0 or 1 should fail with EINVAL.
    EXPECT_THAT(prctl(PR_SET_KEEPCAPS, 2), SyscallFailsWithErrno(EINVAL));
    EXPECT_THAT(prctl(PR_SET_KEEPCAPS, -1), SyscallFailsWithErrno(EINVAL));
  });
}

TEST(PrctlTest, NoNewPrivs) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Initially no_new_privs is not enabled.
    EXPECT_THAT(prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0), SyscallSucceedsWithValue(0));

    // PR_GET_NO_NEW_PRIVS with non-zero extra arguments should return EINVAL.
    EXPECT_THAT(prctl(PR_GET_NO_NEW_PRIVS, 1, 0, 0, 0), SyscallFailsWithErrno(EINVAL));

    // PR_SET_NO_NEW_PRIVS with arg2 != 1 should fail with EINVAL.
    EXPECT_THAT(prctl(PR_SET_NO_NEW_PRIVS, 0, 0, 0, 0), SyscallFailsWithErrno(EINVAL));
    EXPECT_THAT(prctl(PR_SET_NO_NEW_PRIVS, 2, 0, 0, 0), SyscallFailsWithErrno(EINVAL));

    // PR_SET_NO_NEW_PRIVS with extra non-zero arguments should fail with EINVAL.
    EXPECT_THAT(prctl(PR_SET_NO_NEW_PRIVS, 1, 1, 0, 0), SyscallFailsWithErrno(EINVAL));

    // Successfully enable no_new_privs.
    EXPECT_THAT(prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0), SyscallSucceedsWithValue(0));
    EXPECT_THAT(prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0), SyscallSucceedsWithValue(1));
  });
}

class CapGetSetTest : public ::testing::Test {
 protected:
  void SetUp() override {
    memset(&header_, 0, sizeof(header_));
    memset(&caps_, 0, sizeof(caps_));
  }

  __user_cap_header_struct header_;
  __user_cap_data_struct caps_[_LINUX_CAPABILITY_U32S_3];
};

/// `capget` should succeed when the header is valid and the target
/// thread is the caller.
TEST_F(CapGetSetTest, CapGet) {
  // The calling process can be referenced using either a header
  // `pid` value of 0, or the caller's PID.
  header_.version = _LINUX_CAPABILITY_VERSION_3;
  header_.pid = 0;
  EXPECT_THAT(syscall(SYS_capget, &header_, &caps_), SyscallSucceeds());

  header_.pid = getpid();
  EXPECT_THAT(syscall(SYS_capget, &header_, &caps_), SyscallSucceeds());
}

/// `capget` should succeed when the header is valid and the target
/// thread is different from the caller.
TEST_F(CapGetSetTest, CapGetDifferentPid) {
  test_helper::ForkHelper helper;

  pid_t parent_pid = getpid();

  helper.RunInForkedProcess([&parent_pid, this] {
    header_.version = _LINUX_CAPABILITY_VERSION_3;
    header_.pid = parent_pid;
    EXPECT_THAT(syscall(SYS_capget, &header_, &caps_), SyscallSucceeds());
  });
}

/// `capget` populates the header version field with the preferred capability
/// version and fails with EINVAL when the header contains an invalid capability
/// version and the pointer to the capability data struct is non-null.
TEST_F(CapGetSetTest, CapGetInvalidCapabilityVersion) {
  EXPECT_THAT(syscall(SYS_capget, &header_, &caps_), SyscallFailsWithErrno(EINVAL));
  EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_3));
}

/// `capget` fails with EINVAL when the header contains an invalid pid
/// and the pointer to the capability data struct is non-null.
TEST_F(CapGetSetTest, CapGetInvalidPid) {
  header_.version = _LINUX_CAPABILITY_VERSION_3;
  header_.pid = -1;
  EXPECT_THAT(syscall(SYS_capget, &header_, &caps_), SyscallFailsWithErrno(EINVAL));
}

/// `capget` fails with EINVAL when the header contains an invalid pid
/// and version, and the pointer to the capability data struct is non-null.
/// The header is updated with the preferred capability version.
/// TODO(https://fxbug.dev/452426191): Modify the header on Starnix in this case.
TEST_F(CapGetSetTest, CapGetInvalidVersionAndPid) {
  header_.pid = -1;
  EXPECT_THAT(syscall(SYS_capget, &header_, &caps_), SyscallFailsWithErrno(EINVAL));
  EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_3));
}

/// `capget` fails with EFAULT when the pointer to the header argument is null,
/// whether or not the pointer to the capability data struct is non-null.
TEST_F(CapGetSetTest, CapGetNullHeader) {
  EXPECT_THAT(syscall(SYS_capget, NULL, &caps_), SyscallFailsWithErrno(EFAULT));
  EXPECT_THAT(syscall(SYS_capget, NULL, NULL), SyscallFailsWithErrno(EFAULT));
}

/// `capget` succeeds and does not modify the header's version field when
/// the provided header contains a valid but non-preferred capability version.
TEST_F(CapGetSetTest, CapGetNonPreferredHeaderVersion) {
  header_.version = _LINUX_CAPABILITY_VERSION_1;
  EXPECT_THAT(syscall(SYS_capget, &header_, &caps_), SyscallSucceeds());
  EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_1));
}

/// `capget` succeeds and does not modify the header's version field when
/// the provided header contains a valid version and the pointer to the
/// capability data struct is null.
/// TODO(https://fxbug.dev/452426191): Don't modify the header on Starnix.
TEST_F(CapGetSetTest, CapGetNullUserDataAndValidHeaderVersion) {
  header_.version = _LINUX_CAPABILITY_VERSION_1;
  EXPECT_THAT(syscall(SYS_capget, &header_, NULL), SyscallSucceeds());
  EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_1));
}

/// `capget` succeeds and populates the header's version field with the
/// preferred capability version when the header contains an invalid version
/// and a valid pid, and the pointer to the capability data struct is null.
TEST_F(CapGetSetTest, CapGetNullUserDataAndInvalidHeaderVersion) {
  EXPECT_THAT(syscall(SYS_capget, &header_, NULL), SyscallSucceeds());
  EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_3));
}

/// `capget` succeeds and populates the header's version field with the
/// preferred capability version when the header contains an invalid version
/// and an invalid pid, and the pointer to the capability data struct is null.
TEST_F(CapGetSetTest, CapGetNullUserDataAndInvalidHeaderVersionAndPid) {
  header_.pid = -1;
  EXPECT_THAT(syscall(SYS_capget, &header_, NULL), SyscallSucceeds());
  EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_3));
}

/// `capset` should succeed when the target PID is the same as the caller's,
/// the header version is valid, and the new capability set is consistent
/// with the rules described in https://man7.org/linux/man-pages/man2/capget.2.html.
TEST_F(CapGetSetTest, CapSet) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([this] {
    header_.version = _LINUX_CAPABILITY_VERSION_3;

    header_.pid = 0;
    EXPECT_THAT(syscall(SYS_capset, &header_, &caps_), SyscallSucceeds());

    header_.pid = getpid();
    EXPECT_THAT(syscall(SYS_capset, &header_, &caps_), SyscallSucceeds());
  });
}

// Attempting to set capabilities on a thread other than the caller should
// cause `capset` to fail with `EPERM`. For kernels that support file capabilities,
// this is true whether or not the caller has the `CAP_SETPCAP` capability;
// otherwise, callers with `CAP_SETPCAP` may set capabilities for other threads.
// A negative PID should result in either success (in the case that file caps are
// not supported, and assuming sufficient privilege) or `EPERM`.
//
// TODO(https://fxbug.dev/453731091): Include a test involving `CAP_SETPCAP`
// once we have decided which behavior we need, and test success cases for negative
// PIDs if file caps are not supported.
TEST_F(CapGetSetTest, CapSetDifferentPid) {
  test_helper::ForkHelper helper;

  pid_t parent_pid = getpid();

  helper.RunInForkedProcess([&parent_pid, this] {
    header_.version = _LINUX_CAPABILITY_VERSION_3;
    header_.pid = parent_pid;
    EXPECT_THAT(syscall(SYS_capset, &header_, &caps_), SyscallFailsWithErrno(EPERM));

    header_.pid = -1;
    EXPECT_THAT(syscall(SYS_capset, &header_, &caps_), SyscallFailsWithErrno(EPERM));
  });
}

/// `capset` populates the header version field with the preferred capability
/// version and fails with EINVAL when the header contains an invalid capability
/// version and the pointer to the capability data struct is non-null.
TEST_F(CapGetSetTest, CapSetInvalidCapabilityVersion) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([this] {
    EXPECT_THAT(syscall(SYS_capset, &header_, &caps_), SyscallFailsWithErrno(EINVAL));
    EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_3));
  });
}

/// `capset` populates the header version field with the preferred capability
/// version and fails with EINVAL when the header contains an invalid capability
/// version and the target PID is different from the caller's PID.
/// TODO(https://fxbug.dev/452426191): Modify the header on Starnix in this case.
TEST_F(CapGetSetTest, CapSetInvalidCapabilityVersionAndDifferentPid) {
  test_helper::ForkHelper helper;

  pid_t parent_pid = getpid();

  helper.RunInForkedProcess([&parent_pid, this] {
    header_.pid = parent_pid;
    EXPECT_THAT(syscall(SYS_capset, &header_, &caps_), SyscallFailsWithErrno(EINVAL));
    EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_3));
  });
}

/// `capset` fails with `EFAULT` when the provided header is valid and the
/// pointer to the capability data is null.
TEST_F(CapGetSetTest, CapSetNullData) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([this] {
    header_.version = _LINUX_CAPABILITY_VERSION_3;
    EXPECT_THAT(syscall(SYS_capset, &header_, NULL), SyscallFailsWithErrno(EFAULT));
  });
}

/// `capset` fails with `EINVAL` when the provided header has an invalid
/// capability version and the pointer to the capability data is null.
/// The header version is set to the preferred capability version.
TEST_F(CapGetSetTest, CapSetInvalidCapabilityVersionAndNullData) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([this] {
    EXPECT_THAT(syscall(SYS_capset, &header_, NULL), SyscallFailsWithErrno(EINVAL));
    EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_3));
  });
}

/// `capset` fails with `EINVAL` when the provided header has a non-permitted
/// target PID and the pointer to the capability data is null.
TEST_F(CapGetSetTest, CapSetDifferentPidAndNullData) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([this] {
    header_.version = _LINUX_CAPABILITY_VERSION_3;
    header_.pid = -1;
    EXPECT_THAT(syscall(SYS_capset, &header_, NULL), SyscallFailsWithErrno(EPERM));
  });
}

// `capset` should fail with `EPERM` on attempts to add a new capability to the
// permitted set.
TEST_F(CapGetSetTest, CapSetExpandPermittedSet) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([this] {
    header_.version = _LINUX_CAPABILITY_VERSION_3;
    ASSERT_THAT(syscall(SYS_capget, &header_, &caps_), SyscallSucceeds());

    // Drop the `CAP_SYS_ADMIN` capability from the effective and permitted sets.
    caps_[CAP_TO_INDEX(CAP_SYS_ADMIN)].effective &= ~CAP_TO_MASK(CAP_SYS_ADMIN);
    caps_[CAP_TO_INDEX(CAP_SYS_ADMIN)].permitted &= ~CAP_TO_MASK(CAP_SYS_ADMIN);
    ASSERT_THAT(syscall(SYS_capset, &header_, &caps_), SyscallSucceeds());

    // Attempt to add the `CAP_SYS_ADMIN` capability back to the permitted set.
    caps_[CAP_TO_INDEX(CAP_SYS_ADMIN)].permitted |= CAP_TO_MASK(CAP_SYS_ADMIN);
    EXPECT_THAT(syscall(SYS_capset, &header_, &caps_), SyscallFailsWithErrno(EPERM));

    // The same request with an invalid `version` field in the header should
    // result in failure with `EINVAL`.
    header_.version = 0;
    EXPECT_THAT(syscall(SYS_capset, &header_, &caps_), SyscallFailsWithErrno(EINVAL));
    EXPECT_EQ(header_.version, static_cast<__u32>(_LINUX_CAPABILITY_VERSION_3));
  });
}

TEST(PrctlTest, SecurebitNoCapAmbientRaisePreventsRaisingAmbientCaps) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running as root";
  }
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&]() {
    SAFE_SYSCALL(setresuid(0, 0, 0));

    test_helper::SetCapabilityEffective(CAP_SETPCAP);
    test_helper::SetCapabilityInheritable(CAP_AUDIT_READ);
    test_helper::SetCapabilityPermitted(CAP_AUDIT_READ);

    SAFE_SYSCALL(prctl(PR_SET_SECUREBITS, SECBIT_NO_CAP_AMBIENT_RAISE));

    EXPECT_THAT(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_RAISE, CAP_AUDIT_READ, 0, 0),
                SyscallFailsWithErrno(EPERM));
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

struct SecurebitParam {
  int bit;
  int lock_bit;
  const char *name;
};

class SecurebitsLockedTest : public ::testing::TestWithParam<SecurebitParam> {};

TEST_P(SecurebitsLockedTest, LockedBitPreventsSettingBit) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running as root";
  }
  SecurebitParam param = GetParam();
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&]() {
    SAFE_SYSCALL(setresuid(0, 0, 0));
    test_helper::SetCapabilityEffective(CAP_SETPCAP);

    SAFE_SYSCALL(prctl(PR_SET_SECUREBITS, param.lock_bit));

    EXPECT_THAT(prctl(PR_SET_SECUREBITS, param.lock_bit | param.bit), SyscallFailsWithErrno(EPERM));

    EXPECT_EQ(SAFE_SYSCALL(prctl(PR_GET_SECUREBITS)), param.lock_bit);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_P(SecurebitsLockedTest, LockedBitPreventsClearingBit) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running as root";
  }
  SecurebitParam param = GetParam();
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&]() {
    SAFE_SYSCALL(setresuid(0, 0, 0));
    test_helper::SetCapabilityEffective(CAP_SETPCAP);

    SAFE_SYSCALL(prctl(PR_SET_SECUREBITS, param.bit));
    SAFE_SYSCALL(prctl(PR_SET_SECUREBITS, param.bit | param.lock_bit));

    EXPECT_THAT(prctl(PR_SET_SECUREBITS, param.lock_bit), SyscallFailsWithErrno(EPERM));

    EXPECT_EQ(SAFE_SYSCALL(prctl(PR_GET_SECUREBITS)), param.bit | param.lock_bit);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_P(SecurebitsLockedTest, LockedBitCannotBeCleared) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Not running as root";
  }
  SecurebitParam param = GetParam();
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&]() {
    SAFE_SYSCALL(setresuid(0, 0, 0));
    test_helper::SetCapabilityEffective(CAP_SETPCAP);

    SAFE_SYSCALL(prctl(PR_SET_SECUREBITS, param.lock_bit));

    // Try to clear the lock bit (by setting securebits to 0).
    EXPECT_THAT(prctl(PR_SET_SECUREBITS, 0), SyscallFailsWithErrno(EPERM));

    // Verify the lock bit is still set.
    EXPECT_EQ(SAFE_SYSCALL(prctl(PR_GET_SECUREBITS)), param.lock_bit);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

INSTANTIATE_TEST_SUITE_P(
    AllBits, SecurebitsLockedTest,
    ::testing::Values(SecurebitParam{SECBIT_NOROOT, SECBIT_NOROOT_LOCKED, "NOROOT"},
                      SecurebitParam{SECBIT_NO_SETUID_FIXUP, SECBIT_NO_SETUID_FIXUP_LOCKED,
                                     "NO_SETUID_FIXUP"},
                      SecurebitParam{SECBIT_NO_CAP_AMBIENT_RAISE,
                                     SECBIT_NO_CAP_AMBIENT_RAISE_LOCKED, "NO_CAP_AMBIENT_RAISE"}),
    [](const ::testing::TestParamInfo<SecurebitsLockedTest::ParamType> &info) {
      return info.param.name;
    });

}  // namespace
