// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <poll.h>
#include <signal.h>
#include <sys/ioctl.h>
#include <sys/prctl.h>
#include <sys/syscall.h>

#include <array>
#include <condition_variable>
#include <fstream>
#include <mutex>
#include <thread>

#include <gtest/gtest.h>
#include <linux/audit.h>
#include <linux/filter.h>
#include <linux/seccomp.h>

#ifndef SECCOMP_FILTER_FLAG_TSYNC_ESRCH
#define SECCOMP_FILTER_FLAG_TSYNC_ESRCH (1UL << 4)
#endif  // SECCOMP_FILTER_FLAG_TSYNC_ESRCH

#ifndef SYS_SECCOMP
#define SYS_SECCOMP 1
#endif

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

// A syscall not implemented by Linux that we don't expect to be called.
#if defined(__x86_64__)
constexpr uint32_t kFilteredSyscall = SYS_vserver;
#elif defined(__aarch64__) || defined(__riscv) || defined(__arm__)
// Use the first of arch_specific_syscalls. It is not implemented on ARM64 or RISC-V.
// TODO(b/383629319): Newer revisions of bionic seem to cause a build failure as
// we can't find a reference to __NR_arch_specific_syscall. Use the underlying
// number here until we can figure this out. This value is currently sourced
// from: bionic/libc/kernel/uapi/asm-generic/unistd.h.
constexpr uint32_t kFilteredSyscall = 244;
#else
#error Unsupported Architecture
#endif

#define ARRAY_SIZE(x) (sizeof((x)) / sizeof((x[0])))

void exit_with_failure_code() {
  if (testing::Test::HasFatalFailure() || testing::Test::HasNonfatalFailure()) {
    exit(1);
  } else {
    exit(0);
  }
}

// Installs a filter that blocks the given syscall.
void install_filter_block(uint32_t syscall_nr, uint32_t action) {
  EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

  sock_filter filter[] = {
      // A = seccomp_data.nr
      BPF_STMT(BPF_LD | BPF_ABS | BPF_W, offsetof(struct seccomp_data, nr)),
      // if (A == sysno) goto kill
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, syscall_nr, 1, 0),
      // allow: return SECCOMP_RET_ALLOW
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
      // kill: return the provided action
      BPF_STMT(BPF_RET | BPF_K, action),
  };
  sock_fprog prog = {.len = ARRAY_SIZE(filter), .filter = filter};
  EXPECT_EQ(0, syscall(__NR_seccomp, SECCOMP_SET_MODE_FILTER, 0, &prog))
      << "syscall failed with errno " << errno;
}

TEST(SeccompTest, RetTrapBypassesIgn) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.ExpectSignal(SIGSYS);
  helper.RunInForkedProcess([] {
    std::thread t([]() {
      install_filter_block(kFilteredSyscall, SECCOMP_RET_TRAP);

      signal(SIGSYS, SIG_IGN);

      syscall(kFilteredSyscall);
      exit_with_failure_code();
    });
    // Should never join - process should be killed.
    t.join();
  });
}

// Test to ensure strict mode works.
TEST(SeccompTest, Strict) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.ExpectSignal(SIGKILL);
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    // prog argument should be ignored with prctl(SECCOMP_MODE_STRICT)
    EXPECT_EQ(0, prctl(PR_SET_SECCOMP, SECCOMP_MODE_STRICT, &prog, 0, 0));
    syscall(kFilteredSyscall);
  });
}

// Cannot change from Filtered to Strict
TEST(SeccompTest, FilterToStrictErrors) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    ASSERT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {.len = sizeof(filter) / sizeof(sock_filter), .filter = filter};
    EXPECT_EQ(0, syscall(__NR_seccomp, SECCOMP_SET_MODE_FILTER, 0, &prog));

    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_STRICT, 0, 0, 0))
        << "Was able to set a seccomp filter to strict when there was a filter " << errno;

    EXPECT_EQ(EINVAL, errno);
    exit_with_failure_code();
  });
}

