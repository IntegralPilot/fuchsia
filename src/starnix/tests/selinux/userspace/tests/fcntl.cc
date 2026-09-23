// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/file.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

extern std::string DoPrePolicyLoadWork() { return "fcntl_policy"; }

namespace {

constexpr int kInvalidCmd = INT_MAX;

enum class NeedsPerm {
  kNone,
  kFdUse,
  kFileLock,
};

struct FcntlTestParam {
  int cmd;
  std::string name;
  int arg;
  NeedsPerm needs_perm;
};

fbl::unique_fd CreateTestFileFd() {
  auto attr =
      ScopedTaskAttrResetter::SetTaskAttr("fscreate", "test_u:object_r:test_fcntl_file_t:s0");
  return fbl::unique_fd(open("/tmp", O_RDWR | O_TMPFILE, 0600));
}

class FcntlTest : public testing::TestWithParam<FcntlTestParam> {
 protected:
  fbl::unique_fd CreateTestFdFor(int cmd) {
    if (cmd == F_GETPIPE_SZ) {
      auto attr =
          ScopedTaskAttrResetter::SetTaskAttr("fscreate", "test_u:object_r:test_fcntl_file_t:s0");
      int pipe_fds[2];
      EXPECT_THAT(pipe(pipe_fds), SyscallSucceeds());
      pipe_read_fd_.reset(pipe_fds[0]);
      return fbl::unique_fd(pipe_fds[1]);
    }
    return CreateTestFileFd();
  }

  uintptr_t GetArgForCmd(int cmd, int default_arg, struct flock& fl) {
    if (cmd == F_GETLK || cmd == F_SETLK || cmd == F_SETLKW || cmd == F_OFD_GETLK ||
        cmd == F_OFD_SETLK || cmd == F_OFD_SETLKW) {
      fl.l_type = F_WRLCK;
      fl.l_whence = SEEK_SET;
      fl.l_start = 0;
      fl.l_len = 0;
      fl.l_pid = 0;
      return reinterpret_cast<uintptr_t>(&fl);
    }
    return default_arg;
  }

  testing::Matcher<const int&> SyscallSucceedsIfSupported() {
    auto cmd = GetParam().cmd;
    bool is_supported = true;
    // TODO: https://fxbug.dev/437972675 - Remove this once set/getsig are implemented in Starnix.
    if (test_helper::IsStarnix() && (cmd == F_GETSIG || cmd == F_SETSIG)) {
      is_supported = false;
    } else if (cmd == kInvalidCmd) {
      is_supported = false;
    }

    if (is_supported) {
      return SyscallSucceeds();
    }
    return SyscallFailsWithErrno(EINVAL);
  }

  fbl::unique_fd pipe_read_fd_;
};

TEST_P(FcntlTest, WithFdUse) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  const auto param = GetParam();

  // Create a temporary file within the parent domain.
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_parent_t:s0", [&] {
    auto fd = CreateTestFdFor(param.cmd);
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    // Domain granted FD-use should always have access.
    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_child_t:s0", [&] {
      struct flock fl;
      uintptr_t arg = GetArgForCmd(param.cmd, param.arg, fl);
      EXPECT_THAT(fcntl(fd.get(), param.cmd, arg), SyscallSucceedsIfSupported());
    }));
  }));
}

TEST_P(FcntlTest, WithoutFdUse) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  const auto param = GetParam();

  // Create a temporary file within the parent domain.
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_parent_t:s0", [&] {
    auto fd = CreateTestFdFor(param.cmd);
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    // Domain not granted FD-use.
    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_child_no_use_fd_t:s0", [&] {
      struct flock fl;
      uintptr_t arg = GetArgForCmd(param.cmd, param.arg, fl);
      if (param.needs_perm == NeedsPerm::kNone) {
        EXPECT_THAT(fcntl(fd.get(), param.cmd, arg), SyscallSucceedsIfSupported());
      } else {
        EXPECT_THAT(fcntl(fd.get(), param.cmd, arg), SyscallFailsWithErrno(EACCES));
      }
    }));
  }));
}

TEST_P(FcntlTest, WithoutFileLock) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  const auto param = GetParam();

  // Create a temporary file within the parent domain.
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_parent_t:s0", [&] {
    auto fd = CreateTestFdFor(param.cmd);
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    // Domain not granted FD-use.
    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_child_no_lock_t:s0", [&] {
      struct flock fl;
      uintptr_t arg = GetArgForCmd(param.cmd, param.arg, fl);
      if (param.needs_perm == NeedsPerm::kFileLock) {
        EXPECT_THAT(fcntl(fd.get(), param.cmd, arg), SyscallFailsWithErrno(EACCES));
      } else {
        EXPECT_THAT(fcntl(fd.get(), param.cmd, arg), SyscallSucceedsIfSupported());
      }
    }));
  }));
}

