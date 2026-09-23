// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <errno.h>
#include <sys/inotify.h>
#include <sys/stat.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

TEST(InotifyTest, DeleteWatchedDirWithoutDeleteSelfMaskEmitsIgnored) {
  test_helper::ScopedTempDir temp_dir;
  std::string watched_dir = temp_dir.path() + "/watched";
  ASSERT_EQ(mkdir(watched_dir.c_str(), 0700), 0) << strerror(errno);

  fbl::unique_fd fd(inotify_init1(IN_NONBLOCK | IN_CLOEXEC));
  ASSERT_TRUE(fd.is_valid()) << strerror(errno);

  int wd = inotify_add_watch(fd.get(), watched_dir.c_str(), IN_CREATE | IN_DELETE);
  ASSERT_GE(wd, 0) << strerror(errno);

  ASSERT_EQ(rmdir(watched_dir.c_str()), 0) << strerror(errno);

  struct inotify_event event = {};
  ssize_t bytes_read = read(fd.get(), &event, sizeof(event));
  ASSERT_EQ(bytes_read, static_cast<ssize_t>(sizeof(event))) << strerror(errno);
  EXPECT_EQ(event.wd, wd);
  EXPECT_EQ(event.mask, static_cast<uint32_t>(IN_IGNORED));

  // No further events should be queued, and the watch descriptor should already be removed.
  EXPECT_EQ(read(fd.get(), &event, sizeof(event)), -1);
  EXPECT_EQ(errno, EAGAIN);
  EXPECT_EQ(inotify_rm_watch(fd.get(), wd), -1);
  EXPECT_EQ(errno, EINVAL);
}

TEST(InotifyTest, DeleteWatchedDirWithDeleteSelfMaskEmitsDeleteSelfAndIgnored) {
  test_helper::ScopedTempDir temp_dir;
  std::string watched_dir = temp_dir.path() + "/watched";
  ASSERT_EQ(mkdir(watched_dir.c_str(), 0700), 0) << strerror(errno);

  fbl::unique_fd fd(inotify_init1(IN_NONBLOCK | IN_CLOEXEC));
  ASSERT_TRUE(fd.is_valid()) << strerror(errno);

  int wd = inotify_add_watch(fd.get(), watched_dir.c_str(), IN_CREATE | IN_DELETE_SELF);
  ASSERT_GE(wd, 0) << strerror(errno);

  ASSERT_EQ(rmdir(watched_dir.c_str()), 0) << strerror(errno);

  struct inotify_event events[2] = {};
  ssize_t bytes_read = read(fd.get(), events, sizeof(events));
  ASSERT_EQ(bytes_read, static_cast<ssize_t>(sizeof(events))) << strerror(errno);
  EXPECT_EQ(events[0].wd, wd);
  EXPECT_EQ(events[0].mask, static_cast<uint32_t>(IN_DELETE_SELF));
  EXPECT_EQ(events[1].wd, wd);
  EXPECT_EQ(events[1].mask, static_cast<uint32_t>(IN_IGNORED));

  EXPECT_EQ(inotify_rm_watch(fd.get(), wd), -1);
  EXPECT_EQ(errno, EINVAL);
}

}  // namespace
