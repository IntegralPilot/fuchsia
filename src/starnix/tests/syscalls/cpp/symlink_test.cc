// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/stat.h>
#include <unistd.h>

#include <string>

#include <gtest/gtest.h>

#include "src/lib/files/file_descriptor.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

TEST(SymlinkTest, StatSize) {
  test_helper::ScopedTempFD temp_file;
  ASSERT_TRUE(temp_file);
  fxl::WriteFileDescriptor(temp_file.fd(), "abc", 3);

  test_helper::ScopedTempSymlink temp_symlink(temp_file.name().c_str());
  ASSERT_TRUE(temp_symlink);

  struct stat stats = {};
  SAFE_SYSCALL(fstatat(AT_FDCWD, temp_symlink.path().c_str(), &stats, AT_SYMLINK_NOFOLLOW));

  EXPECT_EQ(stats.st_mode, static_cast<unsigned>(S_IFLNK | 0777));
  EXPECT_EQ(stats.st_size, static_cast<int>(temp_file.name().length()));
}
