// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <sys/ptrace.h>

#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "ptrace_policy"; }

namespace {

// Returns the path to a binary under the test package's `data` directory.
std::string PathForExec(std::string_view binary_name) {
  return "data/bin/" + std::string(binary_name);
}

// When the `ptrace` permission is denied to the parent for the child task, a
// `ptrace(PTRACE_TRACEME,...)` call by the child should fail with EACCES and
// the child program should exit normally in the absence of other errors.
TEST(PtraceTest, PtraceTraceMeDenied) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_ptrace_parent_deny_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_ptrace_child_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    pid_t pid;
    ASSERT_TRUE((pid = fork()) >= 0);
    if (pid == 0) {
      auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
      ASSERT_TRUE(set_exec_context.is_ok());

      std::string binary_name = "ptrace_traceme_bin";
      std::string path_for_exec = PathForExec(binary_name);
      std::string expect_success = std::to_string(false);
      char* const args[] = {binary_name.data(), expect_success.data(), nullptr};
      SAFE_SYSCALL(execv(path_for_exec.data(), args));
    } else {
      int wstatus;
      ASSERT_THAT(waitpid(pid, &wstatus, 0), SyscallSucceeds());
      EXPECT_TRUE(WIFEXITED(wstatus));
      EXPECT_EQ(WEXITSTATUS(wstatus), 0);
    }
  }));
}

// When the `ptrace` permission is granted to the parent for the child task, a
// `ptrace(PTRACE_TRACEME,...)` call by the child should succeed.
TEST(PtraceTest, PtraceTraceMeAllowed) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_ptrace_parent_allow_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_ptrace_child_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    pid_t pid;
    ASSERT_TRUE((pid = fork()) >= 0);
    if (pid == 0) {
      auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
      ASSERT_TRUE(set_exec_context.is_ok());

      std::string binary_name = "ptrace_traceme_bin";
      std::string path_for_exec = PathForExec(binary_name);
      std::string expect_success = std::to_string(true);
      char* const args[] = {binary_name.data(), expect_success.data(), nullptr};
      SAFE_SYSCALL(execv(path_for_exec.data(), args));
    } else {
      int wstatus;
      ASSERT_THAT(waitpid(pid, &wstatus, 0), SyscallSucceeds());
      EXPECT_TRUE(WIFEXITED(wstatus));
      EXPECT_EQ(WEXITSTATUS(wstatus), 0);
    }
  }));
}

// When the `ptrace` permission is denied to the parent for the child task, a
// `ptrace(PTRACE_ATTACH,...)` call by the parent should fail with EACCES and the
// child program should exit normally in the absence of other errors.
TEST(PtraceTest, PtraceAttachDenied) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_ptrace_parent_deny_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_ptrace_child_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    pid_t pid;
    ASSERT_TRUE((pid = fork()) >= 0);
    if (pid == 0) {
      // Exec into the child domain.
      auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
      ASSERT_TRUE(set_exec_context.is_ok());

      std::string binary_name = "stop_bin";
      std::string path_for_exec = PathForExec(binary_name);
      char* const args[] = {binary_name.data(), nullptr};
      SAFE_SYSCALL(execv(path_for_exec.data(), args));
    } else {
      // Wait for the child program to stop, then attempt to attach (expecting failure).
      int wstatus;
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      EXPECT_THAT(ptrace(PTRACE_ATTACH, pid, nullptr, nullptr), SyscallFailsWithErrno(EACCES));

      // Continue the child program and check that it exits normally.
      int continue_result = kill(pid, SIGCONT);
      ASSERT_EQ(continue_result, 0);
      if (continue_result != 0) {
        SAFE_SYSCALL(kill(pid, SIGKILL));
      }
      bool exited = false;
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      exited = WIFEXITED(wstatus);
      EXPECT_TRUE(exited);
      if (exited) {
        EXPECT_EQ(WEXITSTATUS(wstatus), 0);
      } else {
        SAFE_SYSCALL(kill(pid, SIGKILL));
      }
    }
  }));
}

