// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <signal.h>
#include <sys/prctl.h>
#include <sys/resource.h>
#include <sys/syscall.h>
#include <sys/time.h>

#include <array>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/lib/files/file.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "inherit_policy"; }

namespace {

constexpr char kTmpFilePathTemplate[] = "/tmp/inherit_test_file:XXXXXX";

fit::result<int, std::string> CreateTmpFile() {
  std::string file_path(kTmpFilePathTemplate);
  fbl::unique_fd fd(mkstemp(file_path.data()));
  if (!fd.is_valid()) {
    return fit::error(errno);
  }
  return fit::ok(std::move(file_path));
}

// Returns the path to a binary under the test package's `data` directory.
std::string PathForExec(std::string_view binary_name) {
  return "data/bin/" + std::string(binary_name);
}

// Try to execute a binary in a situation where the post-exec domain does not
// have the `use` permission for file descriptors opened in the pre-exec domain.
// On Linux and Starnix, the executed program segfaults.
TEST(InheritTest, ExecutableFdRemappedToNull) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_use_fd_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectSignal(SIGSEGV);

  RunInForkedProcessWithLabel(fork_helper, kParentSecurityContext, [&] {
    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());

    std::string binary_name = "true_bin";
    std::string path_for_exec = PathForExec(binary_name);
    char* const args[] = {binary_name.data(), nullptr};
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  });

  ASSERT_TRUE(fork_helper.WaitForChildren());
}

// Execute a binary in a situation where the post-exec domain has the `use`
// permission for file descriptors opened by the pre-exec domain. The executed
// program should exit normally.
TEST(InheritTest, ExecutableFdUseAllowed) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_allow_use_fd_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());

    std::string binary_name = "true_bin";
    std::string path_for_exec = PathForExec(binary_name);
    char* const args[] = {binary_name.data(), nullptr};
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// Under the parent domain, open a test file such that the child domain does not have the
// `fd { use }` permission on the file descriptor. Then exec into the child domain via an
// intermediate domain. The child program checks that the test file descriptor was remapped
// to the null file.
TEST(InheritTest, FdUseDeniedFdRemappedToNull) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kBridgeSecurityContext[] = "test_u:test_r:test_inherit_bridge_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_use_fd_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    auto tmp_file_path = CreateTmpFile();
    ASSERT_TRUE(tmp_file_path.is_ok());
    int no_use_fd = open(tmp_file_path.value().data(), O_RDONLY);
    ASSERT_TRUE(no_use_fd >= 0);
    std::string no_use_fd_str = std::to_string(no_use_fd);

    ASSERT_TRUE(RunSubprocessAs(kBridgeSecurityContext, [&] {
      // Exec the `is_selinux_null_inode` binary and expect that `no_use_fd` is remapped.
      std::string binary_name = "is_selinux_null_inode_bin";
      std::string path_for_exec = PathForExec(binary_name);
      std::string expect_null_inode = std::to_string(int(true));
      char* const args[] = {binary_name.data(), no_use_fd_str.data(), expect_null_inode.data(),
                            nullptr};

      ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
      SAFE_SYSCALL(execv(path_for_exec.data(), args));
    }));
  }));
}

// Under the parent domain, open a test file twice such that the child domain does not have the
// `fd { use }` permission on the file descriptor, so that the two open file descriptors should
// be remapped to the selinuxfs null node during exec. Then exec into the child domain via an
// intermediate domain. The child program checks that the two file descriptors are duplicates:
// they have independent file descriptor flag state, but refer to the same file description.
TEST(InheritTest, NullFileDescriptorIsDuplicated) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kBridgeSecurityContext[] = "test_u:test_r:test_inherit_bridge_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_use_fd_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    auto tmp_file_path = CreateTmpFile();
    ASSERT_TRUE(tmp_file_path.is_ok());

    int no_use_fd_1 = open(tmp_file_path.value().data(), O_RDONLY);
    ASSERT_TRUE(no_use_fd_1 >= 0);
    std::string no_use_fd_1_str = std::to_string(no_use_fd_1);

    int no_use_fd_2 = open(tmp_file_path.value().data(), O_RDONLY);
    ASSERT_TRUE(no_use_fd_2 >= 0);
    std::string no_use_fd_2_str = std::to_string(no_use_fd_2);

    ASSERT_TRUE(RunSubprocessAs(kBridgeSecurityContext, [&] {
      // Exec the `is_duplicated_fd` binary and expect that `no_use_fd_1` and
      // `no_use_fd_2` are remapped to the same file description (for the null node).
      std::string binary_name = "is_duplicated_fd_bin";
      std::string path_for_exec = PathForExec(binary_name);
      std::string expect_null_inode = std::to_string(int(true));
      char* const args[] = {binary_name.data(), no_use_fd_1_str.data(), no_use_fd_2_str.data(),
                            nullptr};

      ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
      SAFE_SYSCALL(execv(path_for_exec.data(), args));
    }));
  }));
}

