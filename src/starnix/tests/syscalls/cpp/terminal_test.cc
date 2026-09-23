// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <sys/ioctl.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/sysmacros.h>
#include <termios.h>

#include <string>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

int g_received_signal[64] = {};

void RecordSignalHandler(int signo) { g_received_signal[signo]++; }

void IgnoreSignal(int signal) {
  struct sigaction action = {};
  action.sa_handler = SIG_IGN;
  SAFE_SYSCALL(sigaction(signal, &action, nullptr));
}

void RecordSignal(int signal) {
  g_received_signal[signal] = 0;
  struct sigaction action = {};
  action.sa_handler = RecordSignalHandler;
  SAFE_SYSCALL(sigaction(signal, &action, nullptr));
}

long SleepNs(uint64_t count) {
  const uint64_t NS_PER_SECONDS = 1000000000;
  struct timespec ts = {.tv_sec = static_cast<time_t>(count / NS_PER_SECONDS),
                        .tv_nsec = static_cast<long>(count % NS_PER_SECONDS)};
  // TODO(qsr): Use nanosleep when starnix implements clock_nanosleep
  return syscall(SYS_nanosleep, &ts, nullptr);
}

int OpenMainTerminal(int additional_flags = 0) {
  int fd = SAFE_SYSCALL(posix_openpt(O_RDWR | additional_flags));
  SAFE_SYSCALL(grantpt(fd));
  SAFE_SYSCALL(unlockpt(fd));
  return fd;
}

TEST(JobControl, BackgroundProcessGroupDoNotUpdateOnDeath) {
  // Assume starnix always has /dev/ptmx mapped in.
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::IsStarnix() && access("/dev/ptmx", F_OK) == -1) {
    GTEST_SKIP() << "Pseudoterminal not available, skipping...";
  }
  test_helper::ForkHelper helper;

  IgnoreSignal(SIGTTOU);

  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal();
    int replica_terminal = SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR));

    ASSERT_EQ(SAFE_SYSCALL(tcgetpgrp(replica_terminal)), getpid());
    pid_t child_pid = helper.RunInForkedProcess([&] {
      SAFE_SYSCALL(setpgid(0, 0));
      SAFE_SYSCALL(tcsetpgrp(replica_terminal, getpid()));

      ASSERT_EQ(SAFE_SYSCALL(tcgetpgrp(replica_terminal)), getpid());
    });

    // Wait for the child to die.
    ASSERT_TRUE(helper.WaitForChildren());

    // The foreground process group should still be the one from the child.
    ASSERT_EQ(SAFE_SYSCALL(tcgetpgrp(replica_terminal)), child_pid);

    ASSERT_EQ(setpgid(0, child_pid), -1)
        << "Expected not being able to join a process group that has no member anymore";
    ASSERT_EQ(errno, EPERM);
  });
}

TEST(JobControl, OrphanedProcessGroupsReceivesSignal) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Create a new session here, and associate it with the new terminal.
    SAFE_SYSCALL(setsid());

    helper.RunInForkedProcess([&] {
      // Create a new, non leader, process group.
      SAFE_SYSCALL(setpgid(0, 0));
      pid_t pid = helper.RunInForkedProcess([&] {
        // Deepest child. Set a SIGHUP handler, stop ourself, and check that we
        // are restarted and received the expected SIGHUP when our immediate
        // parent dies
        RecordSignal(SIGHUP);
        SAFE_SYSCALL(kill(getpid(), SIGTSTP));
        // At this point, a SIGHUP should have been received.
        // TODO(qsr): Remove the syscall that is there only because starnix
        // currently doesn't handle signal outside of syscalls, and doesn't
        // handle multiple signals at once.
        SAFE_SYSCALL(getpid());
        EXPECT_EQ(g_received_signal[SIGHUP], 1);
      });
      // Wait for the child to have stopped.
      SAFE_SYSCALL(waitid(P_PID, pid, nullptr, WSTOPPED));
    });
    // Wait for the child to die.
    ASSERT_TRUE(helper.WaitForChildren());
  });
}

class Pty : public testing::Test {
  void SetUp() {
    // Assume starnix always has /dev/ptmx mapped in.
    // TODO(https://fxbug.dev/317285180) don't skip on baseline
    if (!test_helper::IsStarnix() && access("/dev/ptmx", F_OK) == -1) {
      GTEST_SKIP() << "Pseudoterminal not available, skipping...";
    }
  }
};

TEST_F(Pty, SIGWINCH) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Create a new session here, and associate it with the new terminal.
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal();
    SAFE_SYSCALL(ioctl(main_terminal, TIOCSCTTY, 0));

    // Register a signal handler for sigusr1.
    RecordSignal(SIGUSR1);
    IgnoreSignal(SIGTTOU);
    IgnoreSignal(SIGHUP);

    // fork a child, move it to its own process group and makes it the
    // foreground one.
    helper.RunInForkedProcess([&] {
      SAFE_SYSCALL(setpgid(0, 0));
      SAFE_SYSCALL(tcsetpgrp(main_terminal, getpid()));

      // Register a signal handler for sigwinch.
      IgnoreSignal(SIGUSR1);
      RecordSignal(SIGWINCH);

      // Send a SIGUSR1 to notify our parent.
      SAFE_SYSCALL(kill(getppid(), SIGUSR1));

      // Wait for a SIGWINCH
      while (g_received_signal[SIGWINCH] == 0) {
        SleepNs(10e7);
      }
    });
    // Wait for SIGUSR1
    while (g_received_signal[SIGUSR1] == 0) {
      SleepNs(10e7);
    }

    // Resize the window, which must generate a SIGWINCH for the children.
    struct winsize ws = {.ws_row = 10, .ws_col = 10};
    SAFE_SYSCALL(ioctl(main_terminal, TIOCSWINSZ, &ws));
  });
}

