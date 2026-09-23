// Copyright 2025 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/klog.h>
#include <sys/sysmacros.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() {
  EXPECT_THAT(mknod("/tmp/dev_kmsg", S_IFCHR | 0600, makedev(1, 11)), SyscallSucceeds());
  return "syslog_policy";
}

namespace {

// See "The symbolic names are defined in the kernel source, but are
// not exported to user space; you will either need to use the numbers,
// or define the names yourself" at syslog(2).
constexpr int SYSLOG_ACTION_CLOSE = 0;
constexpr int SYSLOG_ACTION_OPEN = 1;
constexpr int SYSLOG_ACTION_READ = 2;
constexpr int SYSLOG_ACTION_READ_ALL = 3;
constexpr int SYSLOG_ACTION_READ_CLEAR = 4;
constexpr int SYSLOG_ACTION_CLEAR = 5;
constexpr int SYSLOG_ACTION_CONSOLE_OFF = 6;
constexpr int SYSLOG_ACTION_CONSOLE_ON = 7;
constexpr int SYSLOG_ACTION_CONSOLE_LEVEL = 8;
constexpr int SYSLOG_ACTION_SIZE_UNREAD = 9;
constexpr int SYSLOG_ACTION_SIZE_BUFFER = 10;

TEST(SyslogTest, ProcKmsgAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_mod_t:s0", [&] {
    fbl::unique_fd proc_kmsg = fbl::unique_fd(open("/proc/kmsg", O_RDONLY | O_NONBLOCK));
    ASSERT_TRUE(proc_kmsg) << strerror(errno);
    char buf;
    EXPECT_THAT(read(proc_kmsg.get(), &buf, 1),
                ::testing::AnyOf(SyscallSucceeds(), SyscallFailsWithErrno(EAGAIN)));
  }));
}

TEST(SyslogTest, ProcKmsgDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(open("/proc/kmsg", O_RDONLY | O_NONBLOCK), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, ProcKmsgReadDeniedIfDroppedPermission) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_mod_t:s0", [&] {
    fbl::unique_fd proc_kmsg = fbl::unique_fd(open("/proc/kmsg", O_RDONLY | O_NONBLOCK));
    ASSERT_TRUE(proc_kmsg) << strerror(errno);
    ASSERT_THAT(WriteTaskAttr("current", "test_u:test_r:syslog_test_no_syslog_perm_t:s0"),
                SyscallResultIsOk());
    char buf;
    EXPECT_THAT(read(proc_kmsg.get(), &buf, 1), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, DevKmsgReadAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_read_t:s0", [&] {
    fbl::unique_fd dev_kmsg = fbl::unique_fd(open("/tmp/dev_kmsg", O_RDONLY | O_NONBLOCK));
    ASSERT_TRUE(dev_kmsg) << strerror(errno);
    char buf[4096];
    EXPECT_THAT(read(dev_kmsg.get(), buf, sizeof(buf)),
                ::testing::AnyOf(SyscallSucceeds(), SyscallFailsWithErrno(EAGAIN)));
  }));
}

TEST(SyslogTest, DevKmsgReadDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(open("/tmp/dev_kmsg", O_RDONLY | O_NONBLOCK), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, DevKmsgReadAllowedIfDroppedPermission) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_read_t:s0", [&] {
    fbl::unique_fd dev_kmsg = fbl::unique_fd(open("/tmp/dev_kmsg", O_RDONLY | O_NONBLOCK));
    ASSERT_TRUE(dev_kmsg) << strerror(errno);

    // Permissions are not re-checked after opening.
    ASSERT_THAT(WriteTaskAttr("current", "test_u:test_r:syslog_test_no_syslog_perm_t:s0"),
                SyscallResultIsOk());
    char buf[4096];
    EXPECT_THAT(read(dev_kmsg.get(), buf, sizeof(buf)),
                ::testing::AnyOf(SyscallSucceeds(), SyscallFailsWithErrno(EAGAIN)));
  }));
}

TEST(SyslogTest, DevKmsgWriteAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    fbl::unique_fd dev_kmsg = fbl::unique_fd(open("/tmp/dev_kmsg", O_WRONLY | O_NONBLOCK));
    ASSERT_TRUE(dev_kmsg) << strerror(errno);
    char buf[] = "hello, world\n";
    EXPECT_THAT(write(dev_kmsg.get(), buf, sizeof(buf)), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionCloseDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CLOSE, 0, 0), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionCloseAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_mod_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CLOSE, 0, 0), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionOpenDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_OPEN, 0, 0), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionOpenAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_mod_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_OPEN, 0, 0), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionReadDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    char read_buf[4096];
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ, read_buf, sizeof(read_buf)),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionReadAllowed) {
  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectSignal(SIGKILL);

  fbl::unique_fd dev_kmsg = fbl::unique_fd(open("/tmp/dev_kmsg", O_WRONLY));
  ASSERT_TRUE(dev_kmsg);

  // In some cases, our messages get removed from the buffer before we can read them. We fork to
  // ensure we get a steady supply of log messages we can read, with at most a 100ms wait.
  pid_t writing_child = fork_helper.RunInForkedProcess([&dev_kmsg] {
    do {
      char write_buf[] = "hello, world!\n";
      EXPECT_THAT(write(dev_kmsg.get(), write_buf, strlen(write_buf)), SyscallSucceeds());
    } while (usleep(100000) == 0);
  });

  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_mod_t:s0", [&] {
    char read_buf[4096];
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ, read_buf, sizeof(read_buf)), SyscallSucceeds());
  }));

  EXPECT_THAT(kill(writing_child, SIGKILL), SyscallSucceeds());
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

TEST(SyslogTest, SyslogActionReadAllDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    char read_buf[4096];
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ_ALL, read_buf, sizeof(read_buf)),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionReadAllAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_read_t:s0", [&] {
    char read_buf[4096];
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ_ALL, read_buf, sizeof(read_buf)), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionReadClearDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    char read_buf[4096];
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ_CLEAR, read_buf, sizeof(read_buf)),
                SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionReadClearAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_mod_t:s0", [&] {
    char read_buf[4096];
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ_CLEAR, read_buf, sizeof(read_buf)), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionClearDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CLEAR, 0, 0), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionClearAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_mod_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CLEAR, 0, 0), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionConsoleOffDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_OFF, 0, 0), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionConsoleOffAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_console_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_OFF, 0, 0), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionConsoleOnDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_ON, 0, 0), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionConsoleOnAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_console_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_ON, 0, 0), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionConsoleLevelDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_LEVEL, 0, 7), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionConsoleLevelAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_console_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_LEVEL, 0, 7), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionSizeUnreadDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_SIZE_UNREAD, 0, 0), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionSizeUnreadAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_mod_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_SIZE_UNREAD, 0, 0), SyscallSucceeds());
  }));
}

TEST(SyslogTest, SyslogActionSizeBufferDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_no_syslog_perm_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_SIZE_BUFFER, 0, 0), SyscallFailsWithErrno(EACCES));
  }));
}

TEST(SyslogTest, SyslogActionSizeBufferAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:syslog_test_has_syslog_read_t:s0", [&] {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_SIZE_BUFFER, 0, 0), SyscallSucceeds());
  }));
}

}  // namespace
