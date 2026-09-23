// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <poll.h>

#include <string>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

class PressureTestFixture : public testing::TestWithParam<const char*> {
 protected:
  fbl::unique_fd fd;

  virtual void SetUp() {
    if (test_helper::IsStarnix()) {
      GTEST_SKIP() << "https://fxbug.dev/458614225 PsiProvider is not available for syscall tests";
      return;
    }
    if (access(GetParam(), F_OK) != 0) {
      GTEST_SKIP() << "https://fxbug.dev/458631470 Pressure file doesn't exist";
    }
    fd = fbl::unique_fd(open(GetParam(), O_RDWR));
    if (!fd.is_valid()) {
      GTEST_SKIP() << "https://fxbug.dev/458631470 Pressure file cannot be opened";
    }
  }
};

TEST_P(PressureTestFixture, PressureFileRespondToPollIn) {
  struct pollfd fds{
      .fd = fd.get(),
      .events = POLL_IN,
      .revents = 0,
  };
  ASSERT_EQ(SAFE_SYSCALL(poll(&fds, 1, 100)), 1);
  ASSERT_EQ(fds.revents & POLL_IN, POLL_IN);
}

TEST_P(PressureTestFixture, PressureFileRespondToPollOut) {
  struct pollfd fds{
      .fd = fd.get(),
      .events = POLL_OUT,
      .revents = 0,
  };
  ASSERT_EQ(SAFE_SYSCALL(poll(&fds, 1, 100)), 1);
  ASSERT_EQ(fds.revents & POLL_OUT, POLL_OUT);
}

INSTANTIATE_TEST_SUITE_P(PressureTest, PressureTestFixture,
                         ::testing::Values("/proc/pressure/cpu", "/proc/pressure/io",
                                           "/proc/pressure/memory"));