// Checks for attempt to install null filter with FILTER, or non-null filter
// with seccomp(SET_MODE_STRICT)
TEST(SeccompTest, BadArgs) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, nullptr, nullptr, nullptr));
    EXPECT_EQ(EFAULT, errno);

    sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, syscall(__NR_seccomp, SECCOMP_SET_MODE_STRICT, 0, &prog));
    EXPECT_EQ(EINVAL, errno);
  });
}

// Test to attempt to install filter without the right caps.
TEST(SeccompTest, BadNoNewPrivs) {
  // Skip this test if we are root.
  if (test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Skipped privs test because running as root";
  }

  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    // Running in locked down mode with no new privs already set - skip.
    if (prctl(PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) == 1) {
      GTEST_SKIP() << "Skipped privs test because privs already set";
    }
    sock_filter filter[] = {
        // A = seccomp_data.nr
        BPF_STMT(BPF_LD | BPF_ABS | BPF_W, offsetof(struct seccomp_data, nr)),
        // if (A == sysno) goto kill
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, __NR_getpid, 1, 0),
        // allow: return SECCOMP_RET_ALLOW
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
        // kill: return SECCOMP_RET_KILL_PROCESS
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL_PROCESS),
    };
    sock_fprog prog = {.len = ARRAY_SIZE(filter), .filter = filter};
    EXPECT_EQ(-1, syscall(__NR_seccomp, SECCOMP_SET_MODE_FILTER, 0, &prog))
        << "syscall failed with errno " << errno;
    EXPECT_EQ(EACCES, errno);
  });
}

TEST(SeccompTest, FilterMax4KMinOne) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));
    constexpr uint64_t too_big = BPF_MAXINSNS + 1;
    auto filter = std::make_unique<sock_filter[]>(too_big);
    sock_filter val = BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    std::fill(filter.get(), filter.get() + too_big, val);

    struct sock_fprog fprog = {.len = too_big, .filter = filter.get()};

    EXPECT_NE(0, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &fprog, 0, 0))
        << "Too large a filter was allowed";

    sock_filter zero_filter[] = {};
    sock_fprog zero_prog = {
        .len = 0,
        .filter = zero_filter,
    };
    EXPECT_NE(0, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &zero_prog, 0, 0))
        << "Zero size filter was allowed";
  });
}

#define MAX_INSNS_PER_PATH 32768

// Ensure that the total number of instructions in filters does not exceed a
// maximum.  In practice, the "32768 instructions" limitation on Linux doesn't
// seem to mean 32768 instructions consistently, so it's acceptable just to use
// that as an upper bound.
TEST(SeccompTest, FilterMaxTotalInsns) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const int kFilterSize = BPF_MAXINSNS;
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));
    auto filter = std::make_unique<sock_filter[]>(kFilterSize);
    sock_filter val = BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW);
    std::fill(filter.get(), filter.get() + kFilterSize, val);

    struct sock_fprog fprog = {.len = kFilterSize, .filter = filter.get()};

    for (int i = 0; i < (MAX_INSNS_PER_PATH / kFilterSize) - 1; i++) {
      prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &fprog, 0, 0);
    }
    // At this point, we've attempted to add enough filters that there should be
    // a failure.

    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &fprog, 0, 0));
    EXPECT_EQ(errno, ENOMEM);
  });
}

// If a BPF operation contains BPF_ABS, then the specified offset has to be
// aligned to a 32-bit boundary and not exceed sizeof(seccomp_data)
TEST(SeccompTest, FilterAccessInBounds) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, sizeof(seccomp_data) + 4),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0))
        << "unaligned abs was allowed in filter";

    EXPECT_EQ(EINVAL, errno) << "unaligned abs did not set set errno correctly";
  });
}

TEST(SeccompTest, RetKillProcess) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.ExpectSignal(SIGSYS);
  helper.RunInForkedProcess([] {
    std::thread t([]() {
      install_filter_block(kFilteredSyscall, SECCOMP_RET_KILL_PROCESS);
      syscall(kFilteredSyscall);
      exit_with_failure_code();
    });
    // Should never join - process should be killed.
    t.join();
  });
}

