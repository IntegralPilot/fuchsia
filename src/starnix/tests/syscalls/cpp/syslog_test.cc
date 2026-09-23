// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/stdcompat/string_view.h>
#include <poll.h>
#include <string.h>
#include <sys/klog.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/prctl.h>
#include <linux/securebits.h>

#include "src/lib/files/file.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

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

class SyslogTest : public ::testing::Test {
 public:
  void SetUp() override {
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    if (getuid() != 0) {
      GTEST_SKIP() << "Can only be run as root";
    }
  }
};

TEST_F(SyslogTest, ReadDevKmsg) {
  int kmsg_fd = open("/dev/kmsg", O_RDWR);
  if (kmsg_fd < 0) {
    fprintf(stderr, "Failed to open /dev/kmsg for writing: %s\n", strerror(errno));
    FAIL();
  }
  const char *message = "Hello from the dev/kmsg test\n";
  write(kmsg_fd, message, strlen(message));

  char read_buffer[4096];
  do {
    size_t size_read = read(kmsg_fd, read_buffer, sizeof(read_buffer));
    ASSERT_GT(size_read, 0ul);
  } while (strstr(message, "Hello from the dev/kmsg test") == nullptr);

  close(kmsg_fd);
}

TEST_F(SyslogTest, SyslogReadAll) {
  int kmsg_fd = open("/dev/kmsg", O_WRONLY);
  if (kmsg_fd < 0) {
    fprintf(stderr, "Failed to open /dev/kmsg -> %s\n", strerror(errno));
    FAIL();
  }
  const char *message = "Hello from the read-all test\n";
  write(kmsg_fd, message, strlen(message));
  close(kmsg_fd);

  int size = klogctl(SYSLOG_ACTION_SIZE_BUFFER, nullptr, 0);
  std::string buf;
  buf.resize(size);

  // Logging is an asynchronous process, so we must loop.
  do {
    int size_read = klogctl(SYSLOG_ACTION_READ_ALL, buf.data(), static_cast<int>(buf.size()));
    if (size_read <= 0) {
      fprintf(stderr, "Failed to read: %s\n", strerror(errno));
      FAIL();
    }
  } while (!cpp23::contains(buf, "Hello from the read-all test"));
}

TEST_F(SyslogTest, Read) {
  int kmsg_fd = open("/dev/kmsg", O_RDWR);
  if (kmsg_fd < 0) {
    fprintf(stderr, "Failed to open /dev/kmsg for writing: %s\n", strerror(errno));
    FAIL();
  }

  // Write a first log.
  const char *first_message = "SyslogRead -- first\n";
  write(kmsg_fd, first_message, strlen(first_message));

  //// Read that first log we wrote.
  char buf[4096];
  do {
    int size_read = klogctl(SYSLOG_ACTION_READ, buf, sizeof(buf));
    ASSERT_GT(size_read, 0);
  } while (strstr(buf, "SyslogRead -- first") == nullptr);

  // Write a second log.
  const char *second_message = "SyslogRead -- second\n";
  write(kmsg_fd, second_message, strlen(second_message));

  // Check that the first log we syslog(READ) from isn't present anymore.
  do {
    std::fill_n(buf, 4096, 0);
    int size_read = klogctl(SYSLOG_ACTION_READ, buf, sizeof(buf));
    ASSERT_GT(size_read, 0);
    EXPECT_EQ(strstr(buf, "SyslogRead -- first"), nullptr);
  } while (strstr(buf, "SyslogRead -- second") == nullptr);

  // Check that all logs are present when reading from /dev/kmsg.
  do {
    std::fill_n(buf, 4096, 0);
    size_t size_read = read(kmsg_fd, buf, sizeof(buf));
    ASSERT_GT(size_read, 0ul);
  } while (strstr(buf, "SyslogRead -- first") == nullptr);

  while (strstr(buf, "SyslogRead -- second") == nullptr) {
    size_t size_read = read(kmsg_fd, buf, sizeof(buf));
    ASSERT_GT(size_read, 0ul);
  }

  // Check that all logs are present when reading using SYSLOG_ACTION_READ_ALL
  int size = klogctl(SYSLOG_ACTION_SIZE_BUFFER, nullptr, 0);
  std::string buf_all;
  buf_all.resize(size);
  int size_read = klogctl(SYSLOG_ACTION_READ_ALL, buf_all.data(), static_cast<int>(buf_all.size()));
  if (size_read <= 0) {
    fprintf(stderr, "Failed to read: %s\n", strerror(errno));
    FAIL();
  }
  EXPECT_TRUE(cpp23::contains(buf_all, "SyslogRead -- first"));
  EXPECT_TRUE(cpp23::contains(buf_all, "SyslogRead -- second"));

  close(kmsg_fd);
}

