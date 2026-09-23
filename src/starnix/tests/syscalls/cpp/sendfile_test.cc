// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <stdlib.h>
#include <sys/sendfile.h>
#include <sys/socket.h>
#include <unistd.h>

#include <string>

#include <gtest/gtest.h>

namespace {

TEST(SendFileTest, FailsWithPipeInput) {
  int fds[2];
  ASSERT_EQ(pipe(fds), 0);
  ASSERT_EQ(write(fds[1], "foo", 3), 3);
  char* tmp = getenv("TEST_TMPDIR");
  std::string path = tmp == nullptr ? "/tmp/sendfile_test" : std::string(tmp) + "/sendfile_test";
  int fd = open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0777);
  ASSERT_EQ(sendfile(fd, fds[0], nullptr, 3), -1);
  ASSERT_EQ(errno, EINVAL);
  close(fd);
  close(fds[0]);
  close(fds[1]);
}

TEST(SendFileTest, FailsWithSocketInput) {
  int fds[2];
  ASSERT_EQ(socketpair(AF_UNIX, SOCK_STREAM, 0, fds), 0);
  ASSERT_EQ(write(fds[1], "foo", 3), 3);
  char* tmp = getenv("TEST_TMPDIR");
  std::string path = tmp == nullptr ? "/tmp/sendfile_test" : std::string(tmp) + "/sendfile_test";
  int fd = open(path.c_str(), O_WRONLY | O_CREAT | O_TRUNC, 0777);
  ASSERT_EQ(sendfile(fd, fds[0], nullptr, 3), -1);
  ASSERT_EQ(errno, EINVAL);
  close(fd);
  close(fds[0]);
  close(fds[1]);
}

}  // namespace
