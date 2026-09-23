// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <optional>
#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "pidfd_policy"; }

namespace {

#ifndef SYS_pidfd_open
#define SYS_pidfd_open 434
#endif

#ifndef SYS_pidfd_getfd
#define SYS_pidfd_getfd 438
#endif

constexpr char kTargetContext[] = "test_u:test_r:test_pidfd_target_t:s0";
constexpr char kFileLabel[] = "test_u:object_r:test_pidfd_file_t:s0";

struct PidFdGetFdTestCase {
  std::string name;
  std::string caller_context;
  int open_flags;
  std::optional<int> expected_errno;
};

class PidFdGetFdTest : public ::testing::TestWithParam<PidFdGetFdTestCase> {};

TEST_P(PidFdGetFdTest, PidFdGetFd) {
  const auto& param = GetParam();

  // 1. Create a temporary file labeled with `kFileLabel` (`test_pidfd_file_t`).
  auto test_file = ScopedTempFDWithLabel(kFileLabel);
  ASSERT_TRUE(test_file.is_valid());

  // 2. Set up synchronization primitives so the parent waits until the target process
  // has opened the file and sent its file descriptor number, and the target process stays
  // alive until the test completes.
  test_helper::ScopedPipe ready_pipe;
  test_helper::Rendezvous exit = test_helper::MakeRendezvous();

  // 3. Spawn the target process in `kTargetContext` (`test_pidfd_target_t`).
  test_helper::ForkHelper helper;
  pid_t target_pid = RunInForkedProcessWithLabel(
      helper, kTargetContext,
      [&, write_fd = std::move(ready_pipe.WriteSide()), holder = std::move(exit.holder)]() mutable {
        // Open the test file in the target domain so the open file description's security
        // context (`fsec->sid`) is set to `test_pidfd_target_t`.
        fbl::unique_fd fd(open(test_file.name().c_str(), param.open_flags));
        ASSERT_THAT(fd.get(), SyscallSucceeds());

        // Send the file descriptor number to the parent process and wait until the test finishes.
        int target_fd = fd.get();
        ASSERT_THAT(write(write_fd.get(), &target_fd, sizeof(target_fd)),
                    SyscallSucceedsWithValue(sizeof(target_fd)));
        write_fd.reset();
        holder.hold();
      });
  ASSERT_GT(target_pid, 0);

  // 4. Wait until the target process has opened the file and read its file descriptor number.
  int target_fd = -1;
  ASSERT_THAT(read(ready_pipe.ReadSide().get(), &target_fd, sizeof(target_fd)),
              SyscallSucceedsWithValue(sizeof(target_fd)));

  // 5. With SELinux enforcing, run the caller subprocess in `param.caller_context` to attempt
  // `pidfd_getfd` against the target process.
  {
    auto enforce = ScopedEnforcement::SetEnforcing();

    EXPECT_TRUE(RunSubprocessAs(param.caller_context, [&] {
      // Obtain a pidfd referring to the target process.
      fbl::unique_fd pidfd(static_cast<int>(syscall(SYS_pidfd_open, target_pid, 0)));
      ASSERT_THAT(pidfd.get(), SyscallSucceeds());

      // Attempt to duplicate `target_fd` from the target process into the caller process.
      fbl::unique_fd received_fd(
          static_cast<int>(syscall(SYS_pidfd_getfd, pidfd.get(), target_fd, 0)));
      if (!param.expected_errno.has_value()) {
        EXPECT_THAT(received_fd.get(), SyscallSucceeds());
      } else {
        EXPECT_THAT(received_fd.get(), SyscallFailsWithErrno(*param.expected_errno));
      }
    }));
  }

  // 6. Signal the target process to exit and reap all children.
  exit.poker.poke();
  EXPECT_TRUE(helper.WaitForChildren());
}

INSTANTIATE_TEST_SUITE_P(
    PidFdTest, PidFdGetFdTest,
    ::testing::Values(
        // When `process { ptrace }`, `fd { use }`, and `file { read write }` permissions are all
        // granted, `pidfd_getfd` succeeds.
        PidFdGetFdTestCase{
            .name = "Allowed",
            .caller_context = "test_u:test_r:test_pidfd_allow_all_t:s0",
            .open_flags = O_RDWR,
            .expected_errno = std::nullopt,
        },
        // When `process { ptrace }` permission is denied to the caller for the target task,
        // `pidfd_getfd` fails with EPERM.
        PidFdGetFdTestCase{
            .name = "DeniedWithoutPtrace",
            .caller_context = "test_u:test_r:test_pidfd_deny_ptrace_t:s0",
            .open_flags = O_RDWR,
            .expected_errno = EPERM,
        },
        // When `process { ptrace }` is allowed but `fd { use }` is denied on the target task's
        // file descriptor, `pidfd_getfd` must fail with EACCES.
        PidFdGetFdTestCase{
            .name = "DeniedWithoutFdUse",
            .caller_context = "test_u:test_r:test_pidfd_deny_fd_use_t:s0",
            .open_flags = O_RDWR,
            .expected_errno = EACCES,
        },
        // When `process { ptrace }` and `fd { use }` are allowed, but `file { read }` is denied on
        // the target file, `pidfd_getfd` must fail with EACCES.
        PidFdGetFdTestCase{
            .name = "DeniedWithoutFileRead",
            .caller_context = "test_u:test_r:test_pidfd_deny_file_read_t:s0",
            .open_flags = O_RDONLY,
            .expected_errno = EACCES,
        },
        // When `process { ptrace }`, `fd { use }`, and `file { write }` are allowed, `pidfd_getfd`
        // on a write-only target file descriptor succeeds even if `file { read }` is denied.
        PidFdGetFdTestCase{
            .name = "AllowedWriteOnlyWithoutFileRead",
            .caller_context = "test_u:test_r:test_pidfd_deny_file_read_t:s0",
            .open_flags = O_WRONLY,
            .expected_errno = std::nullopt,
        },
        // When `process { ptrace }`, `fd { use }`, and `file { read }` are allowed, but
        // `file { write }` is denied on a writable target file descriptor, `pidfd_getfd` must fail
        // with EACCES.
        PidFdGetFdTestCase{
            .name = "DeniedWithoutFileWrite",
            .caller_context = "test_u:test_r:test_pidfd_deny_file_write_t:s0",
            .open_flags = O_RDWR,
            .expected_errno = EACCES,
        },
        // When `process { ptrace }`, `fd { use }`, and `file { read }` are allowed, `pidfd_getfd`
        // on a read-only target file descriptor succeeds even if `file { write }` is denied.
        PidFdGetFdTestCase{
            .name = "AllowedReadOnlyWithoutFileWrite",
            .caller_context = "test_u:test_r:test_pidfd_deny_file_write_t:s0",
            .open_flags = O_RDONLY,
            .expected_errno = std::nullopt,
        },
        // When `process { ptrace }`, `fd { use }`, and `file { read append }` are allowed,
        // `pidfd_getfd` on a target file descriptor opened with `O_APPEND` succeeds even if
        // `file { write }` is denied.
        PidFdGetFdTestCase{
            .name = "AllowedAppendWithoutFileWrite",
            .caller_context = "test_u:test_r:test_pidfd_deny_file_write_t:s0",
            .open_flags = O_RDWR | O_APPEND,
            .expected_errno = std::nullopt,
        },
        // When `process { ptrace }`, `fd { use }`, and `file { read write }` are allowed, but
        // `file { append }` is denied on a target file descriptor opened with `O_APPEND`,
        // `pidfd_getfd` must fail with EACCES.
        PidFdGetFdTestCase{
            .name = "DeniedAppendWithoutFileAppend",
            .caller_context = "test_u:test_r:test_pidfd_deny_file_append_t:s0",
            .open_flags = O_RDWR | O_APPEND,
            .expected_errno = EACCES,
        }),
    [](const ::testing::TestParamInfo<PidFdGetFdTestCase>& info) { return info.param.name; });

}  // namespace