// When the `ptrace` permission is granted to the parent for the child task, a
// `ptrace(PTRACE_ATTACH,...)` call by the parent should succeed.
TEST(PtraceTest, PtraceAttachAllowed) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_ptrace_parent_allow_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_ptrace_child_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    pid_t pid;
    ASSERT_TRUE((pid = fork()) >= 0);
    if (pid == 0) {
      // Exec into the child domain.
      auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
      ASSERT_TRUE(set_exec_context.is_ok());

      std::string binary_name = "stop_bin";
      std::string path_for_exec = PathForExec(binary_name);
      char* const args[] = {binary_name.data(), nullptr};
      SAFE_SYSCALL(execv(path_for_exec.data(), args));
    } else {
      // Wait for the child program to stop, then attempt to attach (expecting success).
      int wstatus;
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      EXPECT_THAT(ptrace(PTRACE_ATTACH, pid, nullptr, nullptr), SyscallSucceeds());

      // Continue the child program and check that it exits normally.
      // This requires 2 `PTRACE_CONT` commands, one to resume from the `PTRACE_ATTACH`
      // command and one to resume from the child's self-signaled `SIGSTOP`.
      // Use `ASSERT_THAT` to ensure the test does not hang on a ptrace fail.
      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      ASSERT_EQ(WSTOPSIG(wstatus), SIGSTOP);

      // Use `ASSERT_THAT` to ensure the test does not hang on a ptrace fail.
      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      bool exited = false;
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      exited = WIFEXITED(wstatus);
      EXPECT_TRUE(exited);
      if (exited) {
        EXPECT_EQ(WEXITSTATUS(wstatus), 0);
      } else {
        SAFE_SYSCALL(kill(pid, SIGKILL));
      }
    }
  }));
}

// When a traced task execs into a new domain while a tracer task is attached, the `ptrace`
// permission is checked for the tracer's context against the tracee's intended post-exec
// context. If the `ptrace` permission is denied, then the tracee's call to `exec` should
// fail with `EPERM`. The tracer should remain attached and the tracee should exit normally
// in the absence of other errors.
TEST(PtraceTest, PtraceAttachThenExecDenied) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_ptrace_parent_deny_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_ptrace_child_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    pid_t pid;
    ASSERT_TRUE((pid = fork()) >= 0);
    if (pid == 0) {
      // Stop and wait for the parent to attach.
      ASSERT_THAT(raise(SIGSTOP), SyscallSucceeds());

      // Exec into the child domain.
      auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
      ASSERT_TRUE(set_exec_context.is_ok());

      std::string binary_name = "stop_bin";
      std::string path_for_exec = PathForExec(binary_name);
      char* const args[] = {binary_name.data(), nullptr};
      EXPECT_THAT(execv(path_for_exec.data(), args), SyscallFailsWithErrno(EPERM));

      ASSERT_THAT(raise(SIGSTOP), SyscallSucceeds());
    } else {
      // Wait for the child program to stop, then attach.
      int wstatus;
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      ASSERT_THAT(ptrace(PTRACE_ATTACH, pid, nullptr, nullptr), SyscallSucceeds());

      // Continue the child program through its `exec` of `true_bin`.
      // This requires 2 `PTRACE_CONT` commands, one to resume from the `PTRACE_ATTACH`
      // command and one to resume from the child's self-signaled `SIGSTOP`.
      // Use `ASSERT_THAT` to ensure the test does not hang on a ptrace fail.
      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      ASSERT_THAT(waitpid(pid, &wstatus, 0), SyscallSucceeds());
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      ASSERT_EQ(WSTOPSIG(wstatus), SIGSTOP);

      // Use `ASSERT_THAT` to ensure the test does not hang on a ptrace fail.
      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      // At this point, the child has failed to exec.
      // Wait for the child to stop itself again.
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      EXPECT_TRUE(WIFSTOPPED(wstatus));
      EXPECT_EQ(WSTOPSIG(wstatus), SIGSTOP);

      // The parent process remains attached to the child.
      // Continue the child and observe normal exit.
      // Use `ASSERT_THAT` to ensure the test does not hang on a ptrace fail.
      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      bool exited = false;
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      exited = WIFEXITED(wstatus);
      EXPECT_TRUE(exited);
      if (exited) {
        EXPECT_EQ(WEXITSTATUS(wstatus), 0);
      } else {
        SAFE_SYSCALL(kill(pid, SIGKILL));
      }
    }
  }));
}