// Under the parent domain, open a test file such that the child domain does have the `fd { use }`
// permission on the file descriptor, but does not have the `read` permission on the file. Then exec
// into the child domain. The child program checks that the test file descriptor was remapped to the
// null file.
TEST(InheritTest, FsNodePermissionDeniedFdRemappedToNull) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_read_file_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    auto tmp_file_path = CreateTmpFile();
    ASSERT_TRUE(tmp_file_path.is_ok());
    int no_use_fd = open(tmp_file_path.value().data(), O_RDONLY);
    ASSERT_TRUE(no_use_fd >= 0);
    std::string no_use_fd_str = std::to_string(no_use_fd);

    // Exec the `is_selinux_null_inode` binary and expect that `no_use_fd` is remapped.
    std::string binary_name = "is_selinux_null_inode_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expect_null_inode = std::to_string(int(true));
    char* const args[] = {binary_name.data(), no_use_fd_str.data(), expect_null_inode.data(),
                          nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// Under the parent domain, open a test file such that the child domain has the `fd { use }`
// permission on the file descriptor and has the appropriate file class permissions on the file.
// Then exec into the child domain. The child program checks that the test file descriptor was
// not remapped to the null file.
TEST(InheritTest, FdUseAllowed) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_allow_use_fd_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    auto tmp_file_path = CreateTmpFile();
    ASSERT_TRUE(tmp_file_path.is_ok());
    int allow_use_fd = open(tmp_file_path.value().data(), O_RDONLY);
    ASSERT_TRUE(allow_use_fd >= 0);
    std::string allow_use_fd_str = std::to_string(allow_use_fd);

    // Exec the `is_selinux_null_inode` binary and expect that `allow_use_fd` is not remapped.
    std::string binary_name = "is_selinux_null_inode_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expect_null_inode = std::to_string(int(false));
    char* const args[] = {binary_name.data(), allow_use_fd_str.data(), expect_null_inode.data(),
                          nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// Verify that a file-descriptor marked with `O_CLOEXEC` will be closed before the SELinux access
// checks are applied, and will therefore be invalid by the time the new program executes, rather
// than having been remapped to null, and will not have triggered any SELinux denial audit logs.
TEST(InheritTest, CloexecProcessedBeforeSelinuxCheck) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_inherit_parent_t:s0", [&] {
    const auto tmp_file_path = CreateTmpFile();
    ASSERT_TRUE(tmp_file_path.is_ok());
    const fbl::unique_fd fd(open(tmp_file_path.value().data(), O_RDONLY | O_CLOEXEC));
    ASSERT_TRUE(fd.is_valid());

    ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_inherit_bridge_t:s0", [&] {
      test_helper::ForkHelper fork_helper;
      fork_helper.ExpectExitValue(1);

      fork_helper.RunInForkedProcess([&] {
        ASSERT_TRUE(
            WriteTaskAttr("exec", "test_u:test_r:test_inherit_child_no_use_fd_t:s0").is_ok());

        std::string fd_str = std::to_string(fd.get());
        std::string bin_name = "is_fd_valid_bin";
        const std::string path = PathForExec(bin_name);
        char* const args[] = {bin_name.data(), fd_str.data(), nullptr};
        SAFE_SYSCALL(execv(path.c_str(), args));
      });
    }));
  }));
}