ssize_t FullRead(int fd, char* buf, size_t count) {
  ssize_t result = 0;
  while (count > 0) {
    ssize_t read_result = read(fd, buf, count);
    if (read_result == -1) {
      if (errno == EAGAIN) {
        break;
      }
      return -1;
    }
    buf += read_result;
    count -= read_result;
    result += read_result;
  }
  return result;
}

TEST_F(Pty, OpenDevTTY) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Create a new session here, and associate it with the new terminal.
    SAFE_SYSCALL(setsid());

    int main_terminal = OpenMainTerminal(O_NONBLOCK);
    SAFE_SYSCALL(ioctl(main_terminal, TIOCSCTTY, 0));

    SAFE_SYSCALL(open("/dev/tty", O_RDWR));
    int other_terminal = SAFE_SYSCALL(open("/dev/tty", O_RDWR));
    struct stat stats;
    SAFE_SYSCALL(fstat(other_terminal, &stats));

    ASSERT_EQ(major(stats.st_rdev), 5u);
    ASSERT_EQ(minor(stats.st_rdev), 0u);

    ASSERT_EQ(write(other_terminal, "h\n", 2), 2);
    char buf[20];
    ASSERT_EQ(FullRead(main_terminal, buf, 20), 3);
    ASSERT_EQ(strncmp(buf, "h\r\n", 3), 0);
  });
}

TEST_F(Pty, RedundantTIOCSCTTY) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Create a new session here, and associate it with the new terminal.
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal();

    // First call to TIOCSCTTY should succeed.
    SAFE_SYSCALL(ioctl(main_terminal, TIOCSCTTY, 0));

    // Second call to TIOCSCTTY on the same terminal should also succeed (noop).
    SAFE_SYSCALL(ioctl(main_terminal, TIOCSCTTY, 0));
  });
}

TEST_F(Pty, StealTIOCSCTTY) {
  test_helper::ForkHelper helper;
  test_helper::Rendezvous acquired = test_helper::MakeRendezvous();
  test_helper::Rendezvous stolen = test_helper::MakeRendezvous();

  int main_terminal = OpenMainTerminal();
  std::string replica_path = ptsname(main_terminal);

  // Session 1 (The victim)
  helper.RunInForkedProcess([&] {
    // Close unused rendezvous sides.
    acquired.holder = {};
    stolen.poker = {};

    SAFE_SYSCALL(setsid());
    int replica_fd = SAFE_SYSCALL(open(replica_path.c_str(), O_RDWR));
    SAFE_SYSCALL(ioctl(replica_fd, TIOCSCTTY, 0));

    // Verify we have it.
    int tty_fd = SAFE_SYSCALL(open("/dev/tty", O_RDWR));
    close(tty_fd);

    // Notify Session 2 that we have acquired it.
    acquired.poker.poke();

    // Wait until Session 2 has stolen it (or tried to).
    stolen.holder.hold();

    if (test_helper::HasSysAdmin()) {
      // Verify we NO LONGER have it.
      // Opening /dev/tty should now fail with ENXIO.
      int ret = open("/dev/tty", O_RDWR);
      EXPECT_EQ(ret, -1);
      EXPECT_EQ(errno, ENXIO);
    } else {
      // Verify we STILL have it.
      int tty_fd = SAFE_SYSCALL(open("/dev/tty", O_RDWR));
      close(tty_fd);
    }
  });

  // Session 2 (The thief)
  helper.RunInForkedProcess([&] {
    // Close unused rendezvous sides.
    acquired.poker = {};
    stolen.holder = {};

    // Wait until Session 1 has acquired it.
    acquired.holder.hold();

    SAFE_SYSCALL(setsid());
    int replica_fd = SAFE_SYSCALL(open(replica_path.c_str(), O_RDWR));

    // Attempt to acquire without steal flag. Should fail with EPERM.
    int ret = ioctl(replica_fd, TIOCSCTTY, 0);
    EXPECT_EQ(ret, -1);
    EXPECT_EQ(errno, EPERM);

    if (test_helper::HasSysAdmin()) {
      // Attempt to steal with steal flag = 1. Should succeed.
      SAFE_SYSCALL(ioctl(replica_fd, TIOCSCTTY, 1));

      // Verify we now have it.
      int tty_fd = SAFE_SYSCALL(open("/dev/tty", O_RDWR));
      close(tty_fd);
    } else {
      // Attempt to steal without CAP_SYS_ADMIN. Should fail with EPERM.
      ret = ioctl(replica_fd, TIOCSCTTY, 1);
      EXPECT_EQ(ret, -1);
      EXPECT_EQ(errno, EPERM);
    }

    // Notify Session 1 that we are done.
    stolen.poker.poke();
  });

  ASSERT_TRUE(helper.WaitForChildren());
  close(main_terminal);
}

TEST_F(Pty, ioctl_TCSETSF) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Create a new session here, and associate it with the new terminal.
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal();

    struct termios config;
    SAFE_SYSCALL(ioctl(main_terminal, TCGETS, &config));
    SAFE_SYSCALL(ioctl(main_terminal, TCSETSF, &config));
  });
}

void FullWrite(int fd, const char* buffer, ssize_t size) {
  ASSERT_EQ(write(fd, buffer, size), size);
}

