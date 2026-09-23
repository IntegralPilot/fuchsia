// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/microfuchsia/tee/lib/dev_urandom_compat/dev_urandom_compat.h"

#include <fcntl.h>
#include <stdio.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

TEST(DevUrandomCompatTest, Open) {
  register_dev_urandom_compat();

  int rv = open("/dev/urandom", O_RDONLY | O_CLOEXEC | O_NOFOLLOW);
  EXPECT_NE(rv, -1);
  close(rv);
}

TEST(DevUrandomCompatTest, Read) {
  register_dev_urandom_compat();

  fbl::unique_fd fd(open("/dev/urandom", O_RDONLY | O_CLOEXEC | O_NOFOLLOW));
  EXPECT_TRUE(fd.is_valid());

  char buf[32];
  EXPECT_EQ(read(fd.get(), buf, sizeof(buf)), static_cast<ssize_t>(sizeof(buf)));

  // Out of 32 random bytes we should see at least one non-zero value.
  bool all_zero = true;
  for (char c : buf) {
    if (c != 0) {
      all_zero = false;
      break;
    }
  }
  EXPECT_FALSE(all_zero);
}