TEST(SeccompTest, KillProcessOnBadRetAction) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.ExpectSignal(SIGSYS);
  helper.RunInForkedProcess([] {
    std::thread t([]() {
      install_filter_block(kFilteredSyscall, 0x00020000U);
      syscall(kFilteredSyscall);
      exit_with_failure_code();
    });
    // Should never join - process should be killed.
    t.join();
  });
}

TEST(SeccompTest, PrctlGetSeccomp) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.ExpectSignal(SIGSYS);
  helper.RunInForkedProcess([] {
    std::thread t([]() {
      EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

      // prctl(PR_GET_SECCOMP) with no filter returns 0
      EXPECT_EQ(0, prctl(PR_GET_SECCOMP, 0, 0, 0, 0));

      install_filter_block(kFilteredSyscall, SECCOMP_RET_KILL_PROCESS);

      // prctl(PR_GET_SECCOMP) with a filter that does not block prctl returns 2
      EXPECT_EQ(2, prctl(PR_GET_SECCOMP, 0, 0, 0, 0));

      install_filter_block(__NR_prctl, SECCOMP_RET_KILL_PROCESS);

      // We're about to kill the process, so if there weer any failures before this,
      // report them and exit.
      if (testing::Test::HasFatalFailure() || testing::Test::HasNonfatalFailure()) {
        exit(1);
      }

      // prctl(PR_GET_SECCOMP) with a filter that blocks prctl kills the process
      prctl(PR_GET_SECCOMP, 0, 0, 0, 0);
      exit_with_failure_code();
    });
    // Should never join - process should be killed.
    t.join();
  });
}

TEST(SeccompTest, GetActionAvail) {
  uint32_t actions[] = {SECCOMP_RET_ALLOW,       SECCOMP_RET_ERRNO, SECCOMP_RET_KILL_PROCESS,
                        SECCOMP_RET_KILL_THREAD, SECCOMP_RET_LOG,   SECCOMP_RET_TRACE,
                        SECCOMP_RET_TRAP};

  for (unsigned long i = 0; i < ARRAY_SIZE(actions); i++) {
    EXPECT_EQ(0, syscall(__NR_seccomp, SECCOMP_GET_ACTION_AVAIL, 0, actions + i));
  }
  uint32_t illegal = 0xcafebeef;

  EXPECT_EQ(-1, syscall(__NR_seccomp, SECCOMP_GET_ACTION_AVAIL, 0, &illegal));
  EXPECT_EQ(EOPNOTSUPP, errno);
}

TEST(SeccompTest, GetActionsAvailProc) {
  std::ifstream t("/proc/sys/kernel/seccomp/actions_avail");
  std::stringstream buffer;
  buffer << t.rdbuf();
  EXPECT_EQ("kill_process kill_thread trap errno user_notif trace log allow\n", buffer.str());
}

// Read-only test.  There is a test in starnix/task/seccomp.rs that exercises the ability to
// write to this file; it isn't here, because we don't want to change a global config.
TEST(SeccompTest, ActionsLoggedProc) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "Skipped ActionsLoggedProc because not running as root";
  }
  int fd = open("/proc/sys/kernel/seccomp/actions_logged", O_RDWR);
  EXPECT_LT(0, fd);
  // Make sure you can't write nonsense to it.
  const char nonsense[] = "How doth the little crocodile improve its shining tail";
  EXPECT_GT(0, write(fd, nonsense, strlen(nonsense)));
  EXPECT_EQ(EINVAL, errno);
  close(fd);
}

TEST(SeccompTest, ErrnoIsMaxFFF) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    install_filter_block(kFilteredSyscall, SECCOMP_RET_ERRNO | 0x1000);
    syscall(kFilteredSyscall);
    EXPECT_EQ(0xfff, errno);

    exit_with_failure_code();
  });
}

