// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <poll.h>
#include <sched.h>
#include <signal.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/prctl.h>
#include <sys/wait.h>
#include <unistd.h>

#include <atomic>
#include <vector>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

std::vector<int> g_received_signals;
std::atomic<int> g_bad_signal_code;

void handler(int signum, siginfo_t *info, void *ucontext) {
  if (info->si_code != CLD_EXITED) {
    g_bad_signal_code.store(info->si_code);
    return;
  }

  g_received_signals.push_back(signum);
}

// This test_helper::CloneHelper instance must only be used after a clone without 'CLONE_THREAD |
// CLONE_VM'.
test_helper::CloneHelper nested_clone_helper;

void ensureWait(int pid, unsigned int waitFlags) {
  int actual_waitpid = waitpid(pid, nullptr, waitFlags);
  EXPECT_EQ(errno, 0);
  EXPECT_EQ(pid, actual_waitpid);
}

class SignalHelper {
 public:
  SignalHelper() {
    g_received_signals.clear();
    g_received_signals.reserve(10);
    g_bad_signal_code.store(0);
    errno = 0;

    struct sigaction sa;
    sa.sa_sigaction = handler;
    sa.sa_flags = SA_SIGINFO | SA_RESTART | SA_NOCLDSTOP;

    sigaction(SIGUSR1, &sa, &old_usr1_act_);
    sigaction(SIGCHLD, &sa, &old_chld_act_);
  }

  ~SignalHelper() {
    sigaction(SIGUSR1, &old_usr1_act_, nullptr);
    sigaction(SIGCHLD, &old_chld_act_, nullptr);
  }

  SignalHelper(const SignalHelper &) = delete;
  SignalHelper &operator=(const SignalHelper &) = delete;

 private:
  struct sigaction old_usr1_act_;
  struct sigaction old_chld_act_;
};

/*
 * Main process (P0) creates a child process (P1).
 * On termination, P1 sends its exit signal (if any) to P0.
 */
