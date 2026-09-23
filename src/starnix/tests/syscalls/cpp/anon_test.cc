// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/eventfd.h>
#include <sys/prctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/securebits.h>

#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"
#include "third_party/android/platform/bionic/libc/kernel/uapi/linux/bpf.h"

#ifndef SYS_userfaultfd
#if defined(__x86_64__)
#define SYS_userfaultfd 323
#elif defined(__aarch64__)
#define SYS_userfaultfd 282
#elif defined(__riscv)
#define SYS_userfaultfd 282
#endif
#endif

#ifndef UFFD_USER_MODE_ONLY
#define UFFD_USER_MODE_ONLY 1
#endif

namespace {

constexpr uid_t kRootUid = 0;
constexpr gid_t kRootGid = 0;
constexpr uid_t kTestUid = 10001;
constexpr gid_t kTestGid = 10002;

struct AnonInodeTestParam {
  std::string name;
  std::function<fbl::unique_fd()> create_fd;
  bool expect_shared;
};

class AnonInodeTest : public ::testing::TestWithParam<AnonInodeTestParam> {};

TEST_P(AnonInodeTest, ModeAndOwnership) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "This test requires CAP_SYS_ADMIN";
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    // Creating BPF nodes requires CAP_BPF so keep capabilities when changing eUID.
    ASSERT_THAT(prctl(PR_SET_SECUREBITS, SECBIT_NO_SETUID_FIXUP), SyscallSucceeds());

    ASSERT_THAT(setegid(kTestGid), SyscallSucceeds());
    ASSERT_THAT(seteuid(kTestUid), SyscallSucceeds());

    const AnonInodeTestParam& param = GetParam();
    fbl::unique_fd fd = param.create_fd();
    ASSERT_THAT(fd.get(), SyscallSucceeds());

    struct stat st;
    ASSERT_THAT(fstat(fd.get(), &st), SyscallSucceeds());

    // All anonymous nodes should be readable and writable only by the owner.
    EXPECT_EQ(st.st_mode, static_cast<mode_t>(0600));

    if (param.expect_shared) {
      // Shared inodes do not inherit ownership, they are owned by root.
      EXPECT_EQ(st.st_uid, kRootUid);
      EXPECT_EQ(st.st_gid, kRootGid);
    } else {
      // Unique inodes inherit ownership.
      EXPECT_EQ(st.st_uid, kTestUid);
      EXPECT_EQ(st.st_gid, kTestGid);
    }
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_P(AnonInodeTest, SingletonNode) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "This test requires CAP_SYS_ADMIN";
  }

  const AnonInodeTestParam& param = GetParam();
  fbl::unique_fd fd1 = param.create_fd();
  ASSERT_THAT(fd1.get(), SyscallSucceeds());

  fbl::unique_fd fd2 = param.create_fd();
  ASSERT_THAT(fd2.get(), SyscallSucceeds());

  struct stat st1, st2;
  ASSERT_THAT(fstat(fd1.get(), &st1), SyscallSucceeds());
  ASSERT_THAT(fstat(fd2.get(), &st2), SyscallSucceeds());

  if (param.expect_shared) {
    EXPECT_EQ(st1.st_ino, st2.st_ino);
  } else {
    EXPECT_NE(st1.st_ino, st2.st_ino);
  }
}

fbl::unique_fd CreateUserfaultfd() {
  int flags = O_CLOEXEC | O_NONBLOCK | UFFD_USER_MODE_ONLY;
  return fbl::unique_fd(static_cast<int>(syscall(SYS_userfaultfd, flags)));
}

fbl::unique_fd CreateEventfd() { return fbl::unique_fd(eventfd(0, 0)); }

fbl::unique_fd CreateBpfMap() {
  union bpf_attr attr;
  memset(&attr, 0, sizeof(attr));
  attr.map_type = BPF_MAP_TYPE_ARRAY;
  attr.key_size = sizeof(int);
  attr.value_size = sizeof(int);
  attr.max_entries = 1;
  return fbl::unique_fd(static_cast<int>(syscall(SYS_bpf, BPF_MAP_CREATE, &attr, sizeof(attr))));
}

INSTANTIATE_TEST_SUITE_P(All, AnonInodeTest,
                         ::testing::Values(
                             AnonInodeTestParam{
                                 .name = "Userfaultfd",
                                 .create_fd = CreateUserfaultfd,
                                 .expect_shared = false,
                             },
                             AnonInodeTestParam{
                                 .name = "Eventfd",
                                 .create_fd = CreateEventfd,
                                 .expect_shared = true,
                             },
                             AnonInodeTestParam{
                                 .name = "BpfMap",
                                 .create_fd = CreateBpfMap,
                                 .expect_shared = true,
                             }),
                         [](const ::testing::TestParamInfo<AnonInodeTestParam>& info) {
                           return info.param.name;
                         });

}  // namespace