TEST_F(SyslogTest, ReadProcKmsg) {
  int kmsg_fd = open("/dev/kmsg", O_WRONLY);
  if (kmsg_fd < 0) {
    fprintf(stderr, "Failed to open /dev/kmsg -> %s\n", strerror(errno));
    FAIL();
  }
  const char *first_message = "ReadProcKmsg -- log one\n";
  write(kmsg_fd, first_message, strlen(first_message));

  int proc_kmsg_fd = open("/proc/kmsg", O_RDONLY);
  if (proc_kmsg_fd < 0) {
    fprintf(stderr, "Failed to open /proc/kmsg -> %s\n", strerror(errno));
    FAIL();
  }

  // Read that first log we wrote.
  char buf[4096];
  do {
    size_t size_read = read(proc_kmsg_fd, buf, sizeof(buf));
    ASSERT_GT(size_read, 0ul);
  } while (strstr(buf, "ReadProcKmsg -- log one") == nullptr);

  // Write a second log.
  const char *second_message = "ReadProcKmsg -- log two\n";
  write(kmsg_fd, second_message, strlen(second_message));
  close(kmsg_fd);

  // Check that the first log we read isn't present anymore.
  std::fill_n(buf, 4096, 0);
  do {
    size_t size_read = read(proc_kmsg_fd, buf, sizeof(buf));
    ASSERT_GT(size_read, 0ul);
    EXPECT_EQ(strstr(buf, "ReadProcKmsg -- log one"), nullptr);

  } while (strstr(buf, "ReadProcKmsg -- log two") == nullptr);

  close(proc_kmsg_fd);
}

TEST_F(SyslogTest, NonBlockingRead) {
  int fd = open("/dev/kmsg", O_RDONLY | O_NONBLOCK);
  char buf[4096];
  ssize_t size_read = 0;
  while (size_read != -1) {
    size_read = read(fd, buf, sizeof(buf));
  }
  EXPECT_EQ(errno, EAGAIN);
  close(fd);
}

TEST_F(SyslogTest, ProcKmsgPoll) {
  int kmsg_fd = open("/dev/kmsg", O_WRONLY);
  if (kmsg_fd < 0) {
    fprintf(stderr, "Failed to open /dev/kmsg -> %s\n", strerror(errno));
    FAIL();
  }
  const char *first_message = "ProcKmsgPoll -- log one\n";
  write(kmsg_fd, first_message, strlen(first_message));

  int proc_kmsg_fd = open("/proc/kmsg", O_RDONLY);

  // Drain the logs.
  char buf[4096];
  do {
    size_t size_read = read(proc_kmsg_fd, buf, sizeof(buf));
    ASSERT_GT(size_read, 0ul);
  } while (strstr(buf, "ProcKmsgPoll -- log one") == nullptr);

  struct pollfd fds[] = {{
      .fd = proc_kmsg_fd,
      .events = POLLIN,
      .revents = 42,
  }};

  // With no timeout, this returns immediately.
  EXPECT_EQ(0, poll(fds, 1, 0));

  // Ensure syslog returns that the unread size is 0.
  EXPECT_EQ(0, klogctl(SYSLOG_ACTION_SIZE_UNREAD, nullptr, 0));

  // Write a log.
  const char *second_message = "ProcKmsgPoll -- log two\n";
  write(kmsg_fd, second_message, strlen(second_message));

  // Wait for the log to be ready to read.
  EXPECT_EQ(1, poll(fds, 1, -1));
  EXPECT_EQ(POLLIN, fds[0].revents);

  // Syslog isn't empty anymore.
  EXPECT_GT(klogctl(SYSLOG_ACTION_SIZE_UNREAD, nullptr, 0), 0);

  close(kmsg_fd);
  close(proc_kmsg_fd);
}

