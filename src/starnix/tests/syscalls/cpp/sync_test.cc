// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

TEST(SyncTest, Sync) {
  test_helper::ScopedTempFD f;
  ASSERT_TRUE(f.is_valid());
  int fd = f.fd();

  const char* data = "sync_test";
  ASSERT_EQ(write(fd, data, 9), 9);

  // Just ensure sync() doesn't crash or error (it returns void).
  sync();

  char buf[10] = {};
  ASSERT_EQ(lseek(fd, 0, SEEK_SET), 0);
  ASSERT_EQ(read(fd, buf, 9), 9);
  EXPECT_STREQ(buf, data);
}

TEST(SyncTest, SyncFs) {
  test_helper::ScopedTempFD f;
  ASSERT_TRUE(f.is_valid());
  int fd = f.fd();

  // Write some data so there's something to sync, though syncfs() works regardless.
  const char* data = "hello";
  ssize_t written = write(fd, data, 5);
  ASSERT_EQ(written, 5);

  SAFE_SYSCALL(syncfs(fd));

  // Reading back with a buffer check doesn't verify the actual write-back to the media
  // due to guaranteed cache coherency. Nonetheless, the check won't hurt to have.
  char buf[6] = {};
  ASSERT_EQ(lseek(fd, 0, SEEK_SET), 0);
  ASSERT_EQ(read(fd, buf, 5), 5);
  EXPECT_STREQ(buf, data);
}

TEST(SyncTest, SyncFsInvalidFd) {
  int ret = syncfs(-1);
  int err = errno;
  EXPECT_EQ(ret, -1);
  EXPECT_EQ(err, EBADF);
}

}  // namespace
