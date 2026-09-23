// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

#include <thread>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

TEST(AbortTest, Abort) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGABRT);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] { abort(); });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(AbortTest, AbortFromChildThread) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGABRT);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    volatile bool done = false;
    std::thread child_thread([] { abort(); });
    // Spin in userspace forever to verify that we can kick out of restricted
    // mode even when not executing any syscalls.
    while (!done) {
      // This loop issues a volatile load on each iteration which is considered
      // progress by C++ even though it has no practical effect:
      // http://eel.is/c++draft/basic.exec#intro.progress
    }
    ASSERT_TRUE(false) << "Should not be reached";
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

namespace {

void SigQuitHandler(int signum) { abort(); }

}  // namespace

TEST(AbortTest, AbortFromSighandler) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGABRT);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    struct sigaction sa{};
    sa.sa_handler = SigQuitHandler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    SAFE_SYSCALL(sigaction(SIGQUIT, &sa, nullptr));
    raise(SIGQUIT);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

namespace {

void SigUsr2Handler(int signum) { abort(); }

void SigUsr1Handler(int signum) { raise(SIGUSR2); }

}  // namespace

TEST(AbortTest, AbortFromNestedSighandler) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGABRT);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    struct sigaction sa{};
    sa.sa_handler = SigUsr1Handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    SAFE_SYSCALL(sigaction(SIGUSR1, &sa, nullptr));

    sa.sa_handler = SigUsr2Handler;
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    SAFE_SYSCALL(sigaction(SIGUSR2, &sa, nullptr));
    raise(SIGUSR1);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}