TEST(WaitpidExitSignalTest, childProcessSendsDefaultSignalOnTerminationToParentProcess) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([] {
    SignalHelper signal_helper;
    test_helper::CloneHelper test_clone_helper;
    int pid = test_clone_helper.runInClonedChild(SIGCHLD, test_helper::CloneHelper::doNothing);
    ensureWait(pid, __WALL);
    EXPECT_TRUE(g_received_signals.size() == 1);
    EXPECT_EQ(g_received_signals[0], SIGCHLD);
    EXPECT_EQ(0, g_bad_signal_code.load());
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(WaitpidExitSignalTest, childProcessSendsCustomExitSignalOnTerminationToParentProcess) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([] {
    SignalHelper signal_helper;
    test_helper::CloneHelper test_clone_helper;
    int pid = test_clone_helper.runInClonedChild(SIGUSR1, test_helper::CloneHelper::doNothing);
    ensureWait(pid, __WALL);
    EXPECT_TRUE(g_received_signals.size() == 1);
    EXPECT_EQ(g_received_signals[0], SIGUSR1);
    EXPECT_EQ(0, g_bad_signal_code.load());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(WaitpidExitSignalTest, childProcessSendsNoExitSignalOnTerminationToParentProcess) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([] {
    SignalHelper signal_helper;
    test_helper::CloneHelper test_clone_helper;
    int pid = test_clone_helper.runInClonedChild(0, test_helper::CloneHelper::doNothing);
    ensureWait(pid, __WALL);
    EXPECT_TRUE(g_received_signals.empty());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

/*
 * Main process (P0) creates a child process (P1) and P1 creates a child thread (T1).
 * After both P1 and T1 terminate, no matter the order of these termination, P0 should receive P1
 * exit signal.
 */
int processThatFinishAfterChildThread(void *) {
  nested_clone_helper.runInClonedChild(CLONE_THREAD | CLONE_VM | CLONE_SIGHAND | SIGUSR2,
                                       test_helper::CloneHelper::doNothing);
  test_helper::CloneHelper::sleep_1sec(nullptr);
  return 0;
}

int processThatFinishBeforeChildThread(void *) {
  nested_clone_helper.runInClonedChild(CLONE_THREAD | CLONE_VM | CLONE_SIGHAND | SIGUSR2,
                                       test_helper::CloneHelper::sleep_1sec);
  return 0;
}

TEST(WaitpidExitSignalTest, childThreadGroupSendsCorrectExitSignalWhenLeaderTerminatesLast) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([] {
    SignalHelper signal_helper;
    test_helper::CloneHelper test_clone_helper;
    int pid = test_clone_helper.runInClonedChild(SIGUSR1, processThatFinishAfterChildThread);
    ensureWait(pid, __WALL);
    EXPECT_TRUE(g_received_signals.size() == 1);
    EXPECT_EQ(g_received_signals[0], SIGUSR1);
    EXPECT_EQ(0, g_bad_signal_code.load());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(WaitpidExitSignalTest, childThreadGroupSendsCorrectExitSignalWhenLeaderTerminatesFirst) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([] {
    SignalHelper signal_helper;
    test_helper::CloneHelper test_clone_helper;
    int pid = test_clone_helper.runInClonedChild(SIGUSR1, processThatFinishBeforeChildThread);
    ensureWait(pid, __WALL);
    EXPECT_TRUE(g_received_signals.size() == 1);
    EXPECT_EQ(g_received_signals[0], SIGUSR1);
    EXPECT_EQ(0, g_bad_signal_code.load());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(WaitpidExitSignalTest, SubreaperCloneExitSignal) {
  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([] {
    test_helper::CloneHelper helper;
    ASSERT_THAT(prctl(PR_SET_CHILD_SUBREAPER, 1), SyscallSucceeds());

    sigset_t signal_set;
    sigemptyset(&signal_set);
    sigaddset(&signal_set, SIGUSR2);
    sigaddset(&signal_set, SIGCHLD);
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &signal_set, nullptr), SyscallSucceeds());

    test_helper::Rendezvous ready = test_helper::MakeRendezvous();
    test_helper::Rendezvous finished = test_helper::MakeRendezvous();

    helper.runInClonedChild(SIGUSR2, [&]() {
      test_helper::CloneHelper clone_helper;
      pid_t parent_pid = getpid();

      clone_helper.runInClonedChild(SIGUSR2, [&]() {
        finished.poker = {};
        ready.holder = {};

        EXPECT_EQ(getppid(), parent_pid);

        ready.poker.poke();
        finished.holder.hold();

        // Wait for reparenting to complete.
        while (getppid() == parent_pid) {
          sched_yield();
        }
      });

      finished.holder = {};
      ready.poker = {};

      ready.holder.hold();
      finished.poker.poke();
    });

    // Wait for the parent to exit, which should exit with SIGUSR2.
    int received_signal;
    sigemptyset(&signal_set);
    sigaddset(&signal_set, SIGUSR2);
    std::cout << "Waiting for SIGUSR2" << std::endl;
    EXPECT_EQ(sigwait(&signal_set, &received_signal), 0);
    EXPECT_EQ(received_signal, SIGUSR2);

    // Wait for the child to exit, which should exit with SIGCHLD since
    // it has been reparented.
    sigemptyset(&signal_set);
    sigaddset(&signal_set, SIGCHLD);
    std::cout << "Waiting for SIGCHLD" << std::endl;
    EXPECT_EQ(sigwait(&signal_set, &received_signal), 0);
    EXPECT_EQ(received_signal, SIGCHLD);
  });
  EXPECT_TRUE(fork_helper.WaitForChildren());
}

// Verifies that a stopped child process in an orphaned process group receives SIGHUP and SIGCONT
// when its parent exits.
//
// 1. Child moves to a new process group.
// 2. Child forks Grandchild, which moves to a new process group.
// 3. Grandchild stops itself (SIGSTOP).
// 4. Child exits, making Grandchild's process group orphaned because its parent Child is dead, and
//    the reparented parent is in a different session, which doesn't prevent orphaning.
// 5. Grandchild is reparented to the outer forked process (subreaper).
// 6. Subreaper waits for Grandchild and verifies it died from SIGHUP.
TEST(WaitpidExitSignalTest, StoppedChildInOrphanedGroup) {
  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([] {
    // This is the grandparent process.
    // Mark ourselves as a subreaper so we inherit orphaned grandchildren.
    ASSERT_THAT(prctl(PR_SET_CHILD_SUBREAPER, 1), SyscallSucceeds());

    test_helper::ForkHelper fork_helper;
    fork_helper.OnlyWaitForForkedChildren();
    test_helper::ScopedPipe pipe;
    fork_helper.RunInForkedProcess([&] {
      // This is the child process.
      pipe.ReadSide().reset();
      ASSERT_THAT(setsid(), SyscallSucceeds());

      pid_t grandchild_pid = fork();
      ASSERT_THAT(grandchild_pid, SyscallSucceeds());
      if (grandchild_pid == 0) {
        // This is the grandchild process.
        ASSERT_THAT(setpgid(0, 0), SyscallSucceeds());
        raise(SIGSTOP);
        _exit(0);
      }

      ASSERT_EQ(write(pipe.WriteSide().get(), &grandchild_pid, sizeof(grandchild_pid)),
                static_cast<ssize_t>(sizeof(grandchild_pid)));
      pipe.WriteSide().reset();

      int status;
      ASSERT_THAT(waitpid(grandchild_pid, &status, WUNTRACED),
                  SyscallSucceedsWithValue(grandchild_pid));
      ASSERT_TRUE(WIFSTOPPED(status));

      // Grandchild is stopped. Child exit causes grandchild to become orphaned.
      _exit(0);
    });

    pid_t grandchild_pid = -1;
    ASSERT_EQ(read(pipe.ReadSide().get(), &grandchild_pid, sizeof(grandchild_pid)),
              static_cast<ssize_t>(sizeof(grandchild_pid)));
    pipe.ReadSide().reset();
    pipe.WriteSide().reset();

    EXPECT_TRUE(fork_helper.WaitForChildren());

    // Grandchild is now reparented to us. Since its process group is orphaned, and it was stopped,
    // it should have received SIGHUP and SIGCONT, and exited.
    int status;
    ASSERT_THAT(waitpid(grandchild_pid, &status, 0), SyscallSucceedsWithValue(grandchild_pid));
    ASSERT_TRUE(WIFSIGNALED(status));
    EXPECT_EQ(WTERMSIG(status), SIGHUP);
  });
  EXPECT_TRUE(fork_helper.WaitForChildren());
}

}  // namespace