TEST_F(Pty, EndOfFile) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Create a new session here.
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal();
    int replica_terminal = SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR | O_NONBLOCK));

    char source_buffer[2];
    source_buffer[0] = 4;  // ^D
    source_buffer[1] = '\n';
    char target_buffer[2];

    FullWrite(main_terminal, source_buffer, 1);
    ASSERT_EQ(0, SAFE_SYSCALL(read(replica_terminal, target_buffer, 2)));
    ASSERT_EQ(-1, read(replica_terminal, target_buffer, 2));
    ASSERT_EQ(EAGAIN, errno);

    FullWrite(main_terminal, source_buffer, 2);
    ASSERT_EQ(0, SAFE_SYSCALL(read(replica_terminal, target_buffer, 2)));
    ASSERT_EQ(1, SAFE_SYSCALL(read(replica_terminal, target_buffer, 2)));
    ASSERT_EQ('\n', target_buffer[0]);

    FullWrite(main_terminal, source_buffer, 1);
    FullWrite(main_terminal, source_buffer + 1, 1);
    ASSERT_EQ(0, SAFE_SYSCALL(read(replica_terminal, target_buffer, 2)));
    ASSERT_EQ(1, SAFE_SYSCALL(read(replica_terminal, target_buffer, 2)));
    ASSERT_EQ('\n', target_buffer[0]);

    source_buffer[0] = 4;  // ^D
    source_buffer[1] = 4;  // ^D
    FullWrite(main_terminal, source_buffer, 2);
    ASSERT_EQ(0, SAFE_SYSCALL(read(replica_terminal, target_buffer, 2)));
    ASSERT_EQ(0, SAFE_SYSCALL(read(replica_terminal, target_buffer, 2)));
    ASSERT_EQ(-1, read(replica_terminal, target_buffer, 2));
    ASSERT_EQ(EAGAIN, errno);

    source_buffer[0] = ' ';
    source_buffer[1] = 4;  // ^D
    FullWrite(main_terminal, source_buffer, 2);
    ASSERT_EQ(1, SAFE_SYSCALL(read(replica_terminal, target_buffer, 2)));
    ASSERT_EQ(' ', target_buffer[0]);
  });
}

TEST_F(Pty, EchoModes) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Create a new session here.
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal();
    int replica_terminal = SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR | O_NONBLOCK));

    unsigned default_lflags = ISIG | ICANON | ECHO | ECHOE | ECHOK | ECHOCTL | ECHOKE | IEXTEN;

    struct termios termios = {};
    ASSERT_EQ(0, SAFE_SYSCALL(tcgetattr(main_terminal, &termios)));
    ASSERT_EQ(default_lflags, termios.c_lflag);
    memset(&termios, 0, sizeof(termios));
    ASSERT_EQ(0, SAFE_SYSCALL(tcgetattr(replica_terminal, &termios)));
    ASSERT_EQ(default_lflags, termios.c_lflag);

    auto check_input = [main_terminal, replica_terminal](const char* input, const char* main_str,
                                                         const char* replicate_str) {
      char target_buffer[64] = {};

      FullWrite(main_terminal, input, strlen(input));
      ASSERT_EQ((ssize_t)strlen(main_str),
                SAFE_SYSCALL(read(main_terminal, target_buffer, sizeof(target_buffer) - 1)));
      ASSERT_STREQ(main_str, target_buffer);
      memset(target_buffer, 0, sizeof(target_buffer));
      ASSERT_EQ((ssize_t)strlen(replicate_str),
                SAFE_SYSCALL(read(replica_terminal, target_buffer, sizeof(target_buffer) - 1)));
      ASSERT_STREQ(replicate_str, target_buffer);
    };

    // clang-format off
    check_input("ab\x7F" "cd\n", "ab\b \bcd\r\n", "acd\n");
    check_input("ab\x01" "cd\n", "ab^Acd\r\n", "ab\x01" "cd\n");
    check_input("ab\x06" "cd\n", "ab^Fcd\r\n", "ab\x06" "cd\n");
    check_input("ab\x07" "cd\n", "ab^Gcd\r\n", "ab\x07" "cd\n");
    check_input("ab\x08" "cd\n", "ab^Hcd\r\n", "ab\x08" "cd\n");
    check_input("ab\x09" "cd\n", "ab\tcd\r\n", "ab\tcd\n");
    check_input("ab\x0E" "cd\n", "ab^Ncd\r\n", "ab\x0E" "cd\n");
    check_input("ab\x0F" "cd\n", "ab^Ocd\r\n", "ab\x0F" "cd\n");
    check_input("ab\x15" "cd\n", "ab\b \b\b \bcd\r\n", "cd\n");
    check_input("ab\x1B" "cd\n", "ab^[cd\r\n", "ab\x1B" "cd\n");
    // clang-format on
  });
}
TEST_F(Pty, EchoFlags) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    // Create a new session here.
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal();
    int replica_terminal = SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR | O_NONBLOCK));

    struct termios termios = {};
    ASSERT_EQ(0, SAFE_SYSCALL(tcgetattr(main_terminal, &termios)));

    auto check_input = [main_terminal, replica_terminal](unsigned int lflags, const char* input,
                                                         const char* main_str,
                                                         const char* replicate_str) {
      char target_buffer[64] = {};

      // Set the flags
      struct termios termios = {};
      ASSERT_EQ(0, SAFE_SYSCALL(tcgetattr(main_terminal, &termios)));
      termios.c_lflag = lflags;
      ASSERT_EQ(0, SAFE_SYSCALL(tcsetattr(main_terminal, TCSANOW, &termios)));

      FullWrite(main_terminal, input, strlen(input));
      ASSERT_EQ((ssize_t)strlen(main_str),
                SAFE_SYSCALL(read(main_terminal, target_buffer, sizeof(target_buffer) - 1)));
      ASSERT_STREQ(main_str, target_buffer);
      memset(target_buffer, 0, sizeof(target_buffer));
      ASSERT_EQ((ssize_t)strlen(replicate_str),
                SAFE_SYSCALL(read(replica_terminal, target_buffer, sizeof(target_buffer) - 1)));
      ASSERT_STREQ(replicate_str, target_buffer);
    };

    // Test different combinations of echo flags
    unsigned base_lflags = ISIG | ICANON | ECHO | ECHOCTL | IEXTEN;

    // Just ECHO
    check_input(base_lflags, "abc\n", "abc\r\n", "abc\n");

    // ECHO + ECHOE (erase char)
    check_input(base_lflags | ECHOE,
                "ab\x7F"
                "c\n",
                "ab\b \bc\r\n", "ac\n");

    // ECHO + ECHOK (kill line)
    check_input(base_lflags | ECHOK,
                "ab\x15"
                "c\n",
                "ab^U\r\nc\r\n", "c\n");

    // ECHO + ECHOE + ECHOK (erase char + kill line)
    check_input(base_lflags | ECHOE | ECHOK,
                "ab\x7F\x15"
                "c\n",
                "ab\b \b^U\r\nc\r\n", "c\n");

    // ECHO + ECHOE + ECHOK + ECHOKE (erase char + kill line + kill line erase)
    check_input(base_lflags | ECHOE | ECHOK | ECHOKE,
                "abc\x7F\x15"
                "d\n",
                "abc\b \b\b \b\b \bd\r\n", "d\n");
  });
}

