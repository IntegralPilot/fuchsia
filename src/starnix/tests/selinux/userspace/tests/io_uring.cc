// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fit/defer.h>
#include <sys/resource.h>
#include <sys/socket.h>
#include <sys/syscall.h>
#include <sys/utsname.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/io_uring.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/io_uring_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

extern std::string DoPrePolicyLoadWork() { return "io_uring_policy"; }

namespace {

using io_uring_helper::io_uring_enter;
using io_uring_helper::io_uring_register;
using io_uring_helper::io_uring_setup;

TEST(IoUringTest, IoUringSqpollDenied) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:io_uring_test_no_sqpoll_t:s0", [] {
    struct io_uring_params params = {};
    params.flags = IORING_SETUP_SQPOLL;
    fbl::unique_fd ring_fd(io_uring_setup(2, &params));
    EXPECT_FALSE(ring_fd.is_valid());
    EXPECT_EQ(errno, EACCES);
  }));
}

TEST(IoUringTest, IoUringSqpollAllowed) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:io_uring_test_yes_sqpoll_t:s0", [] {
    struct io_uring_params params = {};
    params.flags = IORING_SETUP_SQPOLL;
    // TODO(https://fxbug.dev/515346567): The "io_uring { setup }" permission
    // will be needed once this test starts running on GKI based on Linux 6.18.
    fbl::unique_fd ring_fd(io_uring_setup(2, &params));
    EXPECT_TRUE(ring_fd.is_valid()) << strerror(errno);
  }));
}

struct IoUringTestParam {
  std::string security_context;
  bool allowed;
};

class IoUringCmdTest : public testing::TestWithParam<IoUringTestParam> {};

TEST_P(IoUringCmdTest, Cmd) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  const auto param = GetParam();

  // Sockets file descriptors are one of the few types of file
  // descriptors that supports IORING_OP_URING_CMD.
  auto fd = fbl::unique_fd(socket(AF_INET, SOCK_DGRAM, 0));
  ASSERT_TRUE(fd) << strerror(errno);

  ASSERT_TRUE(RunSubprocessAs(param.security_context, [fd = std::move(fd), param] {
    auto io_uring_res = io_uring_helper::IoUring::Create(2);
    ASSERT_THAT(io_uring_res, SyscallResultIsOk());
    auto ring = std::move(io_uring_res.value());
    const auto& params = ring->params();

    ring->SubmitSqe([fd = fd.get()](io_uring_helper::Sqe* sqe) {
      sqe->opcode = IORING_OP_URING_CMD;
      sqe->fd = fd;
      sqe->user_data = 123;
    });

    ASSERT_EQ(io_uring_enter(ring->fd(), 1, 1, IORING_ENTER_GETEVENTS, nullptr), 1);

    uint32_t head = ring->cq_head_ptr()->load(std::memory_order_acquire);
    ASSERT_EQ(ring->cq_tail_ptr()->load(std::memory_order_acquire) - head, 1U);

    io_uring_cqe* cqe = &ring->cqes()[head & (params.cq_entries - 1)];
    EXPECT_EQ(cqe->user_data, 123U);
    if (param.allowed) {
      EXPECT_EQ(cqe->res, 0);
    } else {
      EXPECT_EQ(cqe->res, -EACCES);
    }
  }));
}

INSTANTIATE_TEST_SUITE_P(IoUringTest, IoUringCmdTest,
                         ::testing::Values(
                             IoUringTestParam{
                                 .security_context = "test_u:test_r:io_uring_test_no_cmd_t:s0",
                                 .allowed = false,
                             },
                             IoUringTestParam{
                                 .security_context = "test_u:test_r:io_uring_test_yes_cmd_t:s0",
                                 .allowed = true,
                             }),
                         [](const testing::TestParamInfo<IoUringTestParam>& info) {
                           return info.param.allowed ? "Allowed" : "Denied";
                         });

class IoUringOverrideCredsTest : public testing::TestWithParam<IoUringTestParam> {};

// Tests that submitting an io_uring request with a registered credential personality
// requires the `override_creds` permission on the `io_uring` class.
//
// Specifically:
// 1. Registers the current thread's credentials (personality) with the io_uring.
// 2. Submits a NOP request using that registered personality.
// 3. Verifies that the request succeeds if the security context has the `override_creds`
//    permission, or fails with EACCES if it does not.
TEST_P(IoUringOverrideCredsTest, OverrideCreds) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  const auto param = GetParam();

  ASSERT_TRUE(RunSubprocessAs(param.security_context, [&] {
    // Create the ring
    auto io_uring_res = io_uring_helper::IoUring::Create(2);
    ASSERT_THAT(io_uring_res, SyscallResultIsOk());
    auto ring = std::move(io_uring_res.value());
    const auto& params = ring->params();

    // Register the current thread's credentials.
    const int personality_id =
        io_uring_register(ring->fd(), IORING_REGISTER_PERSONALITY, nullptr, 0);
    ASSERT_GT(personality_id, 0) << strerror(errno);

    // Submit a NOP request with the registered personality.
    ring->SubmitSqe([personality_id](io_uring_helper::Sqe* sqe) {
      sqe->opcode = IORING_OP_NOP;
      sqe->personality = static_cast<uint16_t>(personality_id);
      sqe->user_data = 123;
    });

    ASSERT_EQ(io_uring_enter(ring->fd(), 1, 1, IORING_ENTER_GETEVENTS, nullptr), 1);

    uint32_t head = ring->cq_head_ptr()->load(std::memory_order_acquire);
    ASSERT_EQ(ring->cq_tail_ptr()->load(std::memory_order_acquire) - head, 1U);

    io_uring_cqe* cqe = &ring->cqes()[head & (params.cq_entries - 1)];
    EXPECT_EQ(cqe->user_data, 123U);
    if (param.allowed) {
      EXPECT_EQ(cqe->res, 0);
    } else {
      EXPECT_EQ(cqe->res, -EACCES);
    }
  }));
}