// Test that TSYNC from thread A won't work if you add a filter in thread B,
// spawned from thread A.
TEST(SeccompTest, TsyncEsrch) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    std::mutex m;
    std::condition_variable cv;
    int ready = 0;

    std::thread t([&m, &cv, &ready]() {
      // Step 1: Install filter that will prevent other thread from doing TSYNC
      install_filter_block(kFilteredSyscall, SECCOMP_RET_KILL);
      {
        std::unique_lock l(m);
        ready++;
      }
      // Wake up other thread so it performs TSYNC
      cv.notify_all();

      {
        // Wait around until other thread tries to do TSYNC|TSYNC_ESRCH
        std::unique_lock l(m);
        cv.wait(l, [&ready] { return ready == 2; });
      }
    });

    {
      std::unique_lock l(m);
      if (ready != 1) {
        cv.wait(l, [&ready] { return ready == 1; });
      }
    }

    // Step 2: When t has installed its filter, try to do TSYNC|TSYNC_ESRCH
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        // A = seccomp_data.nr
        BPF_STMT(BPF_LD | BPF_ABS | BPF_W, offsetof(struct seccomp_data, nr)),
        // if (A == sysno) goto kill
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, kFilteredSyscall, 1, 0),
        // allow: return SECCOMP_RET_ALLOW
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
        // kill: return SECCOMP_RET_KILL
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_KILL),
    };
    sock_fprog prog = {.len = ARRAY_SIZE(filter), .filter = filter};
    EXPECT_EQ(-1, syscall(__NR_seccomp, SECCOMP_SET_MODE_FILTER,
                          SECCOMP_FILTER_FLAG_TSYNC | SECCOMP_FILTER_FLAG_TSYNC_ESRCH, &prog))
        << "expected seccomp fail, but succeeded";
    EXPECT_EQ(ESRCH, errno);

    {
      std::unique_lock l(m);
      ready++;
    }
    cv.notify_all();
    t.join();
  });
}

TEST(SeccompTest, UserNotifMissingListener) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    install_filter_block(kFilteredSyscall, SECCOMP_RET_USER_NOTIF);
    EXPECT_EQ(-1, syscall(kFilteredSyscall));
    EXPECT_EQ(ENOSYS, errno);
  });
}

// Installs a filter that blocks the given syscall.
int install_filter_user_notif(uint32_t syscall_nr) {
  EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

  sock_filter filter[] = {
      // A = seccomp_data.nr
      BPF_STMT(BPF_LD | BPF_ABS | BPF_W, offsetof(struct seccomp_data, nr)),
      // if (A == sysno) goto kill
      BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_K, syscall_nr, 1, 0),
      // allow: return SECCOMP_RET_ALLOW
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
      // notify: return SECCOMP_RET_USER_NOTIF
      BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_USER_NOTIF),
  };
  sock_fprog prog = {.len = ARRAY_SIZE(filter), .filter = filter};
  return static_cast<int>(
      syscall(__NR_seccomp, SECCOMP_SET_MODE_FILTER, SECCOMP_FILTER_FLAG_NEW_LISTENER, &prog));
}