TEST_F(SyslogTest, DevKmsgSeekSet) {
  int fd = open("/dev/kmsg", O_RDWR);
  if (fd < 0) {
    fprintf(stderr, "Failed to open /dev/kmsg for writing: %s\n", strerror(errno));
    FAIL();
  }
  const char *message = "DevKmsgSeekSet: hello\n";
  write(fd, message, strlen(message));

  // Advance until we have read the log written above.
  char buf[4096];
  do {
    size_t size_read = read(fd, buf, sizeof(buf));
    ASSERT_GT(size_read, 0ul);
  } while (strstr(buf, "DevKmsgSeekSet: hello") == nullptr);

  // Seek to the beginning of the log.
  lseek(fd, 0, SEEK_SET);

  // We see the previous log again. If we had not done SEEK_SET,0. This would hang until some
  // unseen log arrives.
  std::fill_n(buf, 4096, 0);
  do {
    size_t size_read = read(fd, buf, sizeof(buf));
    ASSERT_GT(size_read, 0ul);
  } while (strstr(buf, "DevKmsgSeekSet: hello") == nullptr);

  close(fd);
}

TEST_F(SyslogTest, DevKmsgSeekEnd) {
  int fd = open("/dev/kmsg", O_RDWR);
  if (fd < 0) {
    fprintf(stderr, "Failed to open /dev/kmsg for writing: %s\n", strerror(errno));
    FAIL();
  }
  const char *hello_message = "DevKmsgSeekEnd: hello\n";
  write(fd, hello_message, strlen(hello_message));

  // Ensure the log has been written.
  char buf[4096];
  do {
    size_t size_read = read(fd, buf, sizeof(buf));
    ASSERT_GT(size_read, 0ul);
  } while (strstr(buf, "DevKmsgSeekEnd: hello") == nullptr);
  close(fd);

  // Open a new file, and seek to the end of the log.
  fd = open("/dev/kmsg", O_RDWR | O_NONBLOCK);
  if (fd < 0) {
    fprintf(stderr, "Failed to open /dev/kmsg for writing: %s\n", strerror(errno));
    FAIL();
  }

  lseek(fd, 0, SEEK_END);

  const char *bye_message = "DevKmsgSeekEnd: bye\n";
  write(fd, bye_message, strlen(bye_message));

  // We should see the second log but never the first one.
  std::fill_n(buf, 4096, 0);
  do {
    size_t size_read = read(fd, buf, sizeof(buf));
    ASSERT_GT(size_read, 0ul);
    EXPECT_EQ(strstr(buf, "DevKmsgSeekEnd: hello"), nullptr);
  } while (strstr(buf, "DevKmsgSeekEnd: bye") == nullptr);

  close(fd);
}