// Verify that dynamic transitions (those made without replacing the address-space with `exec()`)
// do not cause file-descriptors to be remapped to null if the new domain cannot use them.
TEST(InheritTest, DynamicTransitionFdRemainsValid) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:test_inherit_parent_t:s0", [&] {
    const auto tmp_file_path = CreateTmpFile();
    ASSERT_TRUE(tmp_file_path.is_ok());
    const fbl::unique_fd fd(open(tmp_file_path.value().data(), O_RDONLY));
    ASSERT_TRUE(fd.is_valid());

    // Attempt dynamic transition to child domain.
    // Note: This might fail if the policy does not allow dyntransition between these domains.
    auto transition_result =
        WriteTaskAttr("current", "test_u:test_r:test_inherit_child_no_use_fd_t:s0");
    ASSERT_TRUE(transition_result.is_ok()) << "Failed to transition dynamically";

    // Verify that the FD is still valid.
    EXPECT_THAT(fcntl(fd.get(), F_GETFD), SyscallSucceeds());

    // Verify that the FD was not remapped to the null inode.
    const fbl::unique_fd null_fd(open("/sys/fs/selinux/null", O_RDONLY));
    ASSERT_TRUE(null_fd.is_valid());
    EXPECT_THAT(IsSameInode(fd.get(), null_fd.get()), SyscallResultIsOk(false));

    // Verify that the FD is not accessible.
    // We try to read from it. If SELinux revalidates on use or Starnix revokes access, this should
    // fail.
    char buf[1];
    EXPECT_THAT(read(fd.get(), buf, 1), SyscallFailsWithErrno(EACCES));
  }));
}

// When the `siginh` permission is denied, the parent's ITIMER_REAL is reset during `exec`.
TEST(InheritTest, SiginhDeniedItimerRealReset) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_siginh_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    struct itimerval parent_val;
    parent_val.it_value.tv_sec = 1000000;
    parent_val.it_value.tv_usec = 0;
    parent_val.it_interval.tv_sec = 0;
    parent_val.it_interval.tv_usec = 0;

    ASSERT_THAT(setitimer(ITIMER_REAL, &parent_val, nullptr), SyscallSucceeds());

    std::string binary_name = "is_itimer_real_reset_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expect_itimer_real_reset = std::to_string(int(true));
    char* const args[] = {binary_name.data(), expect_itimer_real_reset.data(), nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// When the `siginh` permission is allowed, the parent's ITIMER_REAL is preserved across `exec`.
TEST(InheritTest, SiginhAllowedItimerRealInherited) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_allow_siginh_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    struct itimerval parent_val;
    parent_val.it_value.tv_sec = 1000000;
    parent_val.it_value.tv_usec = 0;
    parent_val.it_interval.tv_sec = 0;
    parent_val.it_interval.tv_usec = 0;

    ASSERT_THAT(setitimer(ITIMER_REAL, &parent_val, nullptr), SyscallSucceeds());

    std::string binary_name = "is_itimer_real_reset_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expect_itimer_real_reset = std::to_string(int(false));
    char* const args[] = {binary_name.data(), expect_itimer_real_reset.data(), nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// When the `siginh` permission is denied, the parent's pending non-fatal signals are cleared during
// `exec`.
TEST(InheritTest, SiginhDeniedPendingNonFatalSignalsCleared) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_siginh_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    sigset_t blocked_signals;
    sigemptyset(&blocked_signals);
    ASSERT_THAT(sigaddset(&blocked_signals, SIGCONT), SyscallSucceeds());
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &blocked_signals, nullptr), SyscallSucceeds());

    ASSERT_THAT(raise(SIGCONT), SyscallSucceeds());

    std::string binary_name = "has_pending_signals_bin";
    std::string path_for_exec = PathForExec(binary_name);
    // Expect no pending signals for the child program.
    char* const args[] = {binary_name.data(), nullptr};

    auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
    ASSERT_TRUE(set_exec_context.is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// When the `siginh` permission is denied, the parent's pending fatal signals are cleared during
// `exec`.
TEST(InheritTest, SiginhDeniedPendingFatalSignalsCleared) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_siginh_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    sigset_t blocked_signals;
    sigemptyset(&blocked_signals);
    ASSERT_THAT(sigaddset(&blocked_signals, SIGABRT), SyscallSucceeds());
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &blocked_signals, nullptr), SyscallSucceeds());

    ASSERT_THAT(raise(SIGABRT), SyscallSucceeds());

    std::string binary_name = "has_pending_signals_bin";
    std::string path_for_exec = PathForExec(binary_name);
    // Expect no pending signals for the child program..
    char* const args[] = {binary_name.data(), nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// When the `siginh` permission is allowed, the parent's pending signals are preserved across
// `exec`.
TEST(InheritTest, SiginhAllowedPendingSignalsInherited) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_allow_siginh_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    sigset_t blocked_signals;
    sigemptyset(&blocked_signals);
    ASSERT_THAT(sigaddset(&blocked_signals, SIGCONT), SyscallSucceeds());
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &blocked_signals, nullptr), SyscallSucceeds());

    ASSERT_THAT(raise(SIGCONT), SyscallSucceeds());

    std::string binary_name = "has_pending_signals_bin";
    std::string path_for_exec = PathForExec(binary_name);
    // Expect that SIGCONT is pending for the child program.
    std::string expect_sigcont = std::to_string(SIGCONT);
    char* const args[] = {binary_name.data(), expect_sigcont.data(), nullptr};

    auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
    ASSERT_TRUE(set_exec_context.is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// When the `siginh` permission is denied, the signal mask is cleared during `exec`.
TEST(InheritTest, SiginhDeniedSignalMaskReset) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_siginh_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    sigset_t blocked_signals;
    sigemptyset(&blocked_signals);
    ASSERT_THAT(sigaddset(&blocked_signals, SIGCONT), SyscallSucceeds());
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &blocked_signals, nullptr), SyscallSucceeds());

    std::string binary_name = "has_blocked_signals_bin";
    std::string path_for_exec = PathForExec(binary_name);
    // Expect no blocked signals for the child program.
    char* const args[] = {binary_name.data(), nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    if (execv(path_for_exec.data(), args) < 0) {
      perror("exec into child domain failed");
      FAIL();
    }
  }));
}

