// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/sendfile.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "fs_test_policy"; }

namespace {
constexpr char kDirLabel[] = "test_u:object_r:test_fs_readdir_dir_t:s0";

TEST(FsTest, ReaddirAllowed) {
  // Create a directory with the specific label.
  auto fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kDirLabel);
  test_helper::ScopedTempDir temp_dir;

  // Open the directory.
  fbl::unique_fd unique_fd(open(temp_dir.path().c_str(), O_RDONLY | O_DIRECTORY));
  ASSERT_THAT(unique_fd.get(), SyscallSucceeds());

  auto enforcing = ScopedEnforcement::SetEnforcing();

  // Run as a domain that is allowed to read the directory.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_fs_t:s0", [&]() {
    char buf[1024];
    EXPECT_THAT(syscall(SYS_getdents64, unique_fd.get(), buf, sizeof(buf)), SyscallSucceeds());
  }));
}

TEST(FsTest, ReaddirDenied) {
  // Create a directory with the specific label.
  auto fscreate = ScopedTaskAttrResetter::SetTaskAttr("fscreate", kDirLabel);
  test_helper::ScopedTempDir temp_dir;

  // Open the directory.
  fbl::unique_fd unique_fd(open(temp_dir.path().c_str(), O_RDONLY | O_DIRECTORY));
  ASSERT_THAT(unique_fd.get(), SyscallSucceeds());

  auto enforcing = ScopedEnforcement::SetEnforcing();

  // Run as a domain that is NOT allowed to read the directory.
  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_fs_no_read_t:s0", [&]() {
    char buf[1024];
    EXPECT_THAT(syscall(SYS_getdents64, unique_fd.get(), buf, sizeof(buf)),
                SyscallFailsWithErrno(EACCES));
  }));
}

constexpr char kFallocateFileLabel[] = "test_u:object_r:test_fs_fallocate_file_t:s0";

// Verify that fallocate succeeds for a domain with write permission.
TEST(FsTest, FallocateAllowed) {
  auto test_file = ScopedTempFDWithLabel(kFallocateFileLabel);
  ASSERT_TRUE(test_file.is_valid());
  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_fs_t:s0", [&]() {
    EXPECT_THAT(fallocate(test_file.fd(), 0, 0, 1024), SyscallSucceeds());
  }));
}

// Verify that fallocate fails for a domain without write permission.
TEST(FsTest, FallocateDenied) {
  auto test_file = ScopedTempFDWithLabel(kFallocateFileLabel);
  ASSERT_TRUE(test_file.is_valid());
  auto enforcing = ScopedEnforcement::SetEnforcing();

  EXPECT_TRUE(RunSubprocessAs("test_u:test_r:test_fs_no_write_t:s0", [&]() {
    EXPECT_THAT(fallocate(test_file.fd(), 0, 0, 1024), SyscallFailsWithErrno(EACCES));
  }));
}

constexpr char kSendfileSrcLabel[] = "test_u:object_r:test_fs_sendfile_src_t:s0";
constexpr char kSendfileDstLabel[] = "test_u:object_r:test_fs_sendfile_dst_t:s0";
constexpr char kPayload[] = "foo";
constexpr size_t kPayloadSize = sizeof(kPayload) - 1;

struct FsSecurityTestParams {
  const char* test_domain;
  bool src_readable;
  bool dst_writable;
};

class FsSecurityTest : public ::testing::TestWithParam<FsSecurityTestParams> {};

// Verify that sendfile enforces SELinux read/write permissions.
TEST_P(FsSecurityTest, Sendfile) {
  const auto& params = GetParam();
  auto src_file = ScopedTempFDWithLabel(kSendfileSrcLabel);
  auto dst_file = ScopedTempFDWithLabel(kSendfileDstLabel);
  ASSERT_TRUE(src_file.is_valid());
  ASSERT_TRUE(dst_file.is_valid());

  // Write some data to the src file.
  ASSERT_THAT(write(src_file.fd(), kPayload, kPayloadSize), SyscallSucceedsWithValue(kPayloadSize));
  ASSERT_THAT(lseek(src_file.fd(), 0, SEEK_SET), SyscallSucceedsWithValue(0));

  auto enforcing = ScopedEnforcement::SetEnforcing();
  std::string context = MakeTestSecurityContext(params.test_domain);

  EXPECT_TRUE(RunSubprocessAs(context, [&]() {
    if (params.src_readable && params.dst_writable) {
      EXPECT_THAT(sendfile(dst_file.fd(), src_file.fd(), nullptr, kPayloadSize), SyscallSucceeds());
    } else {
      EXPECT_THAT(sendfile(dst_file.fd(), src_file.fd(), nullptr, kPayloadSize),
                  SyscallFailsWithErrno(EACCES));
    }
  }));
}