class SyslogAccessTest : public ::testing::TestWithParam<std::tuple<bool, bool, bool, bool, bool>> {
 protected:
  void SetUp() override {
    if (getuid() != 0) {
      skipped_ = true;
      GTEST_SKIP() << "Can only be run as root";
    }

    dev_kmsg_ = fbl::unique_fd(open("/dev/kmsg", O_WRONLY));
    ASSERT_TRUE(dev_kmsg_.is_valid());

    std::tie(is_root_, has_cap_sys_admin_, has_cap_syslog_, has_dac_override_, dmesg_restrict_) =
        GetParam();

    EXPECT_THAT(prctl(PR_SET_SECUREBITS, SECBIT_KEEP_CAPS), SyscallSucceeds());
    ASSERT_TRUE(
        files::ReadFileToString("/proc/sys/kernel/dmesg_restrict", &previous_dmesg_restrict_));
    ASSERT_TRUE(files::WriteFile("/proc/sys/kernel/dmesg_restrict", dmesg_restrict_ ? "1" : "0"));
    if (!is_root_) {
      EXPECT_THAT(seteuid(1), SyscallSucceeds());
    }
    if (has_cap_syslog_) {
      test_helper::SetCapabilityEffective(CAP_SYSLOG);
    } else {
      test_helper::UnsetCapabilityEffective(CAP_SYSLOG);
    }
    if (has_cap_sys_admin_) {
      test_helper::SetCapabilityEffective(CAP_SYS_ADMIN);
    } else {
      test_helper::UnsetCapabilityEffective(CAP_SYS_ADMIN);
    }
    if (has_dac_override_) {
      test_helper::SetCapabilityEffective(CAP_DAC_READ_SEARCH);
      test_helper::SetCapabilityEffective(CAP_DAC_OVERRIDE);
    } else {
      test_helper::UnsetCapabilityEffective(CAP_DAC_READ_SEARCH);
      test_helper::UnsetCapabilityEffective(CAP_DAC_OVERRIDE);
    }
  }

  void TearDown() override {
    if (skipped_) {
      return;
    }

    EXPECT_THAT(seteuid(0), SyscallSucceeds());
    test_helper::SetCapabilityEffective(CAP_SYSLOG);
    test_helper::SetCapabilityEffective(CAP_SYS_ADMIN);
    test_helper::SetCapabilityEffective(CAP_DAC_OVERRIDE);
    test_helper::SetCapabilityEffective(CAP_DAC_READ_SEARCH);
    EXPECT_TRUE(files::WriteFile("/proc/sys/kernel/dmesg_restrict", previous_dmesg_restrict_));
    EXPECT_THAT(prctl(PR_SET_SECUREBITS, 0), SyscallSucceeds());
  }

  bool is_root_;
  bool has_cap_sys_admin_;
  bool has_cap_syslog_;
  bool has_dac_override_;
  bool dmesg_restrict_;
  fbl::unique_fd dev_kmsg_;

 private:
  bool skipped_ = false;
  std::string previous_dmesg_restrict_;
};

TEST_P(SyslogAccessTest, ProcKmsgRead) {
  struct stat sb = {};
  EXPECT_GE(stat("/proc/kmsg", &sb), 0);
  EXPECT_EQ(sb.st_mode & 0777, 0400u);

  fbl::unique_fd fd = fbl::unique_fd(open("/proc/kmsg", O_RDONLY | O_NONBLOCK));
  bool expect_readable = has_cap_syslog_ && (is_root_ || has_dac_override_);
  if (expect_readable) {
    ASSERT_TRUE(fd);

    // We can drop CAP_SYSLOG without affecting the access check.
    test_helper::UnsetCapabilityEffective(CAP_SYSLOG);
    char buf;
    EXPECT_THAT(read(fd.get(), &buf, 1), AnyOf(SyscallSucceeds(), SyscallFailsWithErrno(EAGAIN)));
  } else {
    EXPECT_FALSE(fd);
  }
}

TEST_P(SyslogAccessTest, ProcKmsgWrite) {
  fbl::unique_fd fd = fbl::unique_fd(open("/proc/kmsg", O_WRONLY));
  bool expect_writable = has_cap_syslog_ & has_dac_override_;
  if (expect_writable) {
    ASSERT_TRUE(fd);
    char buf = 'A';
    // /proc/kmsg is not actually writable.
    EXPECT_THAT(write(fd.get(), &buf, 1), SyscallFailsWithErrno(EIO));
  } else {
    EXPECT_FALSE(fd);
  }
}

