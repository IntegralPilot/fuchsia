// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

TEST(CrashTest, Crash) {
  pid_t child = fork();
  if (child == 0) {
    *((volatile char*)0x0) = 0;
  } else {
    int wstatus = 0;
    int options = 0;
    EXPECT_EQ(waitpid(child, &wstatus, options), child);
    EXPECT_TRUE(!WIFEXITED(wstatus)) << wstatus;
    EXPECT_TRUE(WIFSIGNALED(wstatus)) << wstatus;
    EXPECT_EQ(WTERMSIG(wstatus), SIGSEGV);
  }
}

TEST(CrashTest, CrashFromSighandler) {
  test_helper::ForkHelper helper;
  helper.ExpectSignal(SIGSEGV);
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([] {
    struct sigaction sa{};
    sa.sa_handler = [](int signum) { *reinterpret_cast<volatile char*>(0x0) = 0; };
    sigemptyset(&sa.sa_mask);
    sa.sa_flags = 0;
    SAFE_SYSCALL(sigaction(SIGQUIT, &sa, nullptr));
    raise(SIGQUIT);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}