// When the `siginh` permission is allowed, the signal mask is inherited during `exec`.
TEST(InheritTest, SiginhAllowedSignalMaskInherited) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_allow_siginh_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    sigset_t blocked_signals;
    sigemptyset(&blocked_signals);
    ASSERT_THAT(sigaddset(&blocked_signals, SIGCONT), SyscallSucceeds());
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &blocked_signals, nullptr), SyscallSucceeds());

    std::string binary_name = "has_blocked_signals_bin";
    std::string path_for_exec = PathForExec(binary_name);
    // Expect that SIGCONT is blocked for the child program.
    std::string expect_sigcont = std::to_string(SIGCONT);
    char* const args[] = {binary_name.data(), expect_sigcont.data(), nullptr};

    auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
    ASSERT_TRUE(set_exec_context.is_ok());

    if (execv(path_for_exec.data(), args) < 0) {
      perror("exec into child domain failed");
      FAIL();
    }
  }));
}

// When the `siginh` permission is denied, signal dispositions are reset to the default during
// `exec`.
TEST(InheritTest, SiginhDeniedSignalDispositionsReset) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_siginh_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    struct sigaction action;
    action.sa_handler = SIG_IGN;
    ASSERT_THAT(sigaction(SIGCONT, &action, nullptr), SyscallSucceeds());

    std::string binary_name = "has_ignored_signals_bin";
    std::string path_for_exec = PathForExec(binary_name);
    // Expect that the child process has only default signal handlers.
    char* const args[] = {binary_name.data(), nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());

    if (execv(path_for_exec.data(), args) < 0) {
      perror("exec into child domain failed");
      FAIL();
    }
  }));
}

// When the `siginh` permission is allowed, dispositions for ignored signals are inherited
// across `exec`. (Any dispositions other than `SIG_IGN` or `SIG_DFL` are assumed to have
// been reset to the default at an earlier point in `exec`.)
TEST(InheritTest, SiginhAllowedIgnoredSignalDispositionsInherited) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_allow_siginh_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    struct sigaction action;
    action.sa_handler = SIG_IGN;
    ASSERT_THAT(sigaction(SIGCONT, &action, nullptr), SyscallSucceeds());

    std::string binary_name = "has_ignored_signals_bin";
    std::string path_for_exec = PathForExec(binary_name);
    // Expect that the child process ignores SIGCONT.
    std::string expect_sigcont = std::to_string(SIGCONT);
    char* const args[] = {binary_name.data(), expect_sigcont.data(), nullptr};

    auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
    ASSERT_TRUE(set_exec_context.is_ok());

    if (execv(path_for_exec.data(), args) < 0) {
      perror("exec into child domain failed");
      FAIL();
    }
  }));
}