TEST(SeccompTest, UserNotifBlocking) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    const int kFilterReturnValue = 0xbadcafe;
    int fd = install_filter_user_notif(kFilteredSyscall);
    EXPECT_GT(fd, 0);

    {
      int fail = install_filter_user_notif(kFilteredSyscall);
      EXPECT_EQ(fail, -1);
      EXPECT_EQ(errno, EBUSY);
    }

    test_helper::ForkHelper helper;
    helper.OnlyWaitForForkedChildren();
    pid_t pid = helper.RunInForkedProcess(
        [kFilterReturnValue] { EXPECT_EQ(kFilterReturnValue, syscall(kFilteredSyscall)); });

    struct seccomp_notif req;

    // When req has a non-zero field, ioctl returns EINVAL
    req.id = 1;
    EXPECT_EQ(-1, ioctl(fd, SECCOMP_IOCTL_NOTIF_RECV, &req))
        << "ioctl did not return correct error code";

    memset(&req, 0, sizeof(req));
    EXPECT_NE(-1, ioctl(fd, SECCOMP_IOCTL_NOTIF_RECV, &req))
        << "ioctl failed unexpectedly with errno " << errno;
    EXPECT_EQ(pid, static_cast<pid_t>(req.pid));
    EXPECT_EQ(kFilteredSyscall, static_cast<unsigned int>(req.data.nr));

    struct seccomp_notif_resp resp;
    resp.id = req.id;
    resp.val = kFilterReturnValue;
    resp.flags = resp.error = 0;

    // Test that setting both legal flags value + legal val value is an error.
    resp.flags = SECCOMP_USER_NOTIF_FLAG_CONTINUE;
    EXPECT_EQ(-1, ioctl(fd, SECCOMP_IOCTL_NOTIF_SEND, &resp));
    EXPECT_EQ(errno, EINVAL);

    // Test illegal flags value
    resp.flags = ~resp.flags;
    EXPECT_EQ(-1, ioctl(fd, SECCOMP_IOCTL_NOTIF_SEND, &resp));
    EXPECT_EQ(errno, EINVAL);

    // cookie value should still be valid.
    EXPECT_EQ(0, ioctl(fd, SECCOMP_IOCTL_NOTIF_ID_VALID, &resp.id));

    // Okay, let's do this for real.
    resp.flags = 0;
    EXPECT_NE(-1, ioctl(fd, SECCOMP_IOCTL_NOTIF_SEND, &resp))
        << "ioctl failed with errno " << errno;
    EXPECT_TRUE(helper.WaitForChildren());

    // cookie value should no longer be valid
    EXPECT_EQ(-1, ioctl(fd, SECCOMP_IOCTL_NOTIF_ID_VALID, &resp.id));
    EXPECT_EQ(errno, ENOENT);
  });
}

TEST(SeccompTest, UserNotifNonListenerReturnsEnosysEvenWithOtherListener) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    int fd = install_filter_user_notif(kFilteredSyscall);
    EXPECT_GT(fd, 0);

    install_filter_block(__NR_getppid, SECCOMP_RET_USER_NOTIF);

    EXPECT_EQ(-1, syscall(__NR_getppid));
    EXPECT_EQ(ENOSYS, errno);
  });
}

TEST(SeccompTest, UserNotifNonBlocking) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    int fd;
    std::thread t([&fd] {
      fd = install_filter_user_notif(kFilteredSyscall);
      EXPECT_GT(fd, 0);

      test_helper::ForkHelper helper;
      helper.OnlyWaitForForkedChildren();
      pid_t pid = helper.RunInForkedProcess([] {
        // Call 1: resp.error = -enotnam, get -1 result and errno set.
        EXPECT_EQ(-1, syscall(kFilteredSyscall));
        EXPECT_EQ(ENOTNAM, errno);

        errno = 0;
        // Call 2: resp.error = enotnam, get enotnam result and no errno.
        EXPECT_EQ(ENOTNAM, syscall(kFilteredSyscall));
        EXPECT_EQ(0, errno);

        // Call 3: Zeros all around.
        EXPECT_EQ(0, syscall(kFilteredSyscall));
        EXPECT_EQ(0, errno);
      });

      struct seccomp_notif_resp resps[3];
      resps[0] = {.error = -ENOTNAM, .flags = 0};
      resps[1] = {.error = ENOTNAM, .flags = 0};
      resps[2] = {.error = 0, .flags = 0};

      for (int i = 0; i < 3; i++) {
        // Do receive and send in separate threads.  This increases the chance that we'll actually
        // exercise the blocking case for poll() for the sender, which, if it followed the receiver,
        // would probably not block.
        pollfd pfd[] = {{.fd = fd, .events = POLLIN, .revents = 0}};
        EXPECT_NE(-1, poll(pfd, ARRAY_SIZE(pfd), -1)) << "poll failed with errno " << errno;
        EXPECT_EQ(POLLIN, pfd[0].revents);

        struct seccomp_notif req;
        memset(&req, 0, sizeof(req));
        EXPECT_NE(-1, ioctl(fd, SECCOMP_IOCTL_NOTIF_RECV, &req))
            << "ioctl failed with errno " << errno;
        EXPECT_EQ(pid, static_cast<pid_t>(req.pid));
        EXPECT_EQ(kFilteredSyscall, static_cast<unsigned int>(req.data.nr));

        resps[i].id = req.id;

        pfd[0].events = POLLOUT;
        pfd[0].revents = 0;
        EXPECT_NE(-1, poll(pfd, ARRAY_SIZE(pfd), -1)) << "poll failed with errno " << errno;
        EXPECT_EQ(POLLOUT, pfd[0].revents);

        EXPECT_NE(-1, ioctl(fd, SECCOMP_IOCTL_NOTIF_SEND, &resps[i]))
            << "ioctl failed with errno " << errno;
      }

      EXPECT_TRUE(helper.WaitForChildren());
    });
    t.join();

    // Every thread using the filter has terminated, so fd should be POLLHUP.
    pollfd pfd[] = {{.fd = fd, .events = POLLHUP, .revents = 0}};
    EXPECT_NE(-1, poll(pfd, ARRAY_SIZE(pfd), -1)) << "poll failed with errno " << errno;
  });
}

