// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/mount.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

TEST(Ext4WriteTest, ReadWriteAndTruncate) {
  char* mutable_storage = getenv("TEST_EXT4_ROOT");
  std::string path = std::string(mutable_storage) + "/hello_world.txt";
  std::string expected_original_content;
  ASSERT_TRUE(files::ReadFileToString("data/hello_world.txt", &expected_original_content));

  // Verify initial content
  {
    std::string read_content;
    ASSERT_TRUE(files::ReadFileToString(path, &read_content));
    EXPECT_EQ(read_content, expected_original_content);
  }

  std::string_view overwrite_content = "HELLO, WORLD!\n";
  ASSERT_NE(overwrite_content, expected_original_content);
  {
    fbl::unique_fd fd(open(path.c_str(), O_RDWR));
    ASSERT_TRUE(fd.is_valid()) << "failed to open " << path << ": " << strerror(errno);
    ASSERT_EQ(write(fd.get(), overwrite_content.data(), overwrite_content.size()),
              static_cast<ssize_t>(overwrite_content.size()));
    fd.reset();

    std::string read_content;
    ASSERT_TRUE(files::ReadFileToString(path, &read_content));
    EXPECT_EQ(read_content, overwrite_content);
  }

  // Truncate and overwrite the file
  // Note that this ext4 library only supports overwriting file contents. So the new content that we
  // write should be the same length as the original content.
  std::string_view new_content = "HELLoo WOrld!\n";
  {
    fbl::unique_fd fd(open(path.c_str(), O_RDWR | O_TRUNC));
    ASSERT_TRUE(fd.is_valid()) << "failed to open " << path << ": " << strerror(errno);
    ASSERT_EQ(write(fd.get(), new_content.data(), new_content.size()),
              static_cast<ssize_t>(new_content.size()));
    fd.reset();

    std::string read_content;
    ASSERT_TRUE(files::ReadFileToString(path, &read_content));
    EXPECT_EQ(read_content, new_content);
  }
}

}  // namespace
