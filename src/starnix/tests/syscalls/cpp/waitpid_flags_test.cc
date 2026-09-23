// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/mman.h>
#include <sys/wait.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {
test_helper::SignalMaskHelper sigmaskHelper = test_helper::SignalMaskHelper();
}  // namespace

class WaitpidFlagsTest : public testing::Test {
 protected:
  void SetUp() override {
    sigmaskHelper.blockSignal(SIGUSR1);
    errno = 0;
  }

  void TearDown() override { sigmaskHelper.restoreSigmask(); }
};

TEST_F(WaitpidFlagsTest, waitpidWithNoFlagsFailsWhenChildSendsNoSignal) {
  unsigned int waitFlag = 0;
  int cloneFlags = 0;
  test_helper::waitForChildFails(waitFlag, cloneFlags, test_helper::CloneHelper::sleep_1sec,
                                 test_helper::CloneHelper::doNothing);
  test_helper::waitForChildFails(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                 test_helper::CloneHelper::sleep_1sec);
}

TEST_F(WaitpidFlagsTest, waitpidWithCloneFlagSucceedsWhenChildSendsNoSignal) {
  unsigned int waitFlag = __WCLONE;
  int cloneFlags = 0;
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::sleep_1sec,
                                    test_helper::CloneHelper::doNothing);
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                    test_helper::CloneHelper::sleep_1sec);
}

TEST_F(WaitpidFlagsTest, waitpidWithWallFlagSucceedsWhenChildSendsNoSignal) {
  unsigned int waitFlag = __WALL;
  int cloneFlags = 0;
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::sleep_1sec,
                                    test_helper::CloneHelper::doNothing);
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                    test_helper::CloneHelper::sleep_1sec);
}

TEST_F(WaitpidFlagsTest, waitpidWithNoFlagsFailsWhenChildSendsSIGUSR1) {
  unsigned int waitFlag = 0;
  int cloneFlags = SIGUSR1;
  test_helper::waitForChildFails(waitFlag, cloneFlags, test_helper::CloneHelper::sleep_1sec,
                                 test_helper::CloneHelper::doNothing);
  sigmaskHelper.waitForSignal(SIGUSR1);
  test_helper::waitForChildFails(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                 test_helper::CloneHelper::sleep_1sec);
  sigmaskHelper.waitForSignal(SIGUSR1);
}

TEST_F(WaitpidFlagsTest, waipidWithCloneFlagSucceedsWhenChildSendsSIGUSR1) {
  unsigned int waitFlag = __WCLONE;
  int cloneFlags = SIGUSR1;
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::sleep_1sec,
                                    test_helper::CloneHelper::doNothing);
  sigmaskHelper.waitForSignal(SIGUSR1);
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                    test_helper::CloneHelper::sleep_1sec);
  sigmaskHelper.waitForSignal(SIGUSR1);
}

TEST_F(WaitpidFlagsTest, waipidWithWallFlagSucceedsWhenChildSendsSIGUSR1) {
  unsigned int waitFlag = __WALL;
  int cloneFlags = SIGUSR1;
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::sleep_1sec,
                                    test_helper::CloneHelper::doNothing);
  sigmaskHelper.waitForSignal(SIGUSR1);
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                    test_helper::CloneHelper::sleep_1sec);
  sigmaskHelper.waitForSignal(SIGUSR1);
}

TEST_F(WaitpidFlagsTest, waitpidWithNoFlagsSucceedsWhenChildSendsSIGCHLD) {
  unsigned int waitFlag = 0;
  int cloneFlags = SIGCHLD;
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::sleep_1sec,
                                    test_helper::CloneHelper::doNothing);
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                    test_helper::CloneHelper::sleep_1sec);
}

TEST_F(WaitpidFlagsTest, waitpidWithCloneFlagFailsWhenChildSendsSIGCHLD) {
  unsigned int waitFlag = __WCLONE;
  int cloneFlags = SIGCHLD;
  test_helper::waitForChildFails(waitFlag, cloneFlags, test_helper::CloneHelper::sleep_1sec,
                                 test_helper::CloneHelper::doNothing);
  test_helper::waitForChildFails(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                 test_helper::CloneHelper::sleep_1sec);
}

TEST_F(WaitpidFlagsTest, waitpidWithWallFlagSucceedsWhenChildSendsSIGCHLD) {
  unsigned int waitFlag = 0;
  int cloneFlags = SIGCHLD;
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::sleep_1sec,
                                    test_helper::CloneHelper::doNothing);
  test_helper::waitForChildSucceeds(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                    test_helper::CloneHelper::sleep_1sec);
}