// When the `rlimitinh` permission is denied, the parent's soft resource limits are reset during
// `exec`.
//
// Check this for `RLIMIT_FSIZE` as a representative example where there is some leeway to
// change the limits for testing purposes without interfering with the child program's execution.
TEST(InheritTest, RlimitinhDeniedSoftRlimitReset) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_no_rlimitinh_t:s0";
  constexpr rlim_t kParentSoftRlimitValue = 12345;
  constexpr rlim_t kParentHardRlimitValue = 67890;

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    struct rlimit prev_rlim = {};
    ASSERT_THAT(getrlimit(RLIMIT_FSIZE, &prev_rlim), SyscallSucceeds());
    ASSERT_GE(prev_rlim.rlim_max, kParentHardRlimitValue);

    struct rlimit rlim = {.rlim_cur = kParentSoftRlimitValue, .rlim_max = kParentHardRlimitValue};
    ASSERT_THAT(setrlimit(RLIMIT_FSIZE, &rlim), SyscallSucceeds());

    std::string binary_name = "is_soft_rlimit_reset_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expect_soft_limit_reset = std::to_string(int(true));
    std::string parent_soft_limit = std::to_string(kParentSoftRlimitValue);
    std::string parent_hard_limit = std::to_string(kParentHardRlimitValue);
    char* const args[] = {binary_name.data(), expect_soft_limit_reset.data(),
                          parent_soft_limit.data(), parent_hard_limit.data(), nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// When the `rlimitinh` permission is allowed, the parent's resource limits are preserved across
// `exec`.
//
// Check this for `RLIMIT_FSIZE` as a representative example where there is some leeway to
// change the limits for testing purposes without interfering with the child program's execution.
TEST(InheritTest, RlimitinhAllowedRlimitsInherited) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_allow_rlimitinh_t:s0";
  constexpr rlim_t kParentSoftRlimitValue = 12345;
  constexpr rlim_t kParentHardRlimitValue = 67890;

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    struct rlimit prev_rlim = {};
    ASSERT_THAT(getrlimit(RLIMIT_FSIZE, &prev_rlim), SyscallSucceeds());
    ASSERT_GE(prev_rlim.rlim_max, kParentHardRlimitValue);

    struct rlimit rlim = {.rlim_cur = kParentSoftRlimitValue, .rlim_max = kParentHardRlimitValue};
    ASSERT_THAT(setrlimit(RLIMIT_FSIZE, &rlim), SyscallSucceeds());

    std::string binary_name = "is_soft_rlimit_reset_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expect_soft_limit_reset = std::to_string(int(false));
    std::string parent_soft_limit = std::to_string(kParentSoftRlimitValue);
    std::string parent_hard_limit = std::to_string(kParentHardRlimitValue);
    char* const args[] = {binary_name.data(), expect_soft_limit_reset.data(),
                          parent_soft_limit.data(), parent_hard_limit.data(), nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// When the `noatsecure` permission is denied and the security domain is changing
// during exec, the `AT_SECURE` entry is set in the executable's auxiliary vector.
TEST(InheritTest, NoAtSecureDenied) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_deny_noatsecure_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    std::string binary_name = "is_at_secure_set_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expect_at_secure_set = std::to_string(int(true));
    char* const args[] = {binary_name.data(), expect_at_secure_set.data(), nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// When `noatsecure` is denied on a domain transition, `AT_SECURE` is set in the auxiliary vector,
// but ambient capabilities are preserved across `execve`.
TEST(InheritTest, NoAtSecureDeniedPreservesAmbientCapabilities) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_deny_noatsecure_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  test_helper::ForkHelper fork_helper;
  fork_helper.RunInForkedProcess([&] {
    __user_cap_header_struct header = {.version = _LINUX_CAPABILITY_VERSION_3, .pid = 0};
    std::array<__user_cap_data_struct, _LINUX_CAPABILITY_U32S_3> caps = {};
    SAFE_SYSCALL(syscall(SYS_capget, &header, caps.data()));
    caps[CAP_TO_INDEX(CAP_CHOWN)].inheritable |= CAP_TO_MASK(CAP_CHOWN);
    SAFE_SYSCALL(syscall(SYS_capset, &header, caps.data()));
    SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_RAISE, CAP_CHOWN, 0, 0));
    ASSERT_EQ(SAFE_SYSCALL(prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_IS_SET, CAP_CHOWN, 0, 0)), 1);

    ASSERT_TRUE(WriteTaskAttr("current", kParentSecurityContext).is_ok());
    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());

    std::string binary_name = "has_ambient_capabilities_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expected_cap = std::to_string(CAP_CHOWN);
    char* const args[] = {binary_name.data(), expected_cap.data(), nullptr};
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  });
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