// Verify that splice from a src file to a pipe enforces SELinux read permissions.
TEST_P(FsSecurityTest, SpliceFromSrc) {
  const auto& params = GetParam();
  auto src_file = ScopedTempFDWithLabel(kSendfileSrcLabel);
  ASSERT_TRUE(src_file.is_valid());
  ASSERT_THAT(write(src_file.fd(), kPayload, kPayloadSize), SyscallSucceedsWithValue(kPayloadSize));
  ASSERT_THAT(lseek(src_file.fd(), 0, SEEK_SET), SyscallSucceedsWithValue(0));

  int pipe_fds[2];
  ASSERT_THAT(pipe(pipe_fds), SyscallSucceeds());
  fbl::unique_fd pipe_in(pipe_fds[0]);
  fbl::unique_fd pipe_out(pipe_fds[1]);

  auto enforcing = ScopedEnforcement::SetEnforcing();
  std::string context = MakeTestSecurityContext(params.test_domain);

  EXPECT_TRUE(RunSubprocessAs(context, [&]() {
    if (params.src_readable) {
      EXPECT_THAT(splice(src_file.fd(), nullptr, pipe_out.get(), nullptr, kPayloadSize, 0),
                  SyscallSucceeds());
    } else {
      EXPECT_THAT(splice(src_file.fd(), nullptr, pipe_out.get(), nullptr, kPayloadSize, 0),
                  SyscallFailsWithErrno(EACCES));
    }
  }));
}

// Verify that splice from a pipe to a dst file enforces SELinux write permissions.
TEST_P(FsSecurityTest, SpliceToDst) {
  const auto& params = GetParam();
  auto dst_file = ScopedTempFDWithLabel(kSendfileDstLabel);
  ASSERT_TRUE(dst_file.is_valid());

  int pipe_fds[2];
  ASSERT_THAT(pipe(pipe_fds), SyscallSucceeds());
  fbl::unique_fd pipe_in(pipe_fds[0]);
  fbl::unique_fd pipe_out(pipe_fds[1]);
  ASSERT_THAT(write(pipe_out.get(), kPayload, kPayloadSize),
              SyscallSucceedsWithValue(kPayloadSize));

  auto enforcing = ScopedEnforcement::SetEnforcing();
  std::string context = MakeTestSecurityContext(params.test_domain);

  EXPECT_TRUE(RunSubprocessAs(context, [&]() {
    if (params.dst_writable) {
      EXPECT_THAT(splice(pipe_in.get(), nullptr, dst_file.fd(), nullptr, kPayloadSize, 0),
                  SyscallSucceeds());
    } else {
      EXPECT_THAT(splice(pipe_in.get(), nullptr, dst_file.fd(), nullptr, kPayloadSize, 0),
                  SyscallFailsWithErrno(EACCES));
    }
  }));
}

INSTANTIATE_TEST_SUITE_P(FsSecurity, FsSecurityTest,
                         ::testing::Values(
                             FsSecurityTestParams{
                                 .test_domain = "test_fs_t",
                                 .src_readable = true,
                                 .dst_writable = true,
                             },
                             FsSecurityTestParams{
                                 .test_domain = "test_fs_no_read_t",
                                 .src_readable = false,
                                 .dst_writable = true,
                             },
                             FsSecurityTestParams{
                                 .test_domain = "test_fs_no_write_t",
                                 .src_readable = true,
                                 .dst_writable = false,
                             }),
                         [](const ::testing::TestParamInfo<FsSecurityTest::ParamType>& info) {
                           return info.param.test_domain;
                         });

}  // namespace
