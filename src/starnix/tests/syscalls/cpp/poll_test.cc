// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <netinet/tcp.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>

#include <gtest/gtest.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

TEST(PollTest, REventsIsCleared) {
  int pipefd[2];
  SAFE_SYSCALL(pipe2(pipefd, 0));

  struct pollfd fds[] = {{
                             .fd = pipefd[0],
                             .events = POLLIN,
                             .revents = 42,
                         },
                         {
                             .fd = pipefd[1],
                             .events = POLLOUT,
                             .revents = 42,
                         }};

  ASSERT_EQ(1, poll(fds, 2, 0));
  ASSERT_EQ(0, fds[0].revents);
  ASSERT_EQ(POLLOUT, fds[1].revents);
}

TEST(PollTest, UnconnectedSocket) {
  int fd = socket(PF_INET, SOCK_STREAM, 0);
  ASSERT_GT(fd, 0);

  struct pollfd p;
  p.fd = fd;
  p.events = 0x7FFF;

  EXPECT_EQ(poll(&p, 1, 0), 1);
  EXPECT_EQ(p.revents, POLLHUP | POLLWRNORM | POLLOUT);

  close(fd);
}

}  // namespace