TEST(SeccompTest, UserNotifClose) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    int fd = install_filter_user_notif(kFilteredSyscall);
    EXPECT_GT(fd, 0);
    close(fd);
    EXPECT_EQ(-1, syscall(kFilteredSyscall));
    EXPECT_EQ(ENOSYS, errno);
  });
}

volatile unsigned g_sigtrap_si_arch = 0;

static void sigtrap_handler_check_arch(int signo, siginfo_t *info, void *context) {
  if (signo != SIGSYS || info->si_code != SYS_SECCOMP || info->si_syscall != kFilteredSyscall) {
    return;
  }

  g_sigtrap_si_arch = info->si_arch;
}

TEST(SeccompTest, RetTrapSignalUsesRightArch) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
#if defined(__aarch64__)
    constexpr unsigned kAuditArch = AUDIT_ARCH_AARCH64;
#elif defined(__arm__)
    constexpr unsigned kAuditArch = AUDIT_ARCH_ARM;
#elif defined(__x86_64__)
    constexpr unsigned kAuditArch = AUDIT_ARCH_X86_64;
#elif defined(__riscv)
    constexpr unsigned kAuditArch = AUDIT_ARCH_RISCV64;
#else
#error "Unsupported architecture"
#endif

    struct sigaction sa{};
    sa.sa_sigaction = sigtrap_handler_check_arch;
    sa.sa_flags = SA_SIGINFO;
    SAFE_SYSCALL(sigaction(SIGSYS, &sa, nullptr));

    install_filter_block(kFilteredSyscall, SECCOMP_RET_TRAP);

    syscall(kFilteredSyscall);
    EXPECT_EQ(g_sigtrap_si_arch, kAuditArch);
  });
}

TEST(SeccompTest, BpfIndRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_LDX | BPF_W | BPF_IMM, 0),
        BPF_STMT(BPF_LD | BPF_W | BPF_IND, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfLdHRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_H | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfLdBRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_B | BPF_ABS, offsetof(struct seccomp_data, nr)),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfLdxLen) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        // Init X = 0 so verifier doesn't complain.
        BPF_STMT(BPF_LDX | BPF_IMM, 0),
        // Init A = 64
        BPF_STMT(BPF_LD | BPF_IMM, 64),
        // Linux: X = 64. Fuchsia (bug): A = 64, X unchanged (0).
        BPF_STMT(BPF_LDX | BPF_W | BPF_LEN, 0),
        // If A == X, SECCOMP_RET_ALLOW. Else ERRNO = 123.
        BPF_JUMP(BPF_JMP | BPF_JEQ | BPF_X, 0, 1, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ERRNO | 123),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0), 0);

    // Try a syscall.
    long res = syscall(SYS_getpid);
    // On Linux, A == X (64 == 64), so it's allowed.
    // On Fuchsia, A (64) != X (0), so it returns ERRNO.
    EXPECT_NE(res, -1);
  });
}