TEST_F(Pty, SendSignals) {
  test_helper::ForkHelper helper;

  std::map<int, char> signal_and_control_character;
  signal_and_control_character[SIGINT] = 3;
  signal_and_control_character[SIGQUIT] = 28;
  signal_and_control_character[SIGSTOP] = 26;

  for (auto [s, c] : signal_and_control_character) {
    auto signal = s;
    auto character = c;

    helper.RunInForkedProcess([&] {
      // Create a new session here, and associate it with the new terminal.
      SAFE_SYSCALL(setsid());
      int main_terminal = OpenMainTerminal();
      SAFE_SYSCALL(ioctl(main_terminal, TIOCSCTTY, 0));

      // Register a signal handler for sigusr1.
      RecordSignal(SIGUSR1);
      IgnoreSignal(SIGTTOU);
      IgnoreSignal(SIGHUP);

      // fork a child, move it to its own process group and makes it the
      // foreground one.
      pid_t child_pid = helper.RunInForkedProcess([&] {
        SAFE_SYSCALL(setpgid(0, 0));
        SAFE_SYSCALL(tcsetpgrp(main_terminal, getpid()));

        // Send a SIGUSR1 to notify our parent.
        SAFE_SYSCALL(kill(getppid(), SIGUSR1));

        // Wait to be killed by our parent.
        for (;;) {
          SleepNs(10e8);
        }
      });
      // Wait for SIGUSR1
      while (g_received_signal[SIGUSR1] == 0) {
        SleepNs(10e7);
      }

      // Send control character.
      char buffer[1];
      buffer[0] = character;
      SAFE_SYSCALL(write(main_terminal, buffer, 1));

      int wstatus;
      pid_t received_pid = SAFE_SYSCALL(waitpid(child_pid, &wstatus, WUNTRACED));
      ASSERT_EQ(received_pid, child_pid);
      if (signal == SIGSTOP) {
        ASSERT_TRUE(WIFSTOPPED(wstatus));
        // Ensure the children is called, even when only stopped.
        SAFE_SYSCALL(kill(child_pid, SIGKILL));
        SAFE_SYSCALL(waitpid(child_pid, nullptr, 0));
      } else {
        ASSERT_TRUE(WIFSIGNALED(wstatus));
        ASSERT_EQ(WTERMSIG(wstatus), signal);
      }
    });
    ASSERT_TRUE(helper.WaitForChildren());
  }
}

TEST_F(Pty, CloseMainTerminal) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    IgnoreSignal(SIGHUP);
    // Create a new session here, and associate it with the new terminal.
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal(O_NONBLOCK | O_NOCTTY);
    int replica_terminal =
        SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR | O_NONBLOCK | O_NOCTTY));
    ASSERT_EQ(open("/dev/tty", O_RDWR), -1);
    ASSERT_EQ(errno, ENXIO);
    close(main_terminal);
    char buffer[1];
    ASSERT_EQ(read(replica_terminal, buffer, 1), 0);
    ASSERT_EQ(write(replica_terminal, buffer, 1), -1);
    EXPECT_EQ(EIO, errno);

    short all_events = POLLIN | POLLPRI | POLLOUT | POLLRDHUP | POLLERR | POLLHUP | POLLNVAL;
    struct pollfd fds = {replica_terminal, all_events, 0};
    ASSERT_EQ(1, SAFE_SYSCALL(poll(&fds, 1, -1)));
    EXPECT_EQ(fds.revents, POLLIN | POLLOUT | POLLERR | POLLHUP);
  });
}

