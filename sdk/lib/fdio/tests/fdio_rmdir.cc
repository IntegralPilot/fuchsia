// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fit/defer.h>
#include <sys/stat.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <zxtest/zxtest.h>

namespace {

TEST(RmdirTest, Rmdir) {
  char filename[] = "/tmp/fdio-rmdir-file.XXXXXX";
  ASSERT_TRUE(fbl::unique_fd(mkstemp(filename)), "%s", strerror(errno));
  auto cleanup =
      fit::defer([&filename]() { EXPECT_EQ(unlink(filename), 0, "%s", strerror(errno)); });
  EXPECT_EQ(rmdir(filename), -1);
  EXPECT_EQ(errno, ENOTDIR);

  char dirname[] = "/tmp/fdio-rmdir-dir.XXXXXX";
  ASSERT_NOT_NULL(mkdtemp(dirname), "%s", strerror(errno));
  EXPECT_EQ(rmdir(dirname), 0);
}

TEST(RmdirTest, RmdirOpenAtDotPassesAfterwards) {
  char dirname[] = "/tmp/fdio-rmdir-dir";

  ASSERT_EQ(0, mkdir(dirname, 0777), "%s", strerror(errno));
  fbl::unique_fd fd1(open(dirname, O_RDONLY | O_DIRECTORY, 0644));
  ASSERT_TRUE(fd1, "%s", strerror(errno));

  EXPECT_EQ(rmdir(dirname), 0);
  // Whilst POSIX documentation suggests that dot and dot-dot entries should be removed after
  // unlinking (see https://pubs.opengroup.org/onlinepubs/009696799/functions/rmdir.html), we want
  // to match Linux which returns success for `openat(fd, ".", flags)`.
  fbl::unique_fd fd2(openat(fd1.get(), ".", O_RDONLY | O_DIRECTORY, 0644));
  ASSERT_TRUE(fd2, "%s", strerror(errno));
}

}  // namespace