// When the security domain does not change during exec, the `noatsecure` permission is not
// checked and the `AT_SECURE` entry is not set in the executable's auxiliary vector.
TEST(InheritTest, NoAtSecureDeniedSameDomain) {
  constexpr char kConstantSecurityContext[] = "test_u:test_r:test_inherit_constant_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kConstantSecurityContext, [&] {
    std::string binary_name = "is_at_secure_set_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expect_at_secure_set = std::to_string(int(false));
    char* const args[] = {binary_name.data(), expect_at_secure_set.data(), nullptr};
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// When the `noatsecure` permission is allowed and the security domain is changing
// during exec, the AT_SECURE entry is not set in the executable's auxiliary vector.
TEST(InheritTest, NoAtSecureAllowed) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_child_allow_noatsecure_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    std::string binary_name = "is_at_secure_set_bin";
    std::string path_for_exec = PathForExec(binary_name);
    std::string expect_at_secure_set = std::to_string(int(false));
    char* const args[] = {binary_name.data(), expect_at_secure_set.data(), nullptr};

    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    SAFE_SYSCALL(execv(path_for_exec.data(), args));
  }));
}

// Creates a temporary `#!` script and ELF interpreter labeled for script transition testing.
void PrepareScriptTest(const test_helper::ScopedTempDir& temp_dir, std::string* script_path) {
  constexpr char kInterpreterFileLabel[] = "test_u:object_r:test_inherit_interp_file_t:s0";
  constexpr char kScriptLabel[] = "test_u:object_r:test_inherit_script_file_t:s0";

  const std::string is_current_domain_bin_path = PathForExec("is_current_domain_bin");
  const std::string interpreter_path = temp_dir.path() + "/interpreter";
  std::string interpreter_content;
  ASSERT_TRUE(files::ReadFileToString(is_current_domain_bin_path, &interpreter_content));
  ASSERT_TRUE(files::WriteFile(interpreter_path, interpreter_content));
  ASSERT_THAT(chmod(interpreter_path.c_str(), 0755), SyscallSucceeds());
  ASSERT_TRUE(SetLabel(interpreter_path, kInterpreterFileLabel).is_ok());

  *script_path = temp_dir.path() + "/script.sh";
  std::string script_content = "#!" + interpreter_path + "\n";
  ASSERT_TRUE(files::WriteFile(*script_path, script_content));
  ASSERT_THAT(chmod(script_path->c_str(), 0755), SyscallSucceeds());
  ASSERT_TRUE(SetLabel(*script_path, kScriptLabel).is_ok());
}

// Verifies that executing a `#!` script transitions to the domain determined by the script's
// label rather than the interpreter's label.
TEST(InheritTest, ExecveScriptTransition) {
  constexpr char kInitialTaskContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kExpectedPostExecContext[] = "test_u:test_r:test_inherit_script_target_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string script_path;
  ASSERT_NO_FATAL_FAILURE(PrepareScriptTest(temp_dir, &script_path));

  ASSERT_TRUE(RunSubprocessAs(kInitialTaskContext, [&] {
    char* const argv[] = {const_cast<char*>(script_path.c_str()),
                          const_cast<char*>(kExpectedPostExecContext), nullptr};
    char* const envp[] = {nullptr};

    SAFE_SYSCALL(execve(script_path.c_str(), argv, envp));
  }));
}

// Verifies that denying the target domain `file { execute }` on the script interpreter terminates
// the process with `SIGSEGV`.
TEST(InheritTest, ExecveScriptInterpreterExecuteDenied) {
  constexpr char kInitialTaskContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_script_no_interp_exec_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string script_path;
  ASSERT_NO_FATAL_FAILURE(PrepareScriptTest(temp_dir, &script_path));

  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectSignal(SIGSEGV);

  RunInForkedProcessWithLabel(fork_helper, kInitialTaskContext, [&] {
    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    char* const argv[] = {const_cast<char*>(script_path.c_str()), nullptr};
    char* const envp[] = {nullptr};

    SAFE_SYSCALL(execve(script_path.c_str(), argv, envp));
  });

  ASSERT_TRUE(fork_helper.WaitForChildren());
}