TEST_F(Pty, CloseReplicaTerminal) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Create a new session here, and associate it with the new terminal.
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal(O_NONBLOCK | O_NOCTTY);
    int replica_terminal =
        SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR | O_NONBLOCK | O_NOCTTY));
    ASSERT_EQ(open("/dev/tty", O_RDWR), -1);
    ASSERT_EQ(errno, ENXIO);
    close(replica_terminal);

    char buffer[1];
    ASSERT_EQ(read(main_terminal, buffer, 1), -1);
    EXPECT_EQ(EIO, errno);

    short all_events = POLLIN | POLLPRI | POLLOUT | POLLRDHUP | POLLERR | POLLHUP | POLLNVAL;
    struct pollfd fds = {main_terminal, all_events, 0};
    ASSERT_EQ(1, SAFE_SYSCALL(poll(&fds, 1, -1)));
    ASSERT_EQ(fds.revents, POLLOUT | POLLHUP);

    ASSERT_EQ(write(main_terminal, buffer, 1), 1);
  });
}

TEST_F(Pty, DetectReplicaClosing) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Create a new session here, and associate it with the new terminal.
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal(O_NOCTTY);
    int replica_terminal = SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR | O_NOCTTY));

    struct pollfd fds = {main_terminal, POLLIN, 0};

    RecordSignal(SIGUSR1);
    pid_t child_pid = helper.RunInForkedProcess([&] {
      close(main_terminal);
      RecordSignal(SIGUSR2);
      SAFE_SYSCALL(kill(getppid(), SIGUSR1));
      // Wait for SIGUSR2
      while (g_received_signal[SIGUSR2] == 0) {
        SleepNs(10e7);
      }
    });

    close(replica_terminal);
    // Wait for SIGUSR1
    while (g_received_signal[SIGUSR1] == 0) {
      SleepNs(10e7);
    }
    SAFE_SYSCALL(kill(child_pid, SIGUSR2));
    ASSERT_EQ(1, SAFE_SYSCALL(HANDLE_EINTR(poll(&fds, 1, 10000))));
    ASSERT_EQ(fds.revents, POLLHUP);
  });
}

TEST_F(Pty, NewInstance) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (getuid() != 0) {
    GTEST_SKIP() << "Can only be run as root.";
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    test_helper::ScopedTempDir mount_point1, mount_point2;

    // Mount a default devpts instance.
    auto mount1 = ASSERT_RESULT_SUCCESS_AND_RETURN(
        test_helper::ScopedMount::Mount("devpts", mount_point1.path(), "devpts", 0, nullptr));

    // Mount a new devpts instance.
    auto mount2 = ASSERT_RESULT_SUCCESS_AND_RETURN(
        test_helper::ScopedMount::Mount("devpts", mount_point2.path(), "devpts", 0, "newinstance"));

    struct stat stat_buf;

    // Open ptmx in the first instance, which should create pts/<id>.
    std::string ptmx1_path = mount_point1.path() + "/ptmx";
    fbl::unique_fd ptmx1_fd(open(ptmx1_path.c_str(), O_RDWR));
    ASSERT_TRUE(ptmx1_fd.is_valid());

    int pty1_id = -1;
    ASSERT_EQ(0, ioctl(ptmx1_fd.get(), TIOCGPTN, &pty1_id));

    std::string pts1_id_path = mount_point1.path() + "/" + std::to_string(pty1_id);
    ASSERT_EQ(0, stat(pts1_id_path.c_str(), &stat_buf));

    // The two instances should be separate. Opening a pty in the first instance
    // should not create a new pty in the second one.
    std::string pts2_0_path = mount_point2.path() + "/0";
    ASSERT_EQ(-1, stat(pts2_0_path.c_str(), &stat_buf));
    ASSERT_EQ(ENOENT, errno);
    std::string pts2_id_path = mount_point2.path() + "/" + std::to_string(pty1_id);
    ASSERT_EQ(-1, stat(pts2_id_path.c_str(), &stat_buf));
    ASSERT_EQ(ENOENT, errno);

    // Open ptmx in the second instance, which should now create pts/0.
    std::string ptmx2_path = mount_point2.path() + "/ptmx";
    fbl::unique_fd ptmx2_fd(open(ptmx2_path.c_str(), O_RDWR));
    ASSERT_TRUE(ptmx2_fd.is_valid());

    int pty2_id = -1;
    ASSERT_EQ(0, ioctl(ptmx2_fd.get(), TIOCGPTN, &pty2_id));
    ASSERT_EQ(0, pty2_id);

    ASSERT_EQ(0, stat(pts2_0_path.c_str(), &stat_buf));
  });
}