TEST_P(SyslogAccessTest, ProcKmsgTrunc) {
  fbl::unique_fd fd = fbl::unique_fd(open("/proc/kmsg", O_WRONLY | O_TRUNC));
  bool expect_writable = has_cap_syslog_ & has_dac_override_;
  EXPECT_EQ(expect_writable, fd.is_valid()) << strerror(errno);
}

TEST_P(SyslogAccessTest, ProcKmsgReadWrite) {
  fbl::unique_fd fd = fbl::unique_fd(open("/proc/kmsg", O_RDWR));
  bool expect_writable = has_cap_syslog_ & has_dac_override_;
  EXPECT_EQ(expect_writable, fd.is_valid()) << strerror(errno);
}

TEST_P(SyslogAccessTest, DevKmsgRead) {
  struct stat sb;
  ASSERT_THAT(stat("/dev/kmsg", &sb), SyscallSucceeds());
  EXPECT_EQ(sb.st_uid, 0u);
  bool permissions_ok =
      (is_root_ && (sb.st_mode & 0400)) || (!is_root_ && (sb.st_mode & 0004)) || has_dac_override_;

  fbl::unique_fd fd = fbl::unique_fd(open("/dev/kmsg", O_RDONLY | O_NONBLOCK));

  bool expect_readable = permissions_ok && (has_cap_syslog_ || !dmesg_restrict_);
  if (expect_readable) {
    ASSERT_TRUE(fd);

    test_helper::UnsetCapabilityEffective(CAP_SYSLOG);
    char buf[4096];
    EXPECT_THAT(read(fd.get(), &buf, 4096),
                AnyOf(SyscallSucceeds(), SyscallFailsWithErrno(EAGAIN)));
  } else {
    EXPECT_FALSE(fd);
  }
}

TEST_P(SyslogAccessTest, DevKmsgWrite) {
  struct stat sb;
  ASSERT_THAT(stat("/dev/kmsg", &sb), SyscallSucceeds());
  EXPECT_EQ(sb.st_uid, 0u);
  bool permissions_ok =
      (is_root_ && (sb.st_mode & 0200)) || (!is_root_ && (sb.st_mode & 0002)) || has_dac_override_;

  fbl::unique_fd fd = fbl::unique_fd(open("/dev/kmsg", O_WRONLY));

  bool expect_writable = permissions_ok;
  if (expect_writable) {
    ASSERT_TRUE(fd);

    const char message[] = "hello, world\n";
    EXPECT_THAT(write(fd.get(), &message, strlen(message)), SyscallSucceeds());
  } else {
    EXPECT_FALSE(fd);
  }
}

TEST_P(SyslogAccessTest, DevKmsgTrunc) {
  struct stat sb;
  ASSERT_THAT(stat("/dev/kmsg", &sb), SyscallSucceeds());
  EXPECT_EQ(sb.st_uid, 0u);
  bool permissions_ok =
      (is_root_ && (sb.st_mode & 0200)) || (!is_root_ && (sb.st_mode & 0002)) || has_dac_override_;

  fbl::unique_fd fd = fbl::unique_fd(open("/dev/kmsg", O_WRONLY | O_TRUNC));

  bool expect_writable = permissions_ok;
  if (expect_writable) {
    EXPECT_TRUE(fd);
  } else {
    EXPECT_FALSE(fd);
  }
}

TEST_P(SyslogAccessTest, SyslogActionClose) {
  bool expect_allowed = has_cap_syslog_;
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CLOSE, 0, 0), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CLOSE, 0, 0), SyscallFailsWithErrno(EPERM));
  }
}

TEST_P(SyslogAccessTest, SyslogActionOpen) {
  bool expect_allowed = has_cap_syslog_;
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_OPEN, 0, 0), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_OPEN, 0, 0), SyscallFailsWithErrno(EPERM));
  }
}

