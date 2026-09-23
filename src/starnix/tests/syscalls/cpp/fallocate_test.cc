// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <unistd.h>

#include <gtest/gtest.h>
#include <linux/falloc.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

TEST(FallocateTest, Success) {
  auto temp_file = test_helper::ScopedTempFD();
  ASSERT_TRUE(temp_file.is_valid());

  ASSERT_EQ(fallocate(temp_file.fd(), 0, 0, 1024), 0) << strerror(errno);

  struct stat st;
  ASSERT_EQ(fstat(temp_file.fd(), &st), 0);
  EXPECT_EQ(st.st_size, 1024);
}

TEST(FallocateTest, PunchHole) {
  auto temp_file = test_helper::ScopedTempFD();
  ASSERT_TRUE(temp_file.is_valid());

  ASSERT_EQ(fallocate(temp_file.fd(), 0, 0, 4096), 0) << strerror(errno);
  ASSERT_EQ(fallocate(temp_file.fd(), FALLOC_FL_PUNCH_HOLE | FALLOC_FL_KEEP_SIZE, 0, 2048), 0)
      << strerror(errno);

  struct stat st;
  ASSERT_EQ(fstat(temp_file.fd(), &st), 0);
  EXPECT_EQ(st.st_size, 4096);
}

}  // namespace
