// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <stdint.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <sys/stat.h>
#include <sys/time.h>
#include <sys/types.h>
#include <unistd.h>

#include <gtest/gtest.h>

namespace {

TEST(SetPriorityTest, ZeroRLimit) {
  struct rlimit limit = {
      .rlim_cur = 0,
      .rlim_max = 0,
  };

  ASSERT_EQ(setrlimit(RLIMIT_NICE, &limit), 0)
      << "setrlimit failed" << std::strerror(errno) << '\n';

  ASSERT_EQ(setpriority(PRIO_PROCESS, 0, 19), 0)
      << "setpriority failed" << std::strerror(errno) << '\n';
}

}  //  namespace
