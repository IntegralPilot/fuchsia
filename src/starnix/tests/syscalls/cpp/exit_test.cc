// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/syscall.h>
#include <unistd.h>

#include <thread>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

TEST(ExitTest, SysExitSimple) {
  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectExitValue(42);

  fork_helper.RunInForkedProcess([] { syscall(SYS_exit, 42); });
}

TEST(ExitTest, SysExitGroupSimple) {
  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectExitValue(42);

  fork_helper.RunInForkedProcess([] { syscall(SYS_exit_group, 42); });
}

TEST(ExitTest, SysExitMultipleThread) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "The behavior on Linux seems to change from version to version.";
  }

  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectExitValue(42);

  fork_helper.RunInForkedProcess([] {
    for (int i = 0; i < 10; ++i) {
      std::thread t([] { syscall(SYS_exit, 24); });
      t.join();
    }
    std::thread t([] {
      usleep(100'000);
      syscall(SYS_exit, 42);
    });
    syscall(SYS_exit, 24);
  });
}

TEST(ExitTest, SysExitGroupMultipleThread) {
  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectExitValue(42);

  fork_helper.RunInForkedProcess([] {
    for (int i = 0; i < 10; ++i) {
      std::thread t([] { syscall(SYS_exit, 24); });
      t.join();
    }
    std::thread t([] { syscall(SYS_exit_group, 42); });
    syscall(SYS_exit, 24);
  });
}

}  // namespace
