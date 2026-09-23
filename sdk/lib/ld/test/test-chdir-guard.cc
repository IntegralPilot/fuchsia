// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test-chdir-guard.h"

#include <fcntl.h>
#include <lib/elfldltl/testing/get-test-data.h>
#include <unistd.h>

#include <cerrno>
#include <cstring>

#include <gtest/gtest.h>

namespace ld::testing {

TestChdirGuard::TestChdirGuard(int dir_fd) {
  cwd_.reset(open(".", O_RDONLY | O_DIRECTORY | O_CLOEXEC));
  EXPECT_TRUE(cwd_) << strerror(errno);
  if (cwd_) {
    EXPECT_EQ(fchdir(dir_fd), 0) << "fchdir: " << strerror(errno);
  }
}

TestChdirGuard::~TestChdirGuard() {
  if (cwd_) {
    EXPECT_EQ(fchdir(cwd_.get()), 0) << strerror(errno);
  }
}

}  // namespace ld::testing