// Verifies that denying the target domain `file { read }` on the script interpreter terminates
// the process with `SIGSEGV`.
TEST(InheritTest, ExecveScriptInterpreterReadDenied) {
  constexpr char kInitialTaskContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_script_no_interp_read_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string script_path;
  ASSERT_NO_FATAL_FAILURE(PrepareScriptTest(temp_dir, &script_path));

  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectSignal(SIGSEGV);

  RunInForkedProcessWithLabel(fork_helper, kInitialTaskContext, [&] {
    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    char* const argv[] = {const_cast<char*>(script_path.c_str()), nullptr};
    char* const envp[] = {nullptr};

    SAFE_SYSCALL(execve(script_path.c_str(), argv, envp));
  });

  ASSERT_TRUE(fork_helper.WaitForChildren());
}

// Verifies that denying the target domain `file { map }` on the script interpreter terminates
// the process with `SIGSEGV`.
TEST(InheritTest, ExecveScriptInterpreterMapDenied) {
  constexpr char kInitialTaskContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_script_no_interp_map_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string script_path;
  ASSERT_NO_FATAL_FAILURE(PrepareScriptTest(temp_dir, &script_path));

  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectSignal(SIGSEGV);

  RunInForkedProcessWithLabel(fork_helper, kInitialTaskContext, [&] {
    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    char* const argv[] = {const_cast<char*>(script_path.c_str()), nullptr};
    char* const envp[] = {nullptr};

    SAFE_SYSCALL(execve(script_path.c_str(), argv, envp));
  });

  ASSERT_TRUE(fork_helper.WaitForChildren());
}

// Verifies that denying the calling domain `file { execute }` on the script interpreter causes
// `execve` to fail with `EACCES` before credentials commit.
TEST(InheritTest, ExecveScriptCallerExecuteDenied) {
  constexpr char kCallerTaskContext[] = "test_u:test_r:test_inherit_parent_no_interp_exec_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string script_path;
  ASSERT_NO_FATAL_FAILURE(PrepareScriptTest(temp_dir, &script_path));

  ASSERT_TRUE(RunSubprocessAs(kCallerTaskContext, [&] {
    char* const argv[] = {const_cast<char*>(script_path.c_str()), nullptr};
    char* const envp[] = {nullptr};

    EXPECT_THAT(execve(script_path.c_str(), argv, envp), SyscallFailsWithErrno(EACCES));
  }));
}

// Creates a temporary ELF executable and custom PT_INTERP linker labeled for testing.
void PrepareCustomPtInterpTest(const test_helper::ScopedTempDir& temp_dir,
                               std::string* executable_path) {
  constexpr char kInterpreterFileLabel[] = "test_u:object_r:test_inherit_interp_file_t:s0";
  constexpr char kExecutableLabel[] = "test_u:object_r:test_inherit_custom_interp_exec_file_t:s0";

  const std::string dynamic_linker_path = test_helper::GetSystemDynamicLinkerPath();
  ASSERT_FALSE(dynamic_linker_path.empty());
  std::string dynamic_linker_content;
  ASSERT_TRUE(files::ReadFileToString(dynamic_linker_path, &dynamic_linker_content))
      << dynamic_linker_path;

  const std::string interp_path = temp_dir.path() + "/test_interp";
  ASSERT_TRUE(files::WriteFile(interp_path, dynamic_linker_content));
  ASSERT_THAT(chmod(interp_path.c_str(), 0755), SyscallSucceeds());
  ASSERT_TRUE(SetLabel(interp_path, kInterpreterFileLabel).is_ok());

  const std::string custom_bin_src = PathForExec("is_current_domain_custom_interp_bin");
  std::string custom_bin_content;
  ASSERT_TRUE(files::ReadFileToString(custom_bin_src, &custom_bin_content)) << custom_bin_src;

  *executable_path = temp_dir.path() + "/custom_bin";
  ASSERT_TRUE(files::WriteFile(*executable_path, custom_bin_content));
  ASSERT_THAT(chmod(executable_path->c_str(), 0755), SyscallSucceeds());
  ASSERT_TRUE(SetLabel(*executable_path, kExecutableLabel).is_ok());
}

// Verifies that executing an ELF binary with a custom `PT_INTERP` transitions to the domain
// determined by the main executable's label rather than the interpreter's label.
TEST(InheritTest, ExecvePtInterpTransition) {
  constexpr char kInitialTaskContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kExpectedPostExecContext[] = "test_u:test_r:test_inherit_script_target_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string executable_path;
  ASSERT_NO_FATAL_FAILURE(PrepareCustomPtInterpTest(temp_dir, &executable_path));

  ASSERT_TRUE(RunSubprocessAs(kInitialTaskContext, [&] {
    SAFE_SYSCALL(chdir(temp_dir.path().c_str()));
    char* const argv[] = {const_cast<char*>(executable_path.c_str()),
                          const_cast<char*>(kExpectedPostExecContext), nullptr};
    char* const envp[] = {nullptr};

    SAFE_SYSCALL(execve(executable_path.c_str(), argv, envp));
  }));
}