INSTANTIATE_TEST_SUITE_P(
    IoUringTest, IoUringOverrideCredsTest,
    ::testing::Values(
        IoUringTestParam{
            .security_context = "test_u:test_r:io_uring_test_no_override_t:s0",
            .allowed = false,
        },
        IoUringTestParam{
            .security_context = "test_u:test_r:io_uring_test_yes_override_t:s0",
            .allowed = true,
        }),
    [](const testing::TestParamInfo<IoUringTestParam>& info) {
      return info.param.allowed ? "Allowed" : "Denied";
    });

// Tests that io_uring_setup doesn't check RLIMIT_MEMLOCK when CAP_IPC_LOCK is present.
TEST(IoUringTest, IoUringSetupMemlockLimit) {
  if (!test_helper::IsStarnix() && !test_helper::IsKernelVersionAtLeast(6, 18)) {
    GTEST_SKIP() << "Skip test because io_uring_setup does not check RLIMIT_MEMLOCK";
  }

  struct rlimit old_limit;
  ASSERT_EQ(getrlimit(RLIMIT_MEMLOCK, &old_limit), 0);

  auto cleanup = fit::defer([&old_limit]() { setrlimit(RLIMIT_MEMLOCK, &old_limit); });

  // Set RLIMIT_MEMLOCK limit to 0.
  struct rlimit new_limit = {0, old_limit.rlim_max};
  ASSERT_EQ(setrlimit(RLIMIT_MEMLOCK, &new_limit), 0);

  test_helper::ForkHelper helper;

  // Setup io_uring with CAP_IPC_LOCK.
  helper.RunInForkedProcess([&] {
    struct io_uring_params params = {};
    fbl::unique_fd fd(io_uring_setup(1, &params));
    ASSERT_TRUE(fd.is_valid());
  });

  // Setup io_uring without CAP_IPC_LOCK.
  helper.RunInForkedProcess([&] {
    test_helper::UnsetCapabilityEffective(CAP_IPC_LOCK);

    // io_uring_setup locks memory for the rings. Since we have set RLIMIT_MEMLOCK to 0,
    // and we do not have CAP_IPC_LOCK privilege, this call must fail with ENOMEM.
    struct io_uring_params params = {};
    fbl::unique_fd fd(io_uring_setup(1, &params));
    ASSERT_FALSE(fd.is_valid());
    ASSERT_EQ(errno, ENOMEM);
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

// Tests that the RLIMIT_MEMLOCK limit is cumulative.
TEST(IoUringTest, IoUringSetupMemlockLimitCumulative) {
  if (!test_helper::IsStarnix() && !test_helper::IsKernelVersionAtLeast(6, 18)) {
    GTEST_SKIP() << "Skip test because io_uring_setup does not check RLIMIT_MEMLOCK";
  }

  struct rlimit old_limit;
  ASSERT_EQ(getrlimit(RLIMIT_MEMLOCK, &old_limit), 0);
  auto cleanup = fit::defer([&old_limit]() { setrlimit(RLIMIT_MEMLOCK, &old_limit); });

  // In this test, a call to `io_uring_setup` uses 2 pages of memory.
  // Set RLIMIT_MEMLOCK limit to 3 pages.
  const rlim_t limit_size = 3 * getpagesize();
  struct rlimit new_limit = {limit_size, old_limit.rlim_max};
  ASSERT_EQ(setrlimit(RLIMIT_MEMLOCK, &new_limit), 0);

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    test_helper::UnsetCapabilityEffective(CAP_IPC_LOCK);

    // First setup should succeed.
    struct io_uring_params params1 = {};
    fbl::unique_fd fd1(io_uring_setup(1, &params1));
    ASSERT_TRUE(fd1.is_valid());

    // Second one should fail because cumulative size exceeds limit.
    struct io_uring_params params2 = {};
    fbl::unique_fd fd2(io_uring_setup(1, &params2));
    ASSERT_FALSE(fd2.is_valid());
    ASSERT_EQ(errno, ENOMEM);

    // Close the first one, which should release its accounted memory.
    fd1.reset();

    // Now the third one should succeed.
    // The memory un-accounting is performed asynchronously, so we may need to retry.
    struct io_uring_params params3 = {};
    fbl::unique_fd fd3;
    constexpr int kMaxRetries = 50;
    for (int i = 0; i < kMaxRetries; i++) {
      fd3 = fbl::unique_fd(io_uring_setup(1, &params3));
      if (fd3.is_valid() || errno != ENOMEM) {
        break;
      }
      usleep(10000);  // Wait 10ms and retry
    }
    ASSERT_TRUE(fd3.is_valid());
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

}  // namespace