TEST_F(Pty, SessionLeaderExitSendsSighup) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    test_helper::Rendezvous ready = test_helper::MakeRendezvous();

    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal();
    int replica_terminal = SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR));

    // Ignore SIGTTOU so the child inherits it ignored and doesn't stop on tcsetpgrp.
    IgnoreSignal(SIGTTOU);

    // Block SIGHUP in the session leader so the child inherits the mask.
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGHUP);
    SAFE_SYSCALL(sigprocmask(SIG_BLOCK, &mask, nullptr));

    pid_t child_pid = SAFE_SYSCALL(fork());
    if (child_pid == 0) {
      ready.holder = {};

      SAFE_SYSCALL(setpgid(0, 0));
      SAFE_SYSCALL(tcsetpgrp(replica_terminal, getpid()));

      RecordSignal(SIGHUP);
      ready.poker.poke();

      sigset_t wait_mask;
      sigemptyset(&wait_mask);
      sigsuspend(&wait_mask);
      EXPECT_EQ(g_received_signal[SIGHUP], 1);
      exit(0);
    }

    ready.poker = {};

    ready.holder.hold();
    exit(0);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(Pty, SessionLeaderAndChildExitConcurrently) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    test_helper::Rendezvous ready = test_helper::MakeRendezvous();
    test_helper::Rendezvous finished = test_helper::MakeRendezvous();

    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal();
    int replica_terminal = SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR));

    // Ignore SIGTTOU so the child inherits it ignored and doesn't stop on tcsetpgrp.
    IgnoreSignal(SIGTTOU);

    pid_t child_pid = SAFE_SYSCALL(fork());
    if (child_pid == 0) {
      ready.holder = {};
      finished.poker = {};

      SAFE_SYSCALL(setpgid(0, 0));
      SAFE_SYSCALL(tcsetpgrp(replica_terminal, getpid()));

      IgnoreSignal(SIGHUP);
      ready.poker.poke();

      finished.holder.hold();
      exit(0);
    }

    ready.poker = {};
    finished.holder = {};

    ready.holder.hold();
    finished.poker.poke();
    exit(0);
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(Pty, PacketMode) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    int main_fd = OpenMainTerminal();
    int replica_fd = SAFE_SYSCALL(open(ptsname(main_fd), O_RDWR | O_NOCTTY));

    // ioctl(replica, TIOCPKT) should fail with ENOTTY
    int on = 1;
    ASSERT_EQ(ioctl(replica_fd, TIOCPKT, &on), -1);
    EXPECT_EQ(errno, ENOTTY);

    // ioctl(main, TIOCPKT) should succeed
    ASSERT_EQ(ioctl(main_fd, TIOCPKT, &on), 0);

    // Write to replica, read from main (packet mode)
    const char* msg = "hello";
    ASSERT_EQ(write(replica_fd, msg, 5), 5);

    // Read 1 byte from main. It should be the control byte (0 for data).
    char c = -1;
    ASSERT_EQ(read(main_fd, &c, 1), 1);
    EXPECT_EQ(c, 0);  // TIOCPKT_DATA

    // Read again. Since we didn't consume any data, it should still have "hello", and 0 should be
    // prepended again.
    char buf[10];
    memset(buf, 0, sizeof(buf));
    ASSERT_EQ(read(main_fd, buf, sizeof(buf)), 6);  // 1 control byte + 5 data bytes
    EXPECT_EQ(buf[0], 0);
    EXPECT_STREQ(buf + 1, "hello");

    // Write again, read with enough space
    ASSERT_EQ(write(replica_fd, "world", 5), 5);
    memset(buf, 0, sizeof(buf));
    ASSERT_EQ(read(main_fd, buf, sizeof(buf)), 6);
    EXPECT_EQ(buf[0], 0);
    EXPECT_STREQ(buf + 1, "world");

    // Disable packet mode, write, read
    int off = 0;
    ASSERT_EQ(ioctl(main_fd, TIOCPKT, &off), 0);
    ASSERT_EQ(write(replica_fd, "test", 4), 4);

    memset(buf, 0, sizeof(buf));
    // Without packet mode, we should not get any control bytes.
    ASSERT_EQ(read(main_fd, buf, sizeof(buf)), 4);
    EXPECT_STREQ(buf, "test");

    close(replica_fd);
    close(main_fd);
  });
}

TEST_F(Pty, PacketModeEvents) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Open in non-blocking mode so we can easily check for no-event case.
    int master_fd = OpenMainTerminal(O_NONBLOCK);
    int replica_fd = SAFE_SYSCALL(open(ptsname(master_fd), O_RDWR | O_NOCTTY | O_NONBLOCK));

    int on = 1;
    ASSERT_EQ(ioctl(master_fd, TIOCPKT, &on), 0);

    // Verify no immediate event.
    char c = -1;
    ASSERT_EQ(read(master_fd, &c, 1), -1);
    EXPECT_EQ(errno, EAGAIN);

    // Ensure IXON is enabled initially to force a state change when we disable it.
    struct termios t;
    ASSERT_EQ(tcgetattr(replica_fd, &t), 0);
    t.c_iflag |= IXON;
    ASSERT_EQ(tcsetattr(replica_fd, TCSANOW, &t), 0);

    // Consume any pending events (like TIOCPKT_DOSTOP).
    while (read(master_fd, &c, 1) == 1) {
    }

    // Disable IXON.
    ASSERT_EQ(tcgetattr(replica_fd, &t), 0);
    t.c_iflag &= ~IXON;
    ASSERT_EQ(tcsetattr(replica_fd, TCSANOW, &t), 0);

    struct pollfd fds = {master_fd, POLLPRI | POLLIN, 0};
    ASSERT_EQ(poll(&fds, 1, 1000), 1);
    EXPECT_TRUE(fds.revents & (POLLPRI | POLLIN));

    ASSERT_EQ(read(master_fd, &c, 1), 1);
    EXPECT_EQ(c, TIOCPKT_NOSTOP);

    close(replica_fd);
    close(master_fd);
  });
}

TEST_F(Pty, IoctlTIOCGPTPEER) {
  int main_terminal = OpenMainTerminal();

  // Test basic functionality.
  int replica_terminal = ioctl(main_terminal, TIOCGPTPEER, O_RDWR | O_NOCTTY);
  ASSERT_GE(replica_terminal, 0);

  // Verify we can communicate.
  ASSERT_EQ(write(main_terminal, "hello\n", 6), 6);
  char buf[10];
  ASSERT_EQ(read(replica_terminal, buf, sizeof(buf)), 6);
  EXPECT_EQ(strncmp(buf, "hello\n", 6), 0);

  close(replica_terminal);

  // Test O_CLOEXEC.
  replica_terminal = ioctl(main_terminal, TIOCGPTPEER, O_RDWR | O_CLOEXEC | O_NOCTTY);
  ASSERT_GE(replica_terminal, 0);
  int fd_flags = fcntl(replica_terminal, F_GETFD);
  ASSERT_GE(fd_flags, 0);
  EXPECT_TRUE(fd_flags & FD_CLOEXEC);
  close(replica_terminal);

  // Test O_NONBLOCK.
  replica_terminal = ioctl(main_terminal, TIOCGPTPEER, O_RDWR | O_NONBLOCK | O_NOCTTY);
  ASSERT_GE(replica_terminal, 0);
  int file_flags = fcntl(replica_terminal, F_GETFL);
  ASSERT_GE(file_flags, 0);
  EXPECT_TRUE(file_flags & O_NONBLOCK);

  // Verify it is indeed non-blocking.
  ASSERT_EQ(read(replica_terminal, buf, sizeof(buf)), -1);
  EXPECT_EQ(errno, EAGAIN);

  close(replica_terminal);
  close(main_terminal);
}

