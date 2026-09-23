// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>
#include <linux/io_uring.h>

#include "src/starnix/tests/syscalls/cpp/io_uring_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

using io_uring_helper::io_uring_enter;
using io_uring_helper::io_uring_setup;

// TODO(b/444216805): Re-enable once debian 12 fix is implemented.
TEST(IoUringTest, IoUringReadWrite) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "Test fails on debian 12 Linux, skipping.";
  }
  test_helper::ScopedTempFD temp_fd;
  ASSERT_TRUE(temp_fd.fd() >= 0);

  auto io_uring_res = io_uring_helper::IoUring::Create(2);
  ASSERT_THAT(io_uring_res, SyscallResultIsOk());
  auto ring = std::move(io_uring_res.value());
  const auto& params = ring->params();

  // Write to the file
  char write_data[] = "hello";
  struct iovec write_iov = {.iov_base = write_data, .iov_len = sizeof(write_data)};
  ring->SubmitSqe([&temp_fd, &write_iov](io_uring_helper::Sqe* sqe) {
    sqe->opcode = IORING_OP_WRITEV;
    sqe->fd = temp_fd.fd();
    sqe->addr = (uint64_t)&write_iov;
    sqe->len = 1;
    sqe->off = 0;
    sqe->user_data = 1;
  });

  // Read from the file
  char read_data[sizeof(write_data)];
  struct iovec read_iov = {.iov_base = read_data, .iov_len = sizeof(read_data)};
  ring->SubmitSqe([&temp_fd, &read_iov](io_uring_helper::Sqe* sqe) {
    sqe->opcode = IORING_OP_READV;
    sqe->fd = temp_fd.fd();
    sqe->addr = (uint64_t)&read_iov;
    sqe->len = 1;
    sqe->off = 0;
    sqe->user_data = 2;
  });

  // Submit and wait for both operations
  ASSERT_EQ(io_uring_enter(ring->fd(), 2, 2, IORING_ENTER_GETEVENTS, nullptr), 2);

  // Process completions
  uint32_t head = ring->cq_head_ptr()->load(std::memory_order_acquire);
  ASSERT_EQ(ring->cq_tail_ptr()->load(std::memory_order_acquire) - head, 2U);

  bool write_done = false;
  bool read_done = false;
  for (int i = 0; i < 2; i++) {
    io_uring_cqe* cqe = &ring->cqes()[head & (params.cq_entries - 1)];
    if (cqe->user_data == 1) {
      ASSERT_EQ(cqe->res, static_cast<ssize_t>(sizeof(write_data)));
      write_done = true;
    } else if (cqe->user_data == 2) {
      ASSERT_EQ(cqe->res, static_cast<ssize_t>(sizeof(read_data)));
      read_done = true;
    }
    head++;
  }
  ASSERT_TRUE(write_done);
  ASSERT_TRUE(read_done);
  ring->cq_head_ptr()->store(head, std::memory_order_release);

  // Verify
  ASSERT_STREQ(write_data, read_data);
}

TEST(IoUringTest, IoUringSetupCoopTaskrun) {
  if (!test_helper::IsStarnix() && !test_helper::IsKernelVersionAtLeast(5, 19)) {
    GTEST_SKIP() << "Skip test for unsupported feature";
  }
  struct io_uring_params params = {};
  params.flags = IORING_SETUP_COOP_TASKRUN;
  fbl::unique_fd fd(io_uring_setup(1, &params));
  ASSERT_TRUE(fd.is_valid()) << strerror(errno);
}

TEST(IoUringTest, IoUringSetupSingleIssuer) {
  if (!test_helper::IsStarnix() && !test_helper::IsKernelVersionAtLeast(6, 0)) {
    GTEST_SKIP() << "Skip test for unsupported feature";
  }
  struct io_uring_params params = {};
  params.flags = IORING_SETUP_SINGLE_ISSUER;
  fbl::unique_fd fd(io_uring_setup(1, &params));
  ASSERT_TRUE(fd.is_valid()) << strerror(errno);
}

TEST(IoUringTest, IoUringSetupDeferTaskrun) {
  if (!test_helper::IsStarnix() && !test_helper::IsKernelVersionAtLeast(6, 1)) {
    GTEST_SKIP() << "Skip test for unsupported feature";
  }
  struct io_uring_params params = {};
  params.flags = IORING_SETUP_DEFER_TASKRUN;
  fbl::unique_fd fd(io_uring_setup(1, &params));
  ASSERT_FALSE(fd.is_valid());
  ASSERT_EQ(EINVAL, errno);

  params.flags = IORING_SETUP_SINGLE_ISSUER | IORING_SETUP_DEFER_TASKRUN;
  fd = fbl::unique_fd(io_uring_setup(1, &params));
  ASSERT_TRUE(fd.is_valid()) << strerror(errno);
}

}  // namespace