INSTANTIATE_TEST_SUITE_P(
    Fcntl, FcntlTest,
    ::testing::Values(FcntlTestParam{F_GETFD, "F_GETFD", 0, NeedsPerm::kNone},
                      FcntlTestParam{F_SETFD, "F_SETFD", FD_CLOEXEC, NeedsPerm::kNone},
                      FcntlTestParam{F_GETOWN, "F_GETOWN", 0, NeedsPerm::kNone},
                      FcntlTestParam{F_SETOWN, "F_SETOWN", getpid(), NeedsPerm::kFdUse},
                      FcntlTestParam{F_GETLEASE, "F_GETLEASE", 0, NeedsPerm::kNone},
                      FcntlTestParam{F_SETLEASE, "F_SETLEASE", F_WRLCK, NeedsPerm::kFileLock},
                      FcntlTestParam{F_GET_SEALS, "F_GET_SEALS", 0, NeedsPerm::kNone},
                      FcntlTestParam{F_GETFL, "F_GETFL", 0, NeedsPerm::kFdUse},
                      FcntlTestParam{F_SETFL, "F_SETFL", O_NONBLOCK, NeedsPerm::kFdUse},
                      FcntlTestParam{F_GETSIG, "F_GETSIG", 0, NeedsPerm::kFdUse},
                      FcntlTestParam{F_SETSIG, "F_SETSIG", 0, NeedsPerm::kFdUse},
                      FcntlTestParam{F_GETPIPE_SZ, "F_GETPIPE_SZ", 0, NeedsPerm::kNone},
                      FcntlTestParam{F_GETLK, "F_GETLK", 0, NeedsPerm::kFileLock},
                      FcntlTestParam{F_SETLK, "F_SETLK", 0, NeedsPerm::kFileLock},
                      FcntlTestParam{F_SETLKW, "F_SETLKW", 0, NeedsPerm::kFileLock},
                      FcntlTestParam{F_OFD_GETLK, "F_OFD_GETLK", 0, NeedsPerm::kFileLock},
                      FcntlTestParam{F_OFD_SETLK, "F_OFD_SETLK", 0, NeedsPerm::kFileLock},
                      FcntlTestParam{F_OFD_SETLKW, "F_OFD_SETLKW", 0, NeedsPerm::kFileLock},
                      FcntlTestParam{.cmd = kInvalidCmd,
                                     .name = "INVALID",
                                     .arg = 0,
                                     .needs_perm = NeedsPerm::kNone}),
    [](const testing::TestParamInfo<FcntlTestParam>& info) { return info.param.name; });

/// Verifies that set-owner to a non-existent PID, and without SELinux fd-use permission, will fail
/// on the fd-use permission check rather than the PID argument validation.
TEST(FcntlOrderingTest, SetOwnChecksMacBeforeArgs) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_parent_t:s0", [&] {
    auto fd(CreateTestFileFd());
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_child_no_use_fd_t:s0", [&] {
      EXPECT_THAT(fcntl(fd.get(), F_SETOWN, 999999), SyscallFailsWithErrno(EACCES));
    }));
  }));
}

/// Verifies that set-lease with an invalid lock mode will fail on the mode validation, rather than
/// on the fd-use permission check.  A file is created with read/write mode, which therefore fails
/// the request to take a read-lock lease.
TEST(FcntlOrderingTest, SetLeaseChecksMacBeforeArgs) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_parent_t:s0", [&] {
    auto fd(CreateTestFileFd());
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_child_no_use_fd_t:s0", [&] {
      EXPECT_THAT(fcntl(fd.get(), F_SETLEASE, F_RDLCK), SyscallFailsWithErrno(EACCES));
    }));
  }));
}

TEST(FlockSelinuxTest, FlockWithFdUse) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_parent_t:s0", [&] {
    auto fd = CreateTestFileFd();
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_child_t:s0", [&] {
      EXPECT_THAT(flock(fd.get(), LOCK_SH), SyscallSucceeds());
      EXPECT_THAT(flock(fd.get(), LOCK_UN), SyscallSucceeds());
    }));
  }));
}

TEST(FlockSelinuxTest, FlockWithoutFileLock) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_parent_t:s0", [&] {
    auto fd = CreateTestFileFd();
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_fcntl_child_no_lock_t:s0", [&] {
      EXPECT_THAT(flock(fd.get(), LOCK_SH), SyscallFailsWithErrno(EACCES));
    }));
  }));
}

}  // namespace