// Check that if we flush, we don't drop data that has already been delivered.
TEST_F(Pty, TcoFlushPreservesProcessedInput) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal(O_NONBLOCK);
    int replica_terminal =
        SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR | O_NONBLOCK | O_NOCTTY));

    // Disable canonical mode and echo
    struct termios t = {};
    ASSERT_EQ(0, SAFE_SYSCALL(tcgetattr(replica_terminal, &t)));
    t.c_lflag &= ~(ICANON | ECHO);
    t.c_cc[VMIN] = 1;
    t.c_cc[VTIME] = 0;
    ASSERT_EQ(0, SAFE_SYSCALL(tcsetattr(replica_terminal, TCSANOW, &t)));

    // Write data to replica
    const char* data = "world";
    ASSERT_EQ(5, SAFE_SYSCALL(write(replica_terminal, data, 5)));

    // Wait for the data to be processed and put into the main terminal's read queue.
    int bytes_avail = 0;
    while (bytes_avail < 5) {
      SleepNs(10e6);  // 10 ms
      ASSERT_EQ(0, SAFE_SYSCALL(ioctl(main_terminal, FIONREAD, &bytes_avail)));
    }

    // Call TCOFLUSH on replica
    ASSERT_EQ(0, SAFE_SYSCALL(ioctl(replica_terminal, TCFLSH, TCOFLUSH)));

    // Verify FIONREAD still shows 5 bytes
    ASSERT_EQ(0, SAFE_SYSCALL(ioctl(main_terminal, FIONREAD, &bytes_avail)));
    ASSERT_EQ(5, bytes_avail);

    // Verify reading 5 bytes from main_terminal succeeds and returns the data
    char buf[10] = {};
    ASSERT_EQ(5, SAFE_SYSCALL(read(main_terminal, buf, 5)));
    ASSERT_STREQ("world", buf);

    close(replica_terminal);
    close(main_terminal);
  });
}

TEST_F(Pty, FlushPollEvents) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal(O_NONBLOCK);
    int replica_terminal =
        SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR | O_NONBLOCK | O_NOCTTY));

    // Disable canonical mode and echo
    struct termios t = {};
    ASSERT_EQ(0, SAFE_SYSCALL(tcgetattr(replica_terminal, &t)));
    t.c_lflag &= ~(ICANON | ECHO);
    t.c_cc[VMIN] = 1;
    t.c_cc[VTIME] = 0;
    ASSERT_EQ(0, SAFE_SYSCALL(tcsetattr(replica_terminal, TCSANOW, &t)));

    // Verify initial poll events: both terminals should be writable (POLLOUT)
    // and neither should have POLLIN or POLLHUP.
    struct pollfd pfd_main = {main_terminal, POLLIN | POLLOUT | POLLHUP | POLLERR, 0};
    struct pollfd pfd_replica = {replica_terminal, POLLIN | POLLOUT | POLLHUP | POLLERR, 0};

    ASSERT_EQ(1, SAFE_SYSCALL(poll(&pfd_main, 1, 0)));
    EXPECT_TRUE(pfd_main.revents & POLLOUT);
    EXPECT_FALSE(pfd_main.revents & POLLIN);
    EXPECT_FALSE(pfd_main.revents & POLLHUP);
    EXPECT_FALSE(pfd_main.revents & POLLERR);

    ASSERT_EQ(1, SAFE_SYSCALL(poll(&pfd_replica, 1, 0)));
    EXPECT_TRUE(pfd_replica.revents & POLLOUT);
    EXPECT_FALSE(pfd_replica.revents & POLLIN);
    EXPECT_FALSE(pfd_replica.revents & POLLHUP);
    EXPECT_FALSE(pfd_replica.revents & POLLERR);

    // Replica writes data to main
    const char* data = "hello";
    ASSERT_EQ(5, SAFE_SYSCALL(write(replica_terminal, data, 5)));

    // Wait for the data to be processed and put into main's read queue
    int bytes_avail = 0;
    while (bytes_avail < 5) {
      SleepNs(10e6);  // 10 ms
      ASSERT_EQ(0, SAFE_SYSCALL(ioctl(main_terminal, FIONREAD, &bytes_avail)));
    }

    // Check poll on main: should now be POLLIN | POLLOUT
    ASSERT_EQ(1, SAFE_SYSCALL(poll(&pfd_main, 1, 0)));
    EXPECT_TRUE(pfd_main.revents & POLLIN);
    EXPECT_TRUE(pfd_main.revents & POLLOUT);
    EXPECT_FALSE(pfd_main.revents & POLLHUP);
    EXPECT_FALSE(pfd_main.revents & POLLERR);

    // Now call TCIFLUSH on main to discard the unread bytes
    ASSERT_EQ(0, SAFE_SYSCALL(ioctl(main_terminal, TCFLSH, TCIFLUSH)));

    // Check FIONREAD on main: should be 0
    bytes_avail = 0;
    ASSERT_EQ(0, SAFE_SYSCALL(ioctl(main_terminal, FIONREAD, &bytes_avail)));
    EXPECT_EQ(0, bytes_avail);

    // Check poll on main after TCIFLUSH. We should no longer see POLLIN.
    ASSERT_EQ(1, SAFE_SYSCALL(poll(&pfd_main, 1, 0)));
    EXPECT_FALSE(pfd_main.revents & POLLIN);
    EXPECT_TRUE(pfd_main.revents & POLLOUT);
    EXPECT_FALSE(pfd_main.revents & POLLHUP);
    EXPECT_FALSE(pfd_main.revents & POLLERR);

    // Verify replica poll after TCOFLUSH: still writable (POLLOUT) and no POLLHUP/POLLIN
    ASSERT_EQ(0, SAFE_SYSCALL(ioctl(replica_terminal, TCFLSH, TCOFLUSH)));
    ASSERT_EQ(1, SAFE_SYSCALL(poll(&pfd_replica, 1, 0)));
    EXPECT_TRUE(pfd_replica.revents & POLLOUT);
    EXPECT_FALSE(pfd_replica.revents & POLLIN);
    EXPECT_FALSE(pfd_replica.revents & POLLHUP);
    EXPECT_FALSE(pfd_replica.revents & POLLERR);

    close(replica_terminal);
    close(main_terminal);
  });
}