TEST_F(WaitpidFlagsTest, waitpidFailsWaitingWhenChildIsNotThreadGroupLeader) {
  unsigned int waitFlag = __WALL;
  int cloneFlags = SIGCHLD | CLONE_VM | CLONE_THREAD | CLONE_SIGHAND;
  test_helper::waitForChildFails(waitFlag, cloneFlags, test_helper::CloneHelper::doNothing,
                                 test_helper::CloneHelper::sleep_1sec);
}

TEST_F(WaitpidFlagsTest, waitPidStopContinue) {
  // Structure of this test:
  // - One process (the grandchild) is just spinning.
  // - Another process (the child) is waiting for the grandchild to change state.
  // - A third process (the main process) changes the state of the grandchild.
  // The child and the main process communicate their readiness to move to the next
  // check via the shared memory:

  void *mapped =
      mmap(nullptr, sizeof(pid_t), PROT_READ | PROT_WRITE, MAP_SHARED | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(mapped, MAP_FAILED) << "Error = " << errno;
  int *volatile shared_memory = static_cast<int *>(mapped);

  *shared_memory = 0;

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&shared_memory] {
    test_helper::ForkHelper helper;
    pid_t grandchild_pid;
    *shared_memory = helper.RunInForkedProcess([] {
      while (true) {
        sleep(1);
      }
    });

    grandchild_pid = *shared_memory;
    helper.ExpectSignal(SIGKILL);

    int status;
    // Wakes on main process's SIGSTOP
    // Grandchild is now waitable using WUNTRACED.
    ASSERT_EQ(grandchild_pid, waitpid(grandchild_pid, &status, WUNTRACED));

    // The same wait does not happen twice.
    ASSERT_EQ(0, waitpid(grandchild_pid, &status, WNOHANG | WUNTRACED));

    *shared_memory = 1;

    // Wakes on main process's SIGCONT
    // Grandchild is now waitable using WUNTRACED.
    ASSERT_EQ(grandchild_pid, waitpid(grandchild_pid, &status, WCONTINUED | WUNTRACED));

    // The same wait does not happen twice.
    ASSERT_EQ(0, waitpid(grandchild_pid, &status, WNOHANG | WUNTRACED));
    ASSERT_EQ(0, kill(grandchild_pid, SIGKILL));
  });

  pid_t grandchild_pid;
  while ((grandchild_pid = *shared_memory) == 0)
    ;

  ASSERT_EQ(0, kill(grandchild_pid, SIGSTOP));

  while (*shared_memory != 1)
    ;

  ASSERT_EQ(0, kill(grandchild_pid, SIGCONT));
}

TEST_F(WaitpidFlagsTest, WaitpidFailsWithEchildWhenSaNocldwaitIsSet) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = SIG_DFL;
    sa.sa_flags = SA_NOCLDWAIT;
    ASSERT_THAT(sigaction(SIGCHLD, &sa, nullptr), SyscallSucceeds());

    test_helper::ScopedPipe pipe;
    test_helper::ForkHelper helper;
    pid_t pid = helper.RunInForkedProcess([&pipe] {
      pipe.ReadSide().reset();
      // Child exits immediately, closing write-end automatically.
    });
    ASSERT_GE(pid, 0);

    pipe.WriteSide().reset();

    char buf;
    ASSERT_EQ(read(pipe.ReadSide().get(), &buf, 1), 0);

    int status = 0;
    EXPECT_THAT(waitpid(pid, &status, 0), SyscallFailsWithErrno(ECHILD));
  });
}

TEST_F(WaitpidFlagsTest, WaitpidFailsWithEchildWhenSigchldIsIgnored) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    struct sigaction sa;
    memset(&sa, 0, sizeof(sa));
    sa.sa_handler = SIG_IGN;
    ASSERT_THAT(sigaction(SIGCHLD, &sa, nullptr), SyscallSucceeds());

    test_helper::ScopedPipe pipe;
    test_helper::ForkHelper helper;
    pid_t pid = helper.RunInForkedProcess([&pipe] {
      pipe.ReadSide().reset();
      // Child exits immediately, closing write-end automatically.
    });
    ASSERT_GE(pid, 0);

    pipe.WriteSide().reset();

    char buf;
    ASSERT_EQ(read(pipe.ReadSide().get(), &buf, 1), 0);

    int status = 0;
    EXPECT_THAT(waitpid(pid, &status, 0), SyscallFailsWithErrno(ECHILD));
  });
}
