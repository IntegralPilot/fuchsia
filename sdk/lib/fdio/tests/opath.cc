// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

TEST(Opath, SetFlagsFails) {
  close(open("/tmp/opath-test", O_CREAT, 0666));
  fbl::unique_fd fd(open("/tmp/opath-test", O_PATH));
  ASSERT_TRUE(fd);

  EXPECT_EQ(fcntl(fd.get(), F_SETFL, O_PATH | O_APPEND), -1);
  EXPECT_EQ(errno, EBADF);
}