TEST_F(Pty, FlushOutputPollEvents) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(setsid());
    int main_terminal = OpenMainTerminal(O_NONBLOCK);
    int replica_terminal =
        SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR | O_NONBLOCK | O_NOCTTY));

    // Disable canonical mode and echo
    struct termios t = {};
    ASSERT_EQ(0, SAFE_SYSCALL(tcgetattr(replica_terminal, &t)));
    t.c_lflag &= ~(ICANON | ECHO);
    t.c_cc[VMIN] = 1;
    t.c_cc[VTIME] = 0;
    ASSERT_EQ(0, SAFE_SYSCALL(tcsetattr(replica_terminal, TCSANOW, &t)));

    struct pollfd pfd_replica = {replica_terminal, POLLIN | POLLOUT | POLLHUP | POLLERR, 0};

    // Fill up the replica's output queue by writing in a loop until write returns EAGAIN.
    // First, 1024 bytes at a time, then 1 byte at a time to top it off.
    char buf[1024];
    memset(buf, 'x', sizeof(buf));
    while (true) {
      ssize_t written = write(replica_terminal, buf, sizeof(buf));
      if (written < 0) {
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
          break;
        }
        FAIL() << "Unexpected error during write: " << strerror(errno);
      }
    }
    char c = 'x';
    while (true) {
      ssize_t written = write(replica_terminal, &c, 1);
      if (written < 0) {
        if (errno == EAGAIN || errno == EWOULDBLOCK) {
          break;
        }
        FAIL() << "Unexpected error during 1-byte write: " << strerror(errno);
      }
    }

    // Verify that replica is no longer writable (!POLLOUT)
    ASSERT_EQ(0, SAFE_SYSCALL(poll(&pfd_replica, 1, 0)));
    EXPECT_FALSE(pfd_replica.revents & POLLOUT);
    EXPECT_FALSE(pfd_replica.revents & POLLIN);
    EXPECT_FALSE(pfd_replica.revents & POLLHUP);
    EXPECT_FALSE(pfd_replica.revents & POLLERR);

    // Call TCOFLUSH on replica to discard unprocessed write buffers
    ASSERT_EQ(0, SAFE_SYSCALL(ioctl(replica_terminal, TCFLSH, TCOFLUSH)));

    // Verify that replica is writable again (POLLOUT) after TCOFLUSH and has no
    // POLLIN/POLLHUP/POLLERR!
    ASSERT_EQ(1, SAFE_SYSCALL(poll(&pfd_replica, 1, 0)));
    EXPECT_TRUE(pfd_replica.revents & POLLOUT);
    EXPECT_FALSE(pfd_replica.revents & POLLIN);
    EXPECT_FALSE(pfd_replica.revents & POLLHUP);
    EXPECT_FALSE(pfd_replica.revents & POLLERR);

    close(replica_terminal);
    close(main_terminal);
  });
}

TEST_F(Pty, CloseMainTerminalSendsSighup) {
  test_helper::ForkHelper helper;
  helper.ExpectExitValue(42);
  helper.RunInForkedProcess([&] {
    // Become session leader.
    SAFE_SYSCALL(setsid());

    // Open master PTY.
    int main_terminal = OpenMainTerminal(O_NOCTTY);

    // Open replica PTY and acquire as controlling terminal.
    int replica_terminal = SAFE_SYSCALL(open(ptsname(main_terminal), O_RDWR));

    // Verify it is indeed the controlling terminal.
    int tty_fd = open("/dev/tty", O_RDWR);
    if (tty_fd < 0) {
      _exit(99);  // Failed to acquire controlling terminal.
    }
    close(tty_fd);

    // Set up signal handler for SIGHUP.
    struct sigaction sa = {};
    sa.sa_handler = [](int sig) {
      _exit(42);  // Exit with 42 when SIGHUP is received.
    };
    if (sigaction(SIGHUP, &sa, nullptr) < 0) {
      _exit(98);  // Failed to set signal handler.
    }

    // Close the master PTY. This should trigger SIGHUP.
    close(main_terminal);

    // Wait for signal.
    sleep(2);

    close(replica_terminal);
    // If we reach here, we didn't get SIGHUP.
    _exit(1);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

}  // namespace