// When a traced task execs into a new domain while a tracer task is attached, the `ptrace`
// permission is checked for the tracer's context against the tracee's intended post-exec
// context. If the `ptrace` permission is allowed, then the tracee's call to `exec` should
// succeed in the absence of other errors. The tracer should remain attached and the
// tracee should exit normally.
TEST(PtraceTest, PtraceAttachThenExecAllowed) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_ptrace_parent_allow_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_ptrace_child_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    pid_t pid;
    ASSERT_TRUE((pid = fork()) >= 0);
    if (pid == 0) {
      // Stop and wait for the parent to attach.
      raise(SIGSTOP);

      // Exec into the child domain.
      auto set_exec_context = WriteTaskAttr("exec", kChildSecurityContext);
      ASSERT_TRUE(set_exec_context.is_ok());

      std::string binary_name = "true_bin";
      std::string path_for_exec = PathForExec(binary_name);
      char* const args[] = {binary_name.data(), nullptr};
      EXPECT_THAT(execv(path_for_exec.data(), args), SyscallSucceeds());
    } else {
      // Wait for the child program to stop, then attach.
      int wstatus;
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      ASSERT_THAT(ptrace(PTRACE_ATTACH, pid, nullptr, nullptr), SyscallSucceeds());

      // Continue the child program and check that it exits normally.
      // This requires 3 `PTRACE_CONT` commands, one to resume from the `PTRACE_ATTACH`
      // command, one to resume from the child's self-signaled `SIGSTOP`, and one to
      // resume after the child's `execve`.
      // Use `ASSERT_THAT` to ensure the test does not hang on a ptrace fail.
      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      ASSERT_THAT(waitpid(pid, &wstatus, 0), SyscallSucceeds());
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      ASSERT_EQ(WSTOPSIG(wstatus), SIGSTOP);

      // Use `ASSERT_THAT` to ensure the test does not hang on a ptrace fail.
      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      ASSERT_THAT(waitpid(pid, &wstatus, 0), SyscallSucceeds());
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      ASSERT_EQ(WSTOPSIG(wstatus), SIGTRAP);

      // Use `ASSERT_THAT` to ensure the test does not hang on a ptrace fail.
      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      bool exited = false;
      ASSERT_THAT(waitpid(pid, &wstatus, 0), SyscallSucceeds());
      exited = WIFEXITED(wstatus);
      EXPECT_TRUE(exited);
      if (exited) {
        EXPECT_EQ(WEXITSTATUS(wstatus), 0);
      } else {
        SAFE_SYSCALL(kill(pid, SIGKILL));
      }
    }
  }));
}

// Reading /proc/<pid>/maps triggers a PTRACE_MODE_READ_FSCREDS check. The read
// part implies that we should check a file { read } permission instead of
// process { ptrace }.
TEST(PtraceTest, ReadProcMaps) {
  constexpr char kParentSecurityContext[] =
      "test_u:test_r:test_ptrace_parent_deny_ptrace_allow_read_t:s0";
  constexpr char kChildSecurityContext[] = "test_u:test_r:test_ptrace_child_t:s0";
  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    pid_t pid;
    ASSERT_TRUE((pid = fork()) >= 0);
    if (pid == 0) {
      ASSERT_TRUE(WriteTaskAttr("current", kChildSecurityContext).is_ok());
      raise(SIGSTOP);
      // Never resumed.
    }
    int wstatus;
    ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
    ASSERT_TRUE(WIFSTOPPED(wstatus));
    EXPECT_THAT(open(("/proc/" + std::to_string(pid) + "/maps").c_str(), O_RDONLY),
                SyscallSucceeds());
    kill(pid, 9);
  }));
}

TEST(PtraceTest, PtraceAttachDeniedWithoutSelinuxCapSysPtrace) {
  constexpr char kParentSecurityContext[] = "test_u:test_r:test_ptrace_parent_allow_t:s0";

  auto enforce = ScopedEnforcement::SetEnforcing();

  ASSERT_TRUE(RunSubprocessAs(kParentSecurityContext, [&] {
    test_helper::ForkHelper helper;

    pid_t tracee_pid = helper.RunInForkedProcess([] { raise(SIGSTOP); });
    ASSERT_GT(tracee_pid, 0);

    pid_t tracer_pid = helper.RunInForkedProcess([&] {
      // Drop a specific capability to make child's caps a non-superset of parent's.
      test_helper::UnsetCapabilityEffective(CAP_SYSLOG);
      test_helper::UnsetCapabilityPermitted(CAP_SYSLOG);

      // Verify that SELinux 'capability:sys_ptrace' permission is required, even if the tracer has
      // 'CAP_SYS_PTRACE'.
      EXPECT_THAT(ptrace(PTRACE_ATTACH, tracee_pid, nullptr, nullptr),
                  SyscallFailsWithErrno(EPERM));
    });

    // Wait for tracer to complete.
    test_helper::ForkResult tracer_result = helper.WaitForChild(tracer_pid);
    EXPECT_TRUE(tracer_result.determined_result);

    // Tear down the tracee.
    EXPECT_THAT(kill(tracee_pid, SIGKILL), SyscallSucceeds());
    helper.ExpectSignal(SIGKILL);
    test_helper::ForkResult tracee_result = helper.WaitForChild(tracee_pid);
    EXPECT_TRUE(tracee_result.determined_result);
  }));
}

}  // namespace