TEST_P(SyslogAccessTest, SyslogActionRead) {
  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectSignal(SIGKILL);

  // In some cases, our messages get removed from the buffer before we can read them. We fork to
  // ensure we get a steady supply of log messages we can read, with at most a 100ms wait.
  pid_t writing_child = fork_helper.RunInForkedProcess([this] {
    do {
      char write_buf[] = "hello, world!\n";
      EXPECT_THAT(write(dev_kmsg_.get(), write_buf, strlen(write_buf)), SyscallSucceeds());
    } while (usleep(100000) == 0);
  });
  bool expect_allowed = has_cap_syslog_;
  char read_buf[4096];
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ, read_buf, sizeof(read_buf)), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ, read_buf, sizeof(read_buf)),
                SyscallFailsWithErrno(EPERM));
  }
  EXPECT_THAT(kill(writing_child, SIGKILL), SyscallSucceeds());
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

TEST_P(SyslogAccessTest, SyslogActionReadAll) {
  bool expect_allowed = has_cap_syslog_ || !dmesg_restrict_;
  char read_buf[4096];
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ_ALL, read_buf, sizeof(read_buf)), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ_ALL, read_buf, sizeof(read_buf)),
                SyscallFailsWithErrno(EPERM));
  }
}

TEST_P(SyslogAccessTest, SyslogActionReadClear) {
  bool expect_allowed = has_cap_syslog_;
  char read_buf[4096];
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ_CLEAR, read_buf, sizeof(read_buf)), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_READ_CLEAR, read_buf, sizeof(read_buf)),
                SyscallFailsWithErrno(EPERM));
  }
}

TEST_P(SyslogAccessTest, SyslogActionClear) {
  bool expect_allowed = has_cap_syslog_;
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CLEAR, 0, 0), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CLEAR, 0, 0), SyscallFailsWithErrno(EPERM));
  }
}

TEST_P(SyslogAccessTest, SyslogActionConsoleOff) {
  bool expect_allowed = has_cap_syslog_;
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_OFF, 0, 0), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_OFF, 0, 0), SyscallFailsWithErrno(EPERM));
  }
}

TEST_P(SyslogAccessTest, SyslogActionConsoleOn) {
  bool expect_allowed = has_cap_syslog_;
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_ON, 0, 0), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_ON, 0, 0), SyscallFailsWithErrno(EPERM));
  }
}

TEST_P(SyslogAccessTest, SyslogActionConsoleLevel) {
  bool expect_allowed = has_cap_syslog_;
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_LEVEL, 0, 7), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_CONSOLE_LEVEL, 0, 7), SyscallFailsWithErrno(EPERM));
  }
}

TEST_P(SyslogAccessTest, SyslogActionSizeUnread) {
  bool expect_allowed = has_cap_syslog_;
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_SIZE_UNREAD, 0, 0), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_SIZE_UNREAD, 0, 0), SyscallFailsWithErrno(EPERM));
  }
}

TEST_P(SyslogAccessTest, SyslogActionSizeBuffer) {
  bool expect_allowed = has_cap_syslog_ || !dmesg_restrict_;
  if (expect_allowed) {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_SIZE_BUFFER, 0, 0), SyscallSucceeds());
  } else {
    EXPECT_THAT(klogctl(SYSLOG_ACTION_SIZE_BUFFER, 0, 0), SyscallFailsWithErrno(EPERM));
  }
}

INSTANTIATE_TEST_SUITE_P(SyslogAccessTestAll, SyslogAccessTest,
                         ::testing::Combine(::testing::Bool(), ::testing::Bool(), ::testing::Bool(),
                                            ::testing::Bool(), ::testing::Bool()),
                         ([](const testing::TestParamInfo<SyslogAccessTest::ParamType> &info) {
                           auto [is_root, has_cap_sys_admin, has_cap_syslog, has_dac_override,
                                 dmesg_restrict] = info.param;
                           return std::string(is_root ? "Root" : "User") + "_" +
                                  (has_cap_sys_admin ? "SysAdmin" : "NoSysAdmin") + "_" +
                                  (has_cap_syslog ? "Syslog" : "NoSyslog") + "_" +
                                  (has_dac_override ? "DacOverride_" : "") +
                                  (dmesg_restrict ? "Restrict" : "NoRestrict");
                         }));

}  // namespace