TEST(SeccompTest, BpfDivZero) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_ALU | BPF_DIV | BPF_K, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    // Linux returns EINVAL. Fuchsia currently allows it.
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfLshTooLarge) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_ALU | BPF_LSH | BPF_K, 32),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    // Linux returns EINVAL. Fuchsia currently allows it.
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfDivByZeroRuntime) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.ExpectSignal(SIGSYS);

  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        // Load 0 into X
        BPF_STMT(BPF_LDX | BPF_IMM, 0),
        // Divide A by X (0)
        BPF_STMT(BPF_ALU | BPF_DIV | BPF_X, 0),
        // Return ALLOW
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(0, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));

    // Will trigger the filter.
    // In Linux, division by zero terminates the BPF execution and returns 0 (KILL_THREAD).
    // In Fuchsia, division by zero sets A=0 and CONTINUES, reaching RET ALLOW.
    getpid();

    // If we reach here, it means we survived, so Fuchsia behavior is observed.
    // In Linux, the process is killed before this point.
    // We exit with a specific code so we can check it.
    exit(42);
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(SeccompTest, BpfMemUninitialized) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        // Read from uninitialized memory slot 0 into A
        BPF_STMT(BPF_LD | BPF_MEM, 0),
        // Return ALLOW
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    // Linux tracks memory initialization and returns EINVAL.
    // Fuchsia's cbpf_to_ebpf just maps this to a stack read without initialization tracking.
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfSeccompDataOutOfBounds) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    // sizeof(seccomp_data) is 64.
    // Linux checks k >= sizeof(seccomp_data), so 64 is invalid.
    // Starnix checks k > sizeof(seccomp_data), so 64 is valid (bug).
    sock_filter filter[] = {
        BPF_STMT(BPF_LD | BPF_W | BPF_ABS, sizeof(struct seccomp_data)),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };

    // We expect the test to pass on Linux (result -1, errno EINVAL).
    // If Starnix allows it (result 0), this expectation fails.
    int res = prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0);
    EXPECT_EQ(res, -1);
    EXPECT_EQ(errno, EINVAL);
  });
}

TEST(SeccompTest, BpfXStartsInitialized) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.ExpectSignal(SIGSYS);
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        // MOV X to A. X is uninitialized (should be 0).
        // Linux allows this load.
        BPF_STMT(BPF_MISC | BPF_TXA, 0),
        BPF_STMT(BPF_RET | BPF_A, 0),
    };
    sock_fprog prog = {
        .len = (unsigned short)ARRAY_SIZE(filter),
        .filter = filter,
    };

    // Linux allows load.
    EXPECT_EQ(prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0), 0);

    // Runtime:
    // X is 0. A becomes 0. RET 0 -> KILL_THREAD.
    syscall(__NR_getpid);
    exit(0);  // Should not be reached.
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(SeccompTest, BpfModRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_ALU | BPF_MOD | BPF_K, 1),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfUnknownCodeUpperBitsRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(0x0100 | BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfNegWithSourceXRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_ALU | BPF_NEG | BPF_X, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfLastInstNotRetRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
        // Dead code, but it's the last instruction and NOT a RET.
        // Linux explicitly requires the last instruction to be RET.
        BPF_STMT(BPF_ALU | BPF_ADD | BPF_K, 1),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfJaWithSourceXRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        // BPF_JA with BPF_X source bit set
        BPF_STMT(BPF_JMP | BPF_JA | BPF_X, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    // Linux returns EINVAL. Fuchsia ignores the source bit for BPF_JA.
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfStWithSourceXRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        // BPF_ST | BPF_X.
        // Linux explicitly rejects BPF_ST | BPF_X (only BPF_ST is allowed, not BPF_X)
        BPF_STMT(BPF_ST | BPF_X, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };
    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, PR_SET_SECCOMP, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

TEST(SeccompTest, BpfMiscWithSourceXRejected) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    EXPECT_GE(0, prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0));

    sock_filter filter[] = {
        BPF_STMT(BPF_MISC | BPF_TAX | BPF_X, 0),
        BPF_STMT(BPF_RET | BPF_K, SECCOMP_RET_ALLOW),
    };
    sock_fprog prog = {
        .len = ARRAY_SIZE(filter),
        .filter = filter,
    };

    EXPECT_EQ(-1, prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog, 0, 0));
    EXPECT_EQ(EINVAL, errno);
  });
}

}  // anonymous namespace
