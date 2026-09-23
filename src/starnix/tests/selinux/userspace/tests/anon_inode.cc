// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/epoll.h>
#include <sys/eventfd.h>
#include <sys/inotify.h>
#include <sys/signalfd.h>
#include <sys/syscall.h>
#include <sys/timerfd.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/perf_event.h>
#include <linux/userfaultfd.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace {

int g_before_policy_userfaultfd = -1;

TEST(AnonInodeTest, EventFdIsUnlabeled) {
  fbl::unique_fd fd(eventfd(0, 0));
  ASSERT_TRUE(fd.is_valid());

  EXPECT_THAT(GetLabel(fd.get()), SyscallResultIsErrno(ENOTSUP));
}

TEST(AnonInodeTest, PrivateFdIsUnchecked) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  // Create an eventfd within a test domain, then validate whether the FD is usable from a set
  // of test domains with differing levels of access.
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:anon_inode_test_t:s0", [] {
    fbl::unique_fd fd(eventfd(0, 0));
    ASSERT_TRUE(fd.is_valid());
    auto fd_label = GetLabel(fd.get());

    // Ensure that `fd` is of an un-labeled, aka "private", kind.
    ASSERT_THAT(fd_label, SyscallResultIsErrno(ENOTSUP));

    uint64_t event_buf = 1;

    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:anon_inode_use_fd_and_perms_t:s0", [&] {
      EXPECT_THAT(write(fd.get(), &event_buf, sizeof(event_buf)), SyscallSucceeds())
          << "Domain granted FD-use and permissions should have access";
    }));
    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:anon_inode_use_fd_no_perms_t:s0", [&] {
      EXPECT_THAT(write(fd.get(), &event_buf, sizeof(event_buf)), SyscallSucceeds())
          << "Domain granted FD-use but no file node permissions should have access";
    }));
    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:anon_inode_no_use_fd_t:s0", [&] {
      EXPECT_THAT(write(fd.get(), &event_buf, sizeof(event_buf)), SyscallFailsWithErrno(EACCES))
          << "Domain not granted FD-use should not have access";
    }));
  }));
}

TEST(AnonInodeTest, TmpFileHasLabel) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:anon_inode_test_t:s0", [] {
    constexpr char kTmpPath[] = "/tmp";

    fbl::unique_fd fd(open(kTmpPath, O_RDWR | O_TMPFILE, 0o600));
    ASSERT_TRUE(fd.is_valid());

    EXPECT_THAT(GetLabel(fd.get()), SyscallResultIsOk());
  }));
}

TEST(AnonInodeTest, UserfaultFdHasLabel) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:anon_inode_test_t:s0", [] {
    fbl::unique_fd fd(static_cast<int>(syscall(SYS_userfaultfd, O_CLOEXEC)));
    ASSERT_TRUE(fd.is_valid());

    EXPECT_THAT(GetLabel(fd.get()),
                SyscallResultIsOk("test_u:object_r:anon_inode_test_userfaultfd_t:s0"));
  }));
}

TEST(AnonInodeTest, UserfaultFdRequiresCreate) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:anon_inode_test_no_create_t:s0", [] {
    EXPECT_THAT(syscall(SYS_userfaultfd, O_CLOEXEC), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(AnonInodeTest, EpollIsUnlabeled) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  fbl::unique_fd fd(epoll_create1(0));
  ASSERT_TRUE(fd.is_valid());

  EXPECT_THAT(GetLabel(fd.get()), SyscallResultIsErrno(ENOTSUP));
}

TEST(AnonInodeTest, InotifyIsUnlabeled) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  fbl::unique_fd fd(inotify_init());
  ASSERT_TRUE(fd.is_valid());

  EXPECT_THAT(GetLabel(fd.get()), SyscallResultIsErrno(ENOTSUP));
}

TEST(AnonInodeTest, PidFdIsUnlabeled) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  fbl::unique_fd fd(static_cast<int>(syscall(SYS_pidfd_open, getpid(), 0)));
  ASSERT_TRUE(fd.is_valid());

  EXPECT_THAT(GetLabel(fd.get()), SyscallResultIsErrno(ENOTSUP));
}

TEST(AnonInodeTest, TimerFdIsUnlabeled) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  fbl::unique_fd fd(timerfd_create(CLOCK_MONOTONIC, 0));
  ASSERT_TRUE(fd.is_valid());

  EXPECT_THAT(GetLabel(fd.get()), SyscallResultIsErrno(ENOTSUP));
}

TEST(AnonInodeTest, SignalFdIsUnlabeled) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  sigset_t signals;
  sigemptyset(&signals);
  fbl::unique_fd fd(signalfd(-1, &signals, SFD_CLOEXEC));
  ASSERT_TRUE(fd.is_valid());

  EXPECT_THAT(GetLabel(fd.get()), SyscallResultIsErrno(ENOTSUP));
}

TEST(AnonInodeTest, PerfEventFdIsUnlabeled) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  perf_event_attr attr{};
  attr.type = PERF_TYPE_SOFTWARE;
  attr.size = sizeof(attr);
  attr.config = PERF_COUNT_SW_CPU_CLOCK;
  attr.sample_type = PERF_SAMPLE_IP;
  attr.disabled = true;
  attr.exclude_kernel = true;
  attr.exclude_hv = true;
  attr.exclude_idle = true;

  fbl::unique_fd fd(static_cast<int>(
      syscall(SYS_perf_event_open, &attr, /*pid=*/0, /*cpu=*/-1, /*group_fd=*/-1, /*flags=*/0)));
  ASSERT_TRUE(fd.is_valid());

  EXPECT_THAT(GetLabel(fd.get()), SyscallResultIsErrno(ENOTSUP));
}

TEST(AnonInodeTest, EventFdBeforePolicy) {
  // userfaultfd() created before policy load should have been labeled based on the "kernel" SID.
  EXPECT_THAT(GetLabel(g_before_policy_userfaultfd),
              SyscallResultIsOk("unlabeled_u:unlabeled_r:unlabeled_t:s0"));
}

}  // namespace

extern std::string DoPrePolicyLoadWork() {
  g_before_policy_userfaultfd = static_cast<int>(syscall(SYS_userfaultfd, O_CLOEXEC));
  EXPECT_NE(g_before_policy_userfaultfd, -1)
      << "Failed pre-policy userfaultfd: " << strerror(errno);
  EXPECT_THAT(GetLabel(g_before_policy_userfaultfd), SyscallResultIsErrno(ENOTSUP));

  return "anon_inode_policy";
}
