// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/time.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

int main(int argc, char** argv) {
  bool expect_itimer_real_reset = atoi(argv[1]);

  struct itimerval val;
  EXPECT_THAT(getitimer(ITIMER_REAL, &val), SyscallSucceeds());
  if (::testing::Test::HasFailure()) {
    return 1;
  }

  if (expect_itimer_real_reset) {
    EXPECT_EQ(val.it_value.tv_sec, 0);
    EXPECT_EQ(val.it_value.tv_usec, 0);
  } else {
    EXPECT_TRUE(val.it_value.tv_sec != 0 || val.it_value.tv_usec != 0);
  }

  return ::testing::Test::HasFailure() ? 1 : 0;
}
