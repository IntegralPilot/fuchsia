// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <gtest/gtest.h>
#include <linux/sched.h>

#include "src/starnix/tests/syscalls/cpp/test_helper.h"

TEST(ThreadGroupLifeCycleTest, ErroneousClone) {
  if (!test_helper::IsStarnix()) {
    // This is testing for a starnix bug. The behavior is different on Linux
    return;
  }
  struct clone_args ca;
  memset(&ca, 0, sizeof(ca));
  ca.flags = CLONE_CHILD_SETTID;
  ASSERT_EQ(-1, syscall(__NR_clone3, &ca, sizeof(struct clone_args)));
  ASSERT_EQ(EFAULT, errno);

  // Fork to ensure starnix iterate over current children.
  pid_t fork_result = fork();
  ASSERT_GE(fork_result, 0);
  if (fork_result == 0) {
    _Exit(0);
  }
}

TEST(ThreadGroupLifeCycleTest, EndMainThreadFirst) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    pthread_t tid;
    pthread_attr_t attr = {};
    pthread_create(
        &tid, &attr,
        [](void* args) -> void* {
          auto tid = gettid();
          auto pid = getpid();
          EXPECT_NE(tid, pid);
          usleep(100000);
          EXPECT_EQ(tid, gettid());
          // Test that the task for the leader is still alive even after the
          // leader task called SYS_exit
          {
            std::string leader_proc =
                "/proc/" + std::to_string(pid) + "/task/" + std::to_string(pid);
            fbl::unique_fd fd(open(leader_proc.c_str(), O_RDONLY));
            EXPECT_TRUE(fd.is_valid());
          }

          // Test that the task for the current thread is still alive even after the
          // leader task called SYS_exit
          {
            std::string leader_proc =
                "/proc/" + std::to_string(pid) + "/task/" + std::to_string(tid);
            fbl::unique_fd fd(open(leader_proc.c_str(), O_RDONLY));
            EXPECT_TRUE(fd.is_valid());
          }

          _exit(testing::Test::HasFailure());
        },
        nullptr);
    syscall(SYS_exit, 1);
  });
}
