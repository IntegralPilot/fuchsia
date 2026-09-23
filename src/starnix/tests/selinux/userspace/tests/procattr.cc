// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <unistd.h>

#include <optional>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"

namespace {

struct PrePolicyProcAttrs {
  fit::result<int, std::string> current;
  fit::result<int, std::string> exec;
  fit::result<int, std::string> fscreate;
  fit::result<int, std::string> keycreate;
  fit::result<int, std::string> prev;
  fit::result<int, std::string> sockcreate;
};

std::optional<PrePolicyProcAttrs> g_pre_policy_procattrs;

}  // namespace

extern std::string DoPrePolicyLoadWork() {
  g_pre_policy_procattrs = PrePolicyProcAttrs{
      .current = ReadTaskAttr("current"),
      .exec = ReadTaskAttr("exec"),
      .fscreate = ReadTaskAttr("fscreate"),
      .keycreate = ReadTaskAttr("keycreate"),
      .prev = ReadTaskAttr("prev"),
      .sockcreate = ReadTaskAttr("sockcreate"),
  };

  return "minimal_policy";
}

namespace {

TEST(ProcAttrTest, PrePolicyAttrs) {
  ASSERT_TRUE(g_pre_policy_procattrs.has_value());
  EXPECT_THAT(g_pre_policy_procattrs->current, SyscallResultIsOk("kernel"));
  EXPECT_THAT(g_pre_policy_procattrs->prev, SyscallResultIsOk("kernel"));
  EXPECT_THAT(g_pre_policy_procattrs->exec, SyscallResultIsOk(""));
  EXPECT_THAT(g_pre_policy_procattrs->fscreate, SyscallResultIsOk(""));
  EXPECT_THAT(g_pre_policy_procattrs->keycreate, SyscallResultIsOk(""));
  EXPECT_THAT(g_pre_policy_procattrs->sockcreate, SyscallResultIsOk(""));
}

// Attempting to read the process' current context should return a value.
TEST(ProcAttrTest, Current) {
  EXPECT_THAT(ReadTaskAttr("current"), SyscallResultIsOk("system_u:unconfined_r:unconfined_t:s0"));
}

TEST(ProcAttrTest, AttrsAreSeekable) {
  auto current_attr = fbl::unique_fd(open("/proc/self/attr/current", O_RDONLY));
  ASSERT_TRUE(current_attr.is_valid()) << strerror(errno);

  // Seek position is retained, so it is possible to read the value in chunks.
  std::string context;
  for (;;) {
    char buf[2];
    auto result = read(current_attr.get(), buf, sizeof(buf));
    ASSERT_GE(result, 0) << strerror(errno);
    if (result == 0) {
      break;
    }
    context.append(buf, result);
  }

  EXPECT_EQ(RemoveTrailingNul(context), "system_u:unconfined_r:unconfined_t:s0");

  // It is possible to seek back to the start of the file and read it all again.
  auto result = lseek(current_attr.get(), 0, SEEK_SET);
  ASSERT_EQ(result, 0) << strerror(errno);
  ASSERT_TRUE(files::ReadFileDescriptorToString(current_attr.get(), &context));
  EXPECT_EQ(RemoveTrailingNul(context), "system_u:unconfined_r:unconfined_t:s0");

  // It is possible to seek into the middle and read from there.
  result = lseek(current_attr.get(), 9, SEEK_SET);
  ASSERT_EQ(result, 9) << strerror(errno);
  ASSERT_TRUE(files::ReadFileDescriptorToString(current_attr.get(), &context));
  EXPECT_EQ(RemoveTrailingNul(context), "unconfined_r:unconfined_t:s0");
}

// Writable attributes validate the contexts written to them.
TEST(ProcAttrTest, WritableAttrsValidateContexts) {
  // Write a valid context and verify that it was set.
  EXPECT_THAT(WriteTaskAttr("exec", "system_u:unconfined_r:unconfined_t:s0"), SyscallResultIsOk());
  EXPECT_THAT(ReadTaskAttr("exec"), SyscallResultIsOk("system_u:unconfined_r:unconfined_t:s0"));

  // Write an invalid context and verify that nothing is changed.
  EXPECT_THAT(WriteTaskAttr("exec", "system_u:invalid_role_r:unconfined_t:s0"),
              SyscallResultIsErrno(EINVAL));
  EXPECT_THAT(ReadTaskAttr("exec"), SyscallResultIsOk("system_u:unconfined_r:unconfined_t:s0"));
}

// Writing a single NUL clears the attribute.
TEST(ProcAttrTest, WritableAttrsClearedByNul) {
  // Set a valid context, then clear it with NUL.
  ASSERT_THAT(WriteTaskAttr("exec", "system_u:unconfined_r:unconfined_t:s0"), SyscallResultIsOk());
  ASSERT_THAT(ReadTaskAttr("exec"), SyscallResultIsOk("system_u:unconfined_r:unconfined_t:s0"));
  EXPECT_THAT(WriteTaskAttr("exec", std::string_view("\0", 1)), SyscallResultIsOk());
  EXPECT_THAT(ReadTaskAttr("exec"), SyscallResultIsOk(""));
}

// Writing a single newline clears the attribute.
TEST(ProcAttrTest, WritableAttrsClearedByNewline) {
  // Set a valid context, then clear it with newline.
  ASSERT_THAT(WriteTaskAttr("exec", "system_u:unconfined_r:unconfined_t:s0"), SyscallResultIsOk());
  ASSERT_THAT(ReadTaskAttr("exec"), SyscallResultIsOk("system_u:unconfined_r:unconfined_t:s0"));
  EXPECT_THAT(WriteTaskAttr("exec", "\n"), SyscallResultIsOk());
  EXPECT_THAT(ReadTaskAttr("exec"), SyscallResultIsOk(""));
}

// Writing a valid context across multiple writes is not valid.
TEST(ProcAttrTest, ContextsMustBeWrittenInASingleWrite) {
  auto exec_attr = fbl::unique_fd(open("/proc/self/attr/exec", O_RDWR));
  ASSERT_TRUE(exec_attr.is_valid()) << strerror(errno);

  constexpr char kFirstPart[] = "system_u:unconfined_r";
  constexpr char kSecondPart[] = ":unconfined_t:s0";

  auto result = write(exec_attr.get(), kFirstPart, sizeof(kFirstPart));
  EXPECT_EQ(result, -1);
  EXPECT_EQ(errno, EINVAL);

  result = write(exec_attr.get(), kSecondPart, sizeof(kSecondPart));
  EXPECT_EQ(result, -1);
  EXPECT_EQ(errno, EINVAL);
}

// Writes are accepted, but only if the seek position is zero.
TEST(ProcAttrTest, WritableAttrsCanOnlyBeWrittenAtSeekPositionZero) {
  auto exec_attr = fbl::unique_fd(open("/proc/self/attr/exec", O_RDWR));
  ASSERT_TRUE(exec_attr.is_valid()) << strerror(errno);

  // Seek to a non-zero position.
  ASSERT_EQ(lseek(exec_attr.get(), 1, SEEK_SET), 1) << strerror(errno);

  // Write a valid context into the attribute.
  constexpr char kValidContext[] = "system_u:unconfined_r:unconfined_t:s0";
  auto result = write(exec_attr.get(), kValidContext, sizeof(kValidContext));
  EXPECT_EQ(result, -1);
  EXPECT_EQ(errno, EINVAL) << strerror(errno);
}

// Writes do not affect the seek position.
TEST(ProcAttrTest, WritesDoNotAffectSeekPosition) {
  auto exec_attr = fbl::unique_fd(open("/proc/self/attr/exec", O_RDWR));
  ASSERT_TRUE(exec_attr.is_valid()) << strerror(errno);

  // Write a valid context into the attribute.
  constexpr char kValidContext[] = "system_u:unconfined_r:unconfined_t:s0";
  auto result = write(exec_attr.get(), kValidContext, sizeof(kValidContext));
  ASSERT_EQ(result, (ssize_t)sizeof(kValidContext)) << strerror(errno);

  // Seek to a non-zero position.
  EXPECT_EQ(lseek(exec_attr.get(), 0, SEEK_CUR), 0) << strerror(errno);
}

// Multiple writes are accepted, so long as the contents are valid.
TEST(ProcAttrTest, WritableAttrsCanBeWrittenMoreThanOnce) {
  auto exec_attr = fbl::unique_fd(open("/proc/self/attr/exec", O_RDWR));
  ASSERT_TRUE(exec_attr.is_valid()) << strerror(errno);

  // Write a valid context into the attribute.
  constexpr char kValidContext[] = "system_u:unconfined_r:unconfined_t:s0";
  auto result = write(exec_attr.get(), kValidContext, sizeof(kValidContext));
  ASSERT_EQ(result, (ssize_t)sizeof(kValidContext)) << strerror(errno);

  // Read back the context.
  ASSERT_EQ(lseek(exec_attr.get(), 0, SEEK_SET), 0) << strerror(errno);
  std::string context;
  ASSERT_TRUE(files::ReadFileDescriptorToString(exec_attr.get(), &context));
  EXPECT_EQ(RemoveTrailingNul(context), kValidContext);

  // Seek back to position zero, so writes will be accepted, and write a NUL to
  // clear the attribute.
  ASSERT_EQ(lseek(exec_attr.get(), 0, SEEK_SET), 0) << strerror(errno);
  constexpr char kSingleNul[] = "\0";
  result = write(exec_attr.get(), kSingleNul, sizeof(kSingleNul));
  ASSERT_EQ(result, (ssize_t)sizeof(kSingleNul)) << strerror(errno);

  // Read back the context, which will now be empty.
  ASSERT_TRUE(files::ReadFileDescriptorToString(exec_attr.get(), &context));
  EXPECT_EQ(context, std::string());
}

void WaitForReadFd(int fd) {
  fd_set read_fds;
  FD_ZERO(&read_fds);
  FD_SET(fd, &read_fds);

  SAFE_SYSCALL(TEMP_FAILURE_RETRY(select(fd + 1, &read_fds, nullptr, nullptr, nullptr)));
  EXPECT_TRUE(FD_ISSET(fd, &read_fds));
}

TEST(ProcAttrTest, ZombieProcPid) {
  ASSERT_TRUE(RunSubprocessAs("unlabeled_u:unlabeled_r:unlabeled_t:s0", []() {
    pid_t child_pid = SAFE_SYSCALL(fork());
    if (child_pid == 0) {
      _exit(EXIT_SUCCESS);
    }

    auto pidfd = fbl::unique_fd(test_helper::PidFdOpen(child_pid, 0));
    WaitForReadFd(pidfd.get());

    // Validate the label reported for the child's "/proc/<pid>" directory.
    auto proc_pid_path = fxl::StringPrintf("/proc/%d", child_pid);
    ASSERT_EQ(access(proc_pid_path.data(), F_OK), 0)
        << "access() to " << proc_pid_path << " failed:" << strerror(errno);

    auto my_label = ReadTaskAttr("current");
    ASSERT_TRUE(my_label.is_ok()) << my_label.error_value();

    EXPECT_THAT(GetLabel(proc_pid_path), SyscallResultIsOk(my_label.value()));
  }));
}

TEST(ProcAttrTest, ZombieCurrentAttr) {
  ASSERT_TRUE(RunSubprocessAs("unlabeled_u:unlabeled_r:unlabeled_t:s0", []() {
    pid_t child_pid = SAFE_SYSCALL(fork());
    if (child_pid == 0) {
      _exit(EXIT_SUCCESS);
    }

    auto pidfd = fbl::unique_fd(test_helper::PidFdOpen(child_pid, 0));
    WaitForReadFd(pidfd.get());

    // Validate the label reported in the child's "/proc/<pid>/attr/current".
    auto current_attr_path = fxl::StringPrintf("/proc/%d/attr/current", child_pid);
    ASSERT_EQ(access(current_attr_path.data(), F_OK), 0)
        << "access() to " << current_attr_path << " failed:" << strerror(errno);
    auto current_attr = fbl::unique_fd(open(current_attr_path.data(), O_RDONLY));
    ASSERT_TRUE(current_attr.is_valid()) << strerror(errno);
    std::string context;
    ASSERT_TRUE(files::ReadFileDescriptorToString(current_attr.get(), &context));

    auto my_label = ReadTaskAttr("current");
    ASSERT_TRUE(my_label.is_ok()) << my_label.error_value();

    EXPECT_EQ(RemoveTrailingNul(context), my_label.value());
  }));
}

}  // namespace
