// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <dirent.h>
#include <fcntl.h>
#include <lib/fdio/directory.h>
#include <sys/stat.h>

#include <string>
#include <unordered_set>

#include <zxtest/zxtest.h>

namespace {

TEST(FuchsiaIoTest, Stat) {
  struct stat info = {};
  // Calling stat on an optional route to void breaks.
  EXPECT_EQ(-1, stat("/svc/fuchsia.test.Void", &info));
  EXPECT_EQ(ENOENT, errno);

  // Calling stat on a broken route breaks.
  EXPECT_EQ(-1, stat("/svc/fuchsia.test.Broken", &info));
  EXPECT_EQ(ENOENT, errno);

  // Calling stat on a path that doesn't exist breaks.
  EXPECT_EQ(-1, stat("/svc/nonexistant", &info));
  EXPECT_EQ(ENOENT, errno);

  // Calling stat on a working route is successful.
  EXPECT_EQ(0, stat("/svc/fuchsia.logger.LogSink", &info));
}

TEST(FuchsiaIoTest, Readdir) {
  std::unordered_set<std::string> paths = {
      "fuchsia.logger.LogSink",
      "fuchsia.test.Broken",
      "fuchsia.test.Void",
  };

  DIR* dir = opendir("/svc");
  struct dirent* dirent = nullptr;
  while ((dirent = readdir(dir))) {
    paths.erase(dirent->d_name);
  }
  ASSERT_EQ(0, paths.size());

  closedir(dir);
}

}  // namespace