// Verifies that denying the calling domain `file { execute }` on `PT_INTERP` causes `execve` to
// fail with `EACCES` before credentials commit.
TEST(InheritTest, ExecvePtInterpCallerExecuteDenied) {
  constexpr char kCallerTaskContext[] = "test_u:test_r:test_inherit_parent_no_interp_exec_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string executable_path;
  ASSERT_NO_FATAL_FAILURE(PrepareCustomPtInterpTest(temp_dir, &executable_path));

  ASSERT_TRUE(RunSubprocessAs(kCallerTaskContext, [&] {
    SAFE_SYSCALL(chdir(temp_dir.path().c_str()));
    char* const argv[] = {const_cast<char*>(executable_path.c_str()), nullptr};
    char* const envp[] = {nullptr};

    EXPECT_THAT(execve(executable_path.c_str(), argv, envp), SyscallFailsWithErrno(EACCES));
  }));
}

// Verifies that denying the target domain `file { execute }` on `PT_INTERP` terminates the
// process with `SIGSEGV`.
TEST(InheritTest, ExecvePtInterpTargetExecuteDenied) {
  constexpr char kInitialTaskContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_script_no_interp_exec_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string executable_path;
  ASSERT_NO_FATAL_FAILURE(PrepareCustomPtInterpTest(temp_dir, &executable_path));

  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectSignal(SIGSEGV);

  RunInForkedProcessWithLabel(fork_helper, kInitialTaskContext, [&] {
    SAFE_SYSCALL(chdir(temp_dir.path().c_str()));
    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    char* const argv[] = {const_cast<char*>(executable_path.c_str()), nullptr};
    char* const envp[] = {nullptr};

    SAFE_SYSCALL(execve(executable_path.c_str(), argv, envp));
  });

  ASSERT_TRUE(fork_helper.WaitForChildren());
}

// Verifies that denying the target domain `file { read }` on `PT_INTERP` terminates the
// process with `SIGSEGV`.
TEST(InheritTest, ExecvePtInterpTargetReadDenied) {
  constexpr char kInitialTaskContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_script_no_interp_read_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string executable_path;
  ASSERT_NO_FATAL_FAILURE(PrepareCustomPtInterpTest(temp_dir, &executable_path));

  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectSignal(SIGSEGV);

  RunInForkedProcessWithLabel(fork_helper, kInitialTaskContext, [&] {
    SAFE_SYSCALL(chdir(temp_dir.path().c_str()));
    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    char* const argv[] = {const_cast<char*>(executable_path.c_str()), nullptr};
    char* const envp[] = {nullptr};

    SAFE_SYSCALL(execve(executable_path.c_str(), argv, envp));
  });

  ASSERT_TRUE(fork_helper.WaitForChildren());
}

// Verifies that denying the target domain `file { map }` on `PT_INTERP` terminates the
// process with `SIGSEGV`.
TEST(InheritTest, ExecvePtInterpTargetMapDenied) {
  constexpr char kInitialTaskContext[] = "test_u:test_r:test_inherit_parent_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_inherit_script_no_interp_map_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();
  test_helper::ScopedTempDir temp_dir;

  std::string executable_path;
  ASSERT_NO_FATAL_FAILURE(PrepareCustomPtInterpTest(temp_dir, &executable_path));

  test_helper::ForkHelper fork_helper;
  fork_helper.ExpectSignal(SIGSEGV);

  RunInForkedProcessWithLabel(fork_helper, kInitialTaskContext, [&] {
    SAFE_SYSCALL(chdir(temp_dir.path().c_str()));
    ASSERT_TRUE(WriteTaskAttr("exec", kChildSecurityContext).is_ok());
    char* const argv[] = {const_cast<char*>(executable_path.c_str()), nullptr};
    char* const envp[] = {nullptr};

    SAFE_SYSCALL(execve(executable_path.c_str(), argv, envp));
  });

  ASSERT_TRUE(fork_helper.WaitForChildren());
}

}  // namespace
