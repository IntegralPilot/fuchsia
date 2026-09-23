// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <elf.h>
#include <fcntl.h>
#include <poll.h>
#include <sched.h>
#include <signal.h>
#include <stdio.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/ptrace.h>
#include <sys/signalfd.h>
#include <sys/uio.h>
#include <sys/user.h>
#include <sys/wait.h>
#include <syscall.h>
#include <time.h>
#include <unistd.h>

#include <atomic>
#include <initializer_list>
#include <latch>
#include <thread>

#include <linux/prctl.h>
#include <linux/sched.h>

#include "src/lib/fxl/strings/string_printf.h"

#if defined(__riscv)
#include <asm/ptrace.h>
#endif  // __riscv

#include <gtest/gtest.h>
#include <linux/capability.h>

#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

constexpr int kOriginalSigno = SIGUSR1;
constexpr int kInjectedSigno = SIGUSR2;
constexpr int kInjectedErrno = EIO;

// user_regs_struct is not defined on __arm__
#if defined(__arm__)
struct user_regs_struct {
  unsigned long regs[18];
};
#endif  // defined(__arm__)

namespace {

TEST(PtraceTest, SetSigInfo) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  pid_t child_pid = helper.RunInForkedProcess([] {
    struct sigaction sa = {};
    sa.sa_sigaction = +[](int sig, siginfo_t *info, void *ucontext) {
      if (sig != kInjectedSigno) {
        _exit(1);
      }
      if (info->si_errno != kInjectedErrno) {
        _exit(2);
      }
      _exit(0);
    };

    sa.sa_flags = SA_SIGINFO | SA_RESTART;
    ASSERT_EQ(sigemptyset(&sa.sa_mask), 0);
    sigaction(kInjectedSigno, &sa, nullptr);
    sigaction(kOriginalSigno, &sa, nullptr);

    ASSERT_EQ(ptrace(PTRACE_TRACEME, 0, 0, 0), 0);
    raise(kOriginalSigno);
    _exit(3);
  });

  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == kOriginalSigno) << " status " << status;

  siginfo_t siginfo = {};
  ASSERT_EQ(ptrace(PTRACE_GETSIGINFO, child_pid, 0, &siginfo), 0)
      << "ptrace failed with error " << strerror(errno);
  ASSERT_EQ(kOriginalSigno, siginfo.si_signo);
  ASSERT_EQ(SI_TKILL, siginfo.si_code);

  // Replace the signal with kInjectedSigno, and check that the child exits
  // with kInjectedSigno, indicating that signal injection was successful.
  siginfo.si_signo = kInjectedSigno;
  siginfo.si_errno = kInjectedErrno;
  ASSERT_EQ(ptrace(PTRACE_SETSIGINFO, child_pid, 0, &siginfo), 0);
  ASSERT_EQ(ptrace(PTRACE_DETACH, child_pid, 0, kInjectedSigno), 0);
}

#ifndef PTRACE_EVENT_STOP  // Not defined in every libc
#define PTRACE_EVENT_STOP 128
#endif

TEST(PtraceTest, InterruptAfterListen) {
  volatile int child_should_spin = 1;
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  pid_t child_pid = helper.RunInForkedProcess([&child_should_spin] {
    const struct timespec req = {.tv_sec = 0, .tv_nsec = 1000};
    while (child_should_spin) {
      nanosleep(&req, nullptr);
    }
    _exit(0);
  });

  // In parent process.
  ASSERT_NE(child_pid, 0);

  ASSERT_EQ(ptrace(PTRACE_SEIZE, child_pid, 0, 0), 0);
  int status;
  ASSERT_EQ(waitpid(child_pid, &status, WNOHANG), 0);

  // Stop the child with PTRACE_INTERRUPT.
  ASSERT_EQ(ptrace(PTRACE_INTERRUPT, child_pid, 0, 0), 0);
  ASSERT_EQ(waitpid(child_pid, &status, 0), child_pid);
  ASSERT_EQ(SIGTRAP | (PTRACE_EVENT_STOP << 8), status >> 8);

  ASSERT_EQ(ptrace(PTRACE_POKEDATA, child_pid, &child_should_spin, 0), 0) << strerror(errno);

  // Send SIGSTOP to the child, then resume it, allowing it to proceed to
  // signal-delivery-stop.
  ASSERT_EQ(kill(child_pid, SIGSTOP), 0);
  ASSERT_EQ(ptrace(PTRACE_CONT, child_pid, 0, 0), 0);
  ASSERT_EQ(waitpid(child_pid, &status, 0), child_pid);
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << " status " << status;

  // Move out of signal-delivery-stop and deliver the SIGSTOP.
  ASSERT_EQ(ptrace(PTRACE_CONT, child_pid, 0, SIGSTOP), 0);
  ASSERT_EQ(waitpid(child_pid, &status, 0), child_pid);
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  ASSERT_EQ(SIGSTOP | (PTRACE_EVENT_STOP << 8), status >> 8);

  // Restart the child, but don't let it execute. Child continues to deliver
  // notifications of when it gets stop / continue signals.  This allows a
  // normal SIGCONT signal to be sent to a child to restart it, rather than
  // having the tracer restart it.  The tracer can then detect the SIGCONT.
  ASSERT_EQ(ptrace(PTRACE_LISTEN, child_pid, 0, 0), 0);

  // "If the tracee was already stopped by a signal and PTRACE_LISTEN was sent
  // to it, the tracee stops with PTRACE_EVENT_STOP and WSTOPSIG(status) returns
  // the stop signal."
  ASSERT_EQ(ptrace(PTRACE_INTERRUPT, child_pid, 0, 0), 0);
  ASSERT_EQ(waitpid(child_pid, &status, 0), child_pid);
  ASSERT_EQ(SIGSTOP | (PTRACE_EVENT_STOP << 8), status >> 8);

  // Allow the tracer to proceed normally.
  ASSERT_EQ(ptrace(PTRACE_CONT, child_pid, 0, 0), 0) << strerror(errno);
}

// None of this seems to be defined in our x64 and ARM sysroots.
#ifndef PTRACE_GET_SYSCALL_INFO
#define PTRACE_GET_SYSCALL_INFO 0x420e
#define PTRACE_SYSCALL_INFO_NONE 0
#define PTRACE_SYSCALL_INFO_ENTRY 1
#define PTRACE_SYSCALL_INFO_EXIT 2
#define PTRACE_SYSCALL_INFO_SECCOMP 3

struct ptrace_syscall_info {
  uint8_t op;
  uint8_t pad[3];
  uint32_t arch;
  uint64_t instruction_pointer;
  uint64_t stack_pointer;
  union {
    struct {
      uint64_t nr;
      uint64_t args[6];
    } entry;
    struct {
      int64_t rval;
      uint8_t is_error;
    } exit;
    struct {
      uint64_t nr;
      uint64_t args[6];
      uint32_t ret_data;
    } seccomp;
  };
};
#else
// In our RISC-V sysroot, this is called __ptrace_syscall_info
using ptrace_syscall_info = __ptrace_syscall_info;
#endif

constexpr int kMaxSyscallSteps = 20;

// Steps the tracee with PTRACE_SYSCALL until it stops at a syscall-enter stop matching
// one of the syscall numbers in `target_syscalls`.
//
// Standard libraries and runtime instrumentation can execute hidden intermediate syscalls
// (e.g. during raise(), signal masking, or thread setup), so this skips past uninteresting
// syscall stops until the expected syscall entry is reached.
//
// Assumes the tracee is already stopped and PTRACE_O_TRACESYSGOOD is enabled.
void StepToSyscallEntry(pid_t tracee_pid, std::initializer_list<uint64_t> target_syscalls) {
  for (int i = 0; i < kMaxSyscallSteps; ++i) {
    // Continue the tracee until the next syscall entry or exit.
    ASSERT_THAT(ptrace(PTRACE_SYSCALL, tracee_pid, 0, 0), SyscallSucceeds());
    int status;
    ASSERT_EQ(tracee_pid, waitpid(tracee_pid, &status, 0));
    ASSERT_TRUE(WIFSTOPPED(status))
        << "Tracee exited unexpectedly (status: " << status << ") before reaching target syscall.";
    ASSERT_EQ(WSTOPSIG(status), SIGTRAP | 0x80)
        << "Expected syscall stop (SIGTRAP | 0x80), got signal: " << WSTOPSIG(status);

    ptrace_syscall_info info;
    ASSERT_GT(ptrace(static_cast<enum __ptrace_request>(PTRACE_GET_SYSCALL_INFO), tracee_pid,
                     sizeof(info), &info),
              0)
        << strerror(errno);

    if (info.op == PTRACE_SYSCALL_INFO_ENTRY) {
      for (uint64_t target : target_syscalls) {
        if (info.entry.nr == target) {
          return;
        }
      }
    }
  }
  FAIL() << "Did not reach any target syscall within " << kMaxSyscallSteps << " steps.";
}

TEST(PtraceTest, TraceSyscall) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  pid_t child_pid = helper.RunInForkedProcess([] {
    ASSERT_EQ(ptrace(PTRACE_TRACEME, 0, 0, 0), 0);
    raise(SIGSTOP);
    struct timespec req = {.tv_sec = 0, .tv_nsec = 0};
    nanosleep(&req, nullptr);
  });

  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << " status " << status;
  ASSERT_EQ(0, ptrace(PTRACE_SETOPTIONS, child_pid, 0, PTRACE_O_TRACESYSGOOD))
      << "error " << strerror(errno);

  ptrace_syscall_info info;
  const int kExpectedNoneSize =
      reinterpret_cast<uint8_t *>(&info.entry) - reinterpret_cast<uint8_t *>(&info);
  const int kExpectedEntrySize =
      reinterpret_cast<uint8_t *>(&info.entry.args[6]) - reinterpret_cast<uint8_t *>(&info);
  const int kExpectedExitSize =
      reinterpret_cast<uint8_t *>(&info.exit.is_error + 1) - reinterpret_cast<uint8_t *>(&info);

  // We are not at a syscall entry
  ASSERT_EQ(ptrace(static_cast<enum __ptrace_request>(PTRACE_GET_SYSCALL_INFO), child_pid,
                   sizeof(ptrace_syscall_info), &info),
            kExpectedNoneSize);
  ASSERT_EQ(info.op, PTRACE_SYSCALL_INFO_NONE);

  ASSERT_NO_FATAL_FAILURE(StepToSyscallEntry(child_pid, {__NR_clock_nanosleep, __NR_nanosleep}));

  // We are now at a syscall entry
  ASSERT_EQ(ptrace(static_cast<enum __ptrace_request>(PTRACE_GET_SYSCALL_INFO), child_pid,
                   sizeof(ptrace_syscall_info), &info),
            kExpectedEntrySize);
  ASSERT_EQ(info.op, PTRACE_SYSCALL_INFO_ENTRY);

  // Step to syscall exit
  ASSERT_EQ(ptrace(PTRACE_SYSCALL, child_pid, 0, 0), 0);
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == (SIGTRAP | 0x80))
      << "WIFSTOPPED(status) " << WIFSTOPPED(status) << " WSTOPSIG(status) " << WSTOPSIG(status);

  // We are now at a syscall exit
  ASSERT_EQ(ptrace(static_cast<enum __ptrace_request>(PTRACE_GET_SYSCALL_INFO), child_pid,
                   sizeof(ptrace_syscall_info), &info),
            kExpectedExitSize);
  ASSERT_EQ(info.op, PTRACE_SYSCALL_INFO_EXIT);
  ASSERT_EQ(info.exit.rval, 0);
  ASSERT_EQ(info.exit.is_error, 0);

  ASSERT_EQ(ptrace(PTRACE_CONT, child_pid, 0, 0), 0);
}

#ifdef __x86_64__

static constexpr int kUnmaskedSignal = SIGUSR1;

// Linux has internal errnos that capture the circumstances when an interrupted
// syscall should restart rather than return.  These are ordinarily invisible to
// the user - the syscall is either restarted, or the internal errno is replaced
// by EINTR.  However, ptrace can detect them on ptrace-syscall-exit.
void TraceSyscallWithRestartWithCall(int call, long arg0, long arg1, long arg2, long arg3,
                                     int expected_errno) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.ExpectSignal(SIGKILL);
  pid_t child_pid = helper.RunInForkedProcess([call, arg0, arg1, arg2, arg3] {
    struct sigaction sa = {};
    sa.sa_handler = [](int signo) {};
    ASSERT_EQ(sigfillset(&sa.sa_mask), 0);
    ASSERT_EQ(sigaction(kUnmaskedSignal, &sa, nullptr), 0);
    ASSERT_EQ(sigprocmask(SIG_UNBLOCK, &sa.sa_mask, nullptr), 0);

    ASSERT_EQ(ptrace(PTRACE_TRACEME, 0, 0, 0), 0);
    raise(SIGSTOP);

    // When the following syscalls are interrupted, errno should be some weird
    // internal errno (expected_errno above).  This means that the syscall will
    // return -1 if it is interrupted by a signal that has a user handler.
    ASSERT_EQ(-1, syscall(call, arg0, arg1, arg2, arg3));
    ASSERT_EQ(EINTR, errno) << strerror(errno);
  });

  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  struct user_regs_struct regs = {};
  int count = 0;
  do {
    // Suppress the SIGSTOP and wait for the child to enter syscall-enter-stop
    // for the given syscall.  Repeat this in case we're using a libc where
    // raise() makes a syscall after sending the signal.
    ASSERT_EQ(ptrace(PTRACE_SYSCALL, child_pid, 0, 0), 0);
    ASSERT_EQ(waitpid(child_pid, &status, 0), child_pid);
    ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP) << " status " << status;

    ASSERT_EQ(ptrace(PTRACE_GETREGS, child_pid, 0, &regs), 0);
    count += 1;
  } while (static_cast<int>(regs.orig_rax) != call && count < 100);
  ASSERT_EQ(call, static_cast<int>(regs.orig_rax));
  ASSERT_EQ(-ENOSYS, static_cast<int>(regs.rax));

  // Resume the child with PTRACE_SYSCALL and expect it to block in the syscall.
  ASSERT_EQ(ptrace(PTRACE_SYSCALL, child_pid, 0, 0), 0);
  ASSERT_TRUE(test_helper::WaitUntilBlocked(child_pid, true));
  ASSERT_EQ(waitpid(child_pid, &status, WNOHANG), 0);

  // Send the child kUnmaskedSignal, causing it to return the given errno and enter
  // syscall-exit-stop from the syscall.
  ASSERT_EQ(kill(child_pid, kUnmaskedSignal), 0);
  ASSERT_EQ(waitpid(child_pid, &status, 0), child_pid);
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP) << " status " << status;

  ASSERT_EQ(ptrace(PTRACE_GETREGS, child_pid, 0, &regs), 0);
  ASSERT_EQ(call, static_cast<int>(regs.orig_rax));
  ASSERT_EQ(-expected_errno, static_cast<int>(regs.rax));

  kill(child_pid, SIGKILL);
  ptrace(PTRACE_DETACH, child_pid, 0, 0);
}

static constexpr int ERESTARTNOHAND = 514;
static constexpr int ERESTART_RESTARTBLOCK = 516;

TEST(PtraceTest, TraceSyscallWithRestart_pause) {
  ASSERT_NO_FATAL_FAILURE(TraceSyscallWithRestartWithCall(SYS_pause, 0, 0, 0, 0, ERESTARTNOHAND));
}

TEST(PtraceTest, TraceSyscallWithRestart_nanosleep) {
  const struct timespec req = {.tv_sec = 10, .tv_nsec = 0};
  ASSERT_NO_FATAL_FAILURE(TraceSyscallWithRestartWithCall(
      SYS_nanosleep, reinterpret_cast<long>(&req), 0, 0, 0, ERESTART_RESTARTBLOCK));
}

TEST(PtraceTest, TraceSyscallWithRestart_rt_sigsuspend) {
  sigset_t sigset;
  ASSERT_EQ(0, sigfillset(&sigset));
  ASSERT_EQ(0, sigdelset(&sigset, kUnmaskedSignal));
  ASSERT_NO_FATAL_FAILURE(
      TraceSyscallWithRestartWithCall(SYS_rt_sigsuspend, reinterpret_cast<long>(&sigset),
                                      sizeof(unsigned long), 0, 0, ERESTARTNOHAND));
}

TEST(PtraceTest, TraceSyscallWithRestart_ppoll) {
  struct timespec req = {.tv_sec = 10, .tv_nsec = 0};
  ASSERT_NO_FATAL_FAILURE(TraceSyscallWithRestartWithCall(
      SYS_ppoll, 0, 0, reinterpret_cast<long>(&req), 0, ERESTARTNOHAND));
}

TEST(PtraceTest, PokeUser) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  constexpr long kStartPattern = 0xabababab;
  constexpr long kEndPattern = 0xcdcdcdcd;

  pid_t child_pid = helper.RunInForkedProcess([kEndPattern] {
    ASSERT_EQ(ptrace(PTRACE_TRACEME, 0, 0, 0), 0);
    long output;

    asm volatile("movq %0, %%rdi"
                 :  // No output
                 : "r"(kStartPattern));
    // Use kill explicitly because we check the syscall argument register below.
    kill(getpid(), SIGSTOP);

    asm volatile("movq %%rdi, %0" : "=r"(output));
    ASSERT_EQ(output, kEndPattern);
  });

  ASSERT_NE(child_pid, 0);

  // Wait for the child to send itself SIGSTOP and enter signal-delivery-stop.
  int status;
  ASSERT_EQ(waitpid(child_pid, &status, 0), child_pid);
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << " status " << status;

  ASSERT_EQ(0,
            ptrace(PTRACE_POKEUSER, child_pid, offsetof(struct user_regs_struct, rdi), kEndPattern))
      << strerror(errno);

  ASSERT_EQ(0, ptrace(PTRACE_DETACH, child_pid, 0, SIGCONT));
}

#endif  // __x86_64__

TEST(PtraceTest, GetGeneralRegs) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  pid_t child_pid = helper.RunInForkedProcess([] {
    ASSERT_EQ(ptrace(PTRACE_TRACEME, 0, 0, 0), 0);

    // Use kill explicitly because we check the syscall argument register below.
    kill(getpid(), SIGSTOP);

    _exit(0);
  });
  ASSERT_NE(child_pid, 0);

  // Wait for the child to send itself SIGSTOP and enter signal-delivery-stop.
  int status;
  ASSERT_EQ(waitpid(child_pid, &status, 0), child_pid);
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << " status " << status;

#if defined(__x86_64__)
#define __REG rsi
#elif defined(__aarch64__) || defined(__arm__)
#define __REG regs[1]
#elif defined(__riscv)
#define __REG a1
#else
#error "Test does not support architecture for PTRACE_GETREGS";
#endif

  // Get the general registers with PTRACE_GETREGSET. Make this too large so
  // that ptrace can provide the correct value.
  struct user_regs_struct regs_set[2];
  struct iovec iov;
  iov.iov_base = regs_set;

  // Expect partial read on smaller size.
  iov.iov_len = sizeof(regs_set[0]) - 8;
  ASSERT_EQ(ptrace(PTRACE_GETREGSET, child_pid, NT_PRSTATUS, &iov), 0)
      << "Error " << errno << " " << strerror(errno);
  ASSERT_EQ(iov.iov_len, sizeof(regs_set[0]) - 8);

  // Provide a too large value for iov_len to make sure that ptrace resets it
  // correctly
  iov.iov_len = sizeof(regs_set);
  ASSERT_EQ(ptrace(PTRACE_GETREGSET, child_pid, NT_PRSTATUS, &iov), 0)
      << "Error " << errno << " " << strerror(errno);

  // Make sure ptrace set the correct size for the user_regs_struct.
  ASSERT_EQ(iov.iov_len, sizeof(struct user_regs_struct));

  // Child called kill(2), with SIGSTOP as arg 2.
  ASSERT_EQ(regs_set[0].__REG, static_cast<unsigned long>(SIGSTOP));

  // The appropriate defines for this are not in the ptrace header for arm64.
#ifdef __x86_64__
  // Get the general registers, with PTRACE_GETREGS
  struct user_regs_struct regs_old;
  ASSERT_EQ(ptrace(PTRACE_GETREGS, child_pid, nullptr, &regs_old), 0)
      << "Error " << errno << " " << strerror(errno);

  ASSERT_EQ(regs_old.__REG, static_cast<unsigned long>(SIGSTOP));
#endif

  // Get the appropriate general register with PTRACE_PEEKUSER
  ASSERT_EQ(ptrace(PTRACE_PEEKUSER, child_pid, offsetof(struct user_regs_struct, __REG), nullptr),
            SIGSTOP)
      << "Error " << errno << " " << strerror(errno);

  // Suppress SIGSTOP and resume the child.
  ASSERT_EQ(ptrace(PTRACE_DETACH, child_pid, 0, 0), 0);
}

namespace {
// As of this writing, our sysroot's syscall.h lacks the SYS_clone3 definition.
#ifndef SYS_clone3
#if defined(__aarch64__) || defined(__arm__) || defined(__x86_64__) || defined(__riscv)
constexpr int SYS_clone3 = 435;
#else
#error SYS_clone3 needs a definition for this architecture.
#endif
#endif

// Generate a child process that will spawn a grandchild process,both of which
// will be traced.  We use SYS_clone3 directly here, as it removes libc
// discretion about whether this is fork/clone/vfork.
void ForkUsingClone3(bool is_seized, uint64_t addl_clone_args, pid_t *out) {
  struct clone_args ca;
  memset(&ca, 0, sizeof(ca));

  ca.flags = addl_clone_args;
  ca.exit_signal = SIGCHLD;  // Needed in order to wait on the child.

  pid_t child_pid = static_cast<pid_t>(syscall(SYS_clone3, &ca, sizeof(ca)));
  if (child_pid == 0) {
    if (!is_seized) {
      ASSERT_EQ(ptrace(PTRACE_TRACEME, 0, 0, 0), 0);
    }
    raise(SIGSTOP);
    pid_t grandchild_pid = static_cast<pid_t>(syscall(SYS_clone3, &ca, sizeof(ca)));
    if (grandchild_pid == 0) {
      // Automatically does a SIGSTOP if started traced
      exit(0);
    }
    int status;
    ASSERT_EQ(grandchild_pid, waitpid(grandchild_pid, &status, 0)) << strerror(errno);
    ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0)
        << "Failure: WIFEXITED(status) =" << WIFEXITED(status)
        << " WEXITSTATUS(status) == " << WEXITSTATUS(status);
    exit(0);
  }
  ASSERT_GT(child_pid, 0) << strerror(errno);
  *out = child_pid;
}

template <typename T>
long get_event_msg(pid_t traced_pid, T *message) {
  unsigned long value;
  long return_code = ptrace(PTRACE_GETEVENTMSG, traced_pid, 0, &value);
  *message = static_cast<T>(value);
  return return_code;
}

void DetectForkAndContinue(pid_t child_pid, bool is_seized, bool child_stops_on_clone) {
  int status;
  pid_t grandchild_pid = 0;
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0));
  if (child_stops_on_clone) {
    // Continue until we hit a fork.
    ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));

    ASSERT_TRUE(WIFSTOPPED(status) && (status >> 8) == (SIGTRAP | (PTRACE_EVENT_FORK << 8)))
        << "status = " << status;

    // Get the grandchild's pid as reported by ptrace
    ASSERT_EQ(0, get_event_msg<pid_t>(child_pid, &grandchild_pid))
        << strerror(errno) << ": with child pid: " << child_pid;
    ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0))
        << strerror(errno) << " with child pid " << child_pid;
    // A grandchild started with TRACEFORK will start with a SIGSTOP or a PTRACE_EVENT_STOP
    // (depending on whether we used PTRACE_SEIZE to attach).
    ASSERT_EQ(grandchild_pid, waitpid(grandchild_pid, &status, 0)) << strerror(errno);
  } else {
    grandchild_pid = waitpid(0, &status, 0);
    ASSERT_NE(-1, grandchild_pid) << strerror(errno);
  }

  if (is_seized) {
    ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP)
        << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
        << " WSTOPSIG = " << WSTOPSIG(status);
    int shifted_status = status >> 8;
    ASSERT_TRUE(((PTRACE_EVENT_STOP << 8) | SIGTRAP) == shifted_status)
        << "shifted_status = " << shifted_status;
  } else {
    ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP)
        << " status " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
        << " WSTOPSIG = " << WSTOPSIG(status);
  }

  ASSERT_EQ(0, ptrace(PTRACE_CONT, grandchild_pid, 0, SIGCONT));

  // The grandchild should now exit.
  ASSERT_EQ(grandchild_pid, waitpid(grandchild_pid, &status, 0)) << strerror(errno);
  ASSERT_TRUE(WIFEXITED(status)) << "WIFEXITED(status) = " << WIFEXITED(status);

  // When the grandchild exits, the child receives a SIGCHLD.
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGCHLD);
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, SIGCHLD));

  // The child should now exit
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0)
      << "WIFEXITED(status) == " << WIFEXITED(status)
      << " WEXITSTATUS(status) == " << WEXITSTATUS(status);
}
}  // namespace

// After a successful `PTRACE_ATTACH`, the traced process should receive SIGSTOP.
TEST(PtraceTest, PtraceAttachSendSigstop) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([&] {
    pid_t pid;
    ASSERT_TRUE((pid = fork()) >= 0);
    if (pid == 0) {
      ASSERT_THAT(raise(SIGSTOP), SyscallSucceeds());
    } else {
      int wstatus;
      // Wait for the child to stop itself, then attach.
      ASSERT_THAT(waitpid(pid, &wstatus, WUNTRACED), SyscallSucceeds());
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      ASSERT_THAT(ptrace(PTRACE_ATTACH, pid, nullptr, nullptr), SyscallSucceeds());

      // Expect that the child has received SIGSTOP since the last wait.
      // In theory we can't expect the signal to be delivered by any specific deadline,
      // but let's give up if it hasn't arrived within 5 seconds.
      int wait_result = 0;
      ASSERT_GE((wait_result = waitpid(pid, &wstatus, WNOHANG)), 0);
      int retry_seconds = 0;
      while (wait_result == 0 && retry_seconds < 5) {
        sleep(1);
        retry_seconds++;
        ASSERT_GE((wait_result = waitpid(pid, &wstatus, WNOHANG)), 0);
      }
      EXPECT_EQ(wait_result, pid);
      EXPECT_TRUE(WIFSTOPPED(wstatus));
      EXPECT_EQ(WSTOPSIG(wstatus), SIGSTOP);

      // Clean up: resume the child from the SIGSTOP sent by `ptrace`, then from
      // the self-signaled SIGSTOP.
      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      ASSERT_THAT(waitpid(pid, &wstatus, 0), SyscallSucceedsWithValue(pid));
      ASSERT_TRUE(WIFSTOPPED(wstatus));
      ASSERT_EQ(WSTOPSIG(wstatus), SIGSTOP);

      ASSERT_THAT(ptrace(PTRACE_CONT, pid, nullptr, 0), SyscallSucceeds());
      ASSERT_THAT(waitpid(pid, &wstatus, 0), SyscallSucceeds());
      ASSERT_TRUE(WIFEXITED(wstatus));
      ASSERT_EQ(WEXITSTATUS(wstatus), 0);
    }
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(PtraceTest, PtraceEventStopWithFork) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }
  pid_t child_pid;
  ForkUsingClone3(false, 0, &child_pid);
  if (HasFatalFailure()) {
    return;
  }

  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << " status " << status;
  ASSERT_EQ(0, ptrace(PTRACE_SETOPTIONS, child_pid, 0, PTRACE_O_TRACEFORK))
      << "error " << strerror(errno);

  DetectForkAndContinue(child_pid, false, true);
}

TEST(PtraceTest, PtraceEventStopWithForkAndSeize) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }
  pid_t child_pid;
  ForkUsingClone3(true, 0, &child_pid);
  if (HasFatalFailure()) {
    return;
  }

  ASSERT_EQ(ptrace(PTRACE_SEIZE, child_pid, 0, PTRACE_O_TRACEFORK), 0) << strerror(errno);
  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << " status " << status;

  DetectForkAndContinue(child_pid, true, true);
}

TEST(PtraceTest, PtraceEventStopWithForkClonePtrace) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }
  pid_t child_pid;
  ForkUsingClone3(false, CLONE_PTRACE, &child_pid);
  if (HasFatalFailure()) {
    return;
  }
  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << " status " << status;

  DetectForkAndContinue(child_pid, false, false);
}

// Exercises the wakeup race on dynamic child ptrace attachment (e.g. CLONE_PTRACE). The tracer is
// forced to sleep in waitpid() before the child transitions to its initial stopped state to verify
// that the wakeup is not lost.
TEST(PtraceTest, PtraceEventStopWithForkClonePtraceWakeup) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    test_helper::Rendezvous ready = test_helper::MakeRendezvous();
    test_helper::Rendezvous attached = test_helper::MakeRendezvous();

    pid_t tracee_pid = fork();
    if (tracee_pid == 0) {
      ready.poker.poke();
      attached.holder.hold();

      // Spawn grandchild via clone3 with CLONE_PTRACE.
      struct clone_args args;
      memset(&args, 0, sizeof(args));
      args.flags = CLONE_PTRACE;
      args.exit_signal = SIGCHLD;

      pid_t grandchild_pid =
          SAFE_SYSCALL(static_cast<pid_t>(syscall(SYS_clone3, &args, sizeof(args))));
      if (grandchild_pid == 0) {
        // Automatically stopped due to CLONE_PTRACE.
        exit(0);
      }

      exit(0);
    }

    ready.holder.hold();

    // Attach to the running tracee. This will automatically send a single SIGSTOP.
    ASSERT_THAT(ptrace(PTRACE_ATTACH, tracee_pid, nullptr, nullptr), SyscallSucceeds());

    int status;
    ASSERT_THAT(waitpid(tracee_pid, &status, 0), SyscallSucceedsWithValue(tracee_pid));
    ASSERT_TRUE(WIFSTOPPED(status));
    ASSERT_EQ(WSTOPSIG(status), SIGSTOP);

    ASSERT_THAT(ptrace(PTRACE_SETOPTIONS, tracee_pid, 0, PTRACE_O_TRACEFORK), SyscallSucceeds());
    attached.poker.poke();
    ASSERT_THAT(ptrace(PTRACE_CONT, tracee_pid, 0, 0), SyscallSucceeds());

    // Force the tracer to immediately block in waitpid(-1, ...) before the grandchild actually
    // finishes spawning or stopping. This simulates the race where the tracer is already asleep
    // when the grandchild is attached from state.
    //
    // We use a loop because the parent's PTRACE_EVENT_FORK and the grandchild's initial SIGSTOP
    // can arrive in either order, and waitpid(-1) will return whichever happens first.
    pid_t grandchild_pid = 0;
    bool got_parent_fork = false;
    bool got_grandchild_stop = false;
    while (!got_parent_fork || !got_grandchild_stop) {
      int status;
      pid_t pid = SAFE_SYSCALL(waitpid(-1, &status, 0));
      if (pid == tracee_pid) {
        ASSERT_TRUE(WIFSTOPPED(status));
        ASSERT_EQ((status >> 8), (SIGTRAP | (PTRACE_EVENT_FORK << 8)));
        got_parent_fork = true;

        pid_t forked_pid = 0;
        ASSERT_THAT(get_event_msg<pid_t>(tracee_pid, &forked_pid), SyscallSucceeds());
        if (got_grandchild_stop) {
          ASSERT_EQ(forked_pid, grandchild_pid);
        } else {
          grandchild_pid = forked_pid;
        }
      } else {
        // This must be the grandchild.
        ASSERT_TRUE(WIFSTOPPED(status));
        ASSERT_EQ(WSTOPSIG(status), SIGSTOP);
        got_grandchild_stop = true;
        if (got_parent_fork) {
          ASSERT_EQ(pid, grandchild_pid);
        } else {
          grandchild_pid = pid;
        }
      }
    }

    // Detach the parent tracee. When the grandchild exits, it sends SIGCHLD to the parent tracee.
    // If the parent tracee is still traced, it will stop on this SIGCHLD, causing
    // waitpid(tracee_pid) to return a stop event instead of the exit event. Detaching it here
    // ensures it can exit without stopping, allowing us to wait for its clean exit below.
    ASSERT_THAT(ptrace(PTRACE_DETACH, tracee_pid, 0, 0), SyscallSucceeds());

    ASSERT_THAT(ptrace(PTRACE_CONT, grandchild_pid, 0, 0), SyscallSucceeds());
    ASSERT_THAT(waitpid(grandchild_pid, &status, 0), SyscallSucceedsWithValue(grandchild_pid));
    ASSERT_TRUE(WIFEXITED(status));

    // Wait for tracee to exit. It won't stop now because it is detached.
    ASSERT_THAT(waitpid(tracee_pid, &status, 0), SyscallSucceedsWithValue(tracee_pid));
    ASSERT_TRUE(WIFEXITED(status));
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(PtraceTest, PtraceEventStopWithVForkClonePtrace) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }
  pid_t child_pid = fork();
  if (child_pid == 0) {
    ASSERT_EQ(ptrace(PTRACE_TRACEME, 0, 0, 0), 0);
    raise(SIGSTOP);
    pid_t grandchild_pid = vfork();
    if (grandchild_pid == 0) {
      exit(99);
    }
    int status;
    ASSERT_EQ(grandchild_pid, waitpid(grandchild_pid, &status, 0));
    ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 99)
        << "Failure: WIFEXITED(status) =" << WIFEXITED(status)
        << " WEXITSTATUS(status) == " << WEXITSTATUS(status);
    exit(0);
  }
  ASSERT_LT(0, child_pid);
  pid_t grandchild_pid;
  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << " status " << status;
  ASSERT_EQ(0,
            ptrace(PTRACE_SETOPTIONS, child_pid, 0, PTRACE_O_TRACEVFORK | PTRACE_O_TRACEVFORKDONE));

  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0))
      << strerror(errno) << ": with child pid " << child_pid;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));

  ASSERT_TRUE(WIFSTOPPED(status) && (status >> 8) == (SIGTRAP | (PTRACE_EVENT_VFORK << 8)))
      << "status = " << status;

  // Get the grandchild's pid as reported by ptrace
  ASSERT_EQ(0, get_event_msg<pid_t>(child_pid, &grandchild_pid)) << strerror(errno);
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0))
      << strerror(errno) << ": with child pid " << child_pid;

  // Let the grandchild continue.
  ASSERT_EQ(grandchild_pid, waitpid(grandchild_pid, &status, 0)) << strerror(errno);
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << " status " << status;
  // Child should not have made progress..
  ASSERT_EQ(0, waitpid(child_pid, &status, WNOHANG)) << strerror(errno);
  ASSERT_EQ(0, ptrace(PTRACE_CONT, grandchild_pid, 0, 0)) << strerror(errno);
  ASSERT_EQ(grandchild_pid, waitpid(grandchild_pid, &status, 0)) << strerror(errno);
  ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 99)
      << "WIFEXITED(status) == " << WIFEXITED(status)
      << " WEXITSTATUS(status) == " << WEXITSTATUS(status);

  // Grandchild is done, child should continue.
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0)) << strerror(errno);
  ASSERT_TRUE(WIFSTOPPED(status) && (status >> 8) == (SIGTRAP | (PTRACE_EVENT_VFORK_DONE << 8)))
      << "status = " << status;
  ASSERT_EQ(0, ptrace(PTRACE_DETACH, child_pid, 0, 0)) << strerror(errno);
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0)) << strerror(errno);
  ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0)
      << "WIFEXITED(status) == " << WIFEXITED(status)
      << " WEXITSTATUS(status) == " << WEXITSTATUS(status);
}

constexpr int kBadExitStatus = 0xabababab;

void DoExec(pid_t *out) {
  pid_t child_pid = fork();
  if (child_pid == 0) {
    ASSERT_EQ(ptrace(PTRACE_TRACEME, 0, 0, 0), 0) << strerror(errno);
    raise(SIGSTOP);

    std::string test_binary = test_helper::GetTestResourcePath("ptrace_test_exec_child");
    char *const argv[] = {const_cast<char *>(test_binary.c_str()), nullptr};

    // execv happens without releasing futex, so futex's FUTEX_OWNER_DIED bit is set.
    execve(test_binary.c_str(), argv, nullptr);
    // Should not get here.
    _exit(kBadExitStatus);
  }
  *out = child_pid;
}

// Ensure that the tracee sends a SIGTRAP when it encounters an exec and
// TRACEEXEC is not enabled.
TEST(PtraceTest, ExecveWithSigtrap) {
  pid_t child_pid;
  DoExec(&child_pid);

  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0));

  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  ASSERT_EQ(0, ptrace(PTRACE_DETACH, child_pid, 0, 0));
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0)
      << "WIFEXITED(status) == " << WIFEXITED(status)
      << " WEXITSTATUS(status) == " << WEXITSTATUS(status);
}

// Ensure that, if TRACEEXIT is enabled, and the tracee executes an exit, it
// then sends a SIGTRAP | (PTRACE_EVENT_EXIT << 8)
TEST(PtraceTest, PtraceEventStopWithExit) {
  // TODO(https://fxbug.dev/322238868): This test does not work on the LTO
  // builder in CQ.
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }

  pid_t child_pid;
  DoExec(&child_pid);

  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  ASSERT_EQ(0, ptrace(PTRACE_SETOPTIONS, child_pid, 0, PTRACE_O_TRACEEXIT))
      << "error " << strerror(errno);
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0));

  // Wait for the exec
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0));

  // Wait for the exit
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  ASSERT_EQ(SIGTRAP | (PTRACE_EVENT_EXIT << 8), status >> 8);
  int exit_status = kBadExitStatus;
  ASSERT_EQ(get_event_msg<int>(child_pid, &exit_status), 0);
  // The actual exit status seems to change depending on how this test is run,
  // so just make sure that something is returned.
  ASSERT_TRUE(kBadExitStatus != exit_status)
      << "expected = " << kBadExitStatus << " actual: " << exit_status;
  ASSERT_EQ(0, ptrace(PTRACE_DETACH, child_pid, 0, 0)) << " with child pid " << child_pid;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0)
      << "WIFEXITED(status) == " << WIFEXITED(status)
      << " WEXITSTATUS(status) == " << WEXITSTATUS(status);
}

// Ensure that, if TRACEEXEC is enabled, and the tracee executes an exec, it
// then sends a SIGTRAP | (PTRACE_EVENT_EXEC << 8).
TEST(PtraceTest, PtraceEventStopWithExecve) {
  // TODO(https://fxbug.dev/322238868): This test does not work on the LTO
  // builder in CQ.
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }
  pid_t child_pid;
  DoExec(&child_pid);

  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  ASSERT_EQ(0, ptrace(PTRACE_SETOPTIONS, child_pid, 0, PTRACE_O_TRACEEXEC | PTRACE_O_TRACEEXIT))
      << "error " << strerror(errno);
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0));

  // Wait for the exec
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  ASSERT_EQ(SIGTRAP | (PTRACE_EVENT_EXEC << 8), status >> 8);
  pid_t target_pid;
  ASSERT_EQ(get_event_msg<pid_t>(child_pid, &target_pid), 0);
  ASSERT_EQ(target_pid, child_pid);

  ASSERT_EQ(0, ptrace(PTRACE_DETACH, child_pid, 0, 0))
      << strerror(errno) << ": with child pid " << child_pid;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0)
      << "WIFEXITED(status) == " << WIFEXITED(status)
      << " WEXITSTATUS(status) == " << WEXITSTATUS(status);
}

// Ensure that, if TRACEEXIT is enabled, and the tracee is killed with a
// SIGTERM, it sends a SIGTRAP | (PTRACE_EVENT_EXIT << 8)
TEST(PtraceTest, PtraceEventStopWithSignalExit) {
  // TODO(https://fxbug.dev/322238868): This test does not work on the LTO
  // builder in CQ.
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }

  pid_t child_pid;
  DoExec(&child_pid);

  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  ASSERT_EQ(0, ptrace(PTRACE_SETOPTIONS, child_pid, 0, PTRACE_O_TRACEEXIT))
      << "error " << strerror(errno);
  ASSERT_EQ(0, kill(child_pid, SIGTERM));
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0));

  // Wait for the signal-delivery-stop
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGTERM)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, SIGTERM));

  // Wait for the exit
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGTRAP)
      << "status = " << status << " WIFSTOPPED = " << WIFSTOPPED(status)
      << " WSTOPSIG = " << WSTOPSIG(status);

  ASSERT_EQ(SIGTRAP | (PTRACE_EVENT_EXIT << 8), status >> 8);
  int exit_status = 0xabababab;
  ASSERT_EQ(get_event_msg<int>(child_pid, &exit_status), 0);
  ASSERT_TRUE(SIGTERM == exit_status) << " exit_status " << exit_status;
  ASSERT_EQ(0, ptrace(PTRACE_DETACH, child_pid, 0, 0))
      << strerror(errno) << " with child pid " << child_pid;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSIGNALED(status) && WTERMSIG(status) == SIGTERM)
      << "WIFSIGNALED(status) == " << WIFEXITED(status)
      << " WTERMSIG(status) == " << WTERMSIG(status);
}

namespace {
void GrandchildWithSigsuspendSigaction(int, siginfo_t *, void *) {
  // NOP
}
}  // namespace

// Test that traced child correctly resumes when signal needs to be delivered
// because of a temporary mask.
TEST(PtraceTest, GrandchildWithSigsuspend) {
  // TODO(https://fxbug.dev/317285180) don't skip on baseline
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP() << "This test does not work on Linux in CQ";
  }
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  pid_t child_pid = helper.RunInForkedProcess([] {
    ASSERT_EQ(0, ptrace(PTRACE_TRACEME, 0, 0, 0));
    ASSERT_EQ(0, raise(SIGSTOP));
    sigset_t child_mask, old_mask;
    ASSERT_EQ(0, sigemptyset(&child_mask));
    ASSERT_EQ(0, sigaddset(&child_mask, SIGCHLD));

    sigset_t empty_mask;
    ASSERT_EQ(0, sigemptyset(&empty_mask));
    struct sigaction sa = {};
    struct sigaction oldact = {};
    sa.sa_sigaction = GrandchildWithSigsuspendSigaction;
    sa.sa_mask = empty_mask;
    ASSERT_EQ(0, sigaction(SIGCHLD, &sa, &oldact));
    pid_t my_pid = getpid();
    ASSERT_EQ(0, sigprocmask(SIG_BLOCK, &child_mask, &old_mask));
    pid_t gc_pid = fork();
    if (gc_pid == 0) {
      ASSERT_TRUE(test_helper::WaitUntilBlocked(my_pid, false));
      exit(0);
    }
    ASSERT_EQ(-1, sigsuspend(&old_mask));
    int status;
    ASSERT_EQ(gc_pid, waitpid(gc_pid, &status, 0));
    ASSERT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0)
        << "WIFEXITED(status) == " << WIFEXITED(status)
        << " WEXITSTATUS(status) == " << WEXITSTATUS(status);
  });
  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP)
      << WIFSTOPPED(status) << " " << WSTOPSIG(status);
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, 0));
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGCHLD)
      << WIFSTOPPED(status) << " " << WSTOPSIG(status);
  ASSERT_EQ(0, ptrace(PTRACE_CONT, child_pid, 0, SIGCHLD));
}

TEST(PtraceTest, ExitKill) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([]() {
    // Test that the PtraceOExitKill works as expected.
    // Set ourselves as the subreaper.
    SAFE_SYSCALL(prctl(PR_SET_CHILD_SUBREAPER, 1));

    pid_t tracer_pid = SAFE_SYSCALL(fork());
    if (tracer_pid == 0) {
      // We are the tracer. Spawn the tracee.
      pid_t tracee_pid = SAFE_SYSCALL(fork());
      if (tracee_pid == 0) {
        ASSERT_THAT(ptrace(PTRACE_TRACEME, 0, nullptr, nullptr), SyscallSucceeds());
        SAFE_SYSCALL(raise(SIGSTOP));
        _exit(EXIT_FAILURE);
      }
      int status;
      SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
      ASSERT_TRUE(WIFSTOPPED(status));
      EXPECT_THAT(ptrace(PTRACE_SETOPTIONS, tracee_pid, nullptr, PTRACE_O_EXITKILL),
                  SyscallSucceeds());
      // With this exit, the kernel will send a sigkill to the tracee.
      _exit(EXIT_SUCCESS);
    }

    int status;
    pid_t pid = SAFE_SYSCALL(waitpid(tracer_pid, &status, 0));
    EXPECT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0);

    pid = SAFE_SYSCALL(waitpid(-1, &status, 0));
    EXPECT_NE(pid, tracer_pid);
    EXPECT_TRUE(WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(PtraceTest, ExitKillFromThread) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([]() {
    // Test that the PtraceOExitKill works as expected.
    // Set ourselves as the subreaper.
    SAFE_SYSCALL(prctl(PR_SET_CHILD_SUBREAPER, 1));

    pid_t tgl_pid = SAFE_SYSCALL(fork());
    if (tgl_pid == 0) {
      // We are the thread-group leader. Create a thread that will be the ptracer.
      std::atomic<pid_t> tracee_pid;
      std::thread ptracer([&tracee_pid]() {
        pid_t pid = SAFE_SYSCALL(fork());
        if (pid == 0) {
          ASSERT_THAT(ptrace(PTRACE_TRACEME, 0, nullptr, nullptr), SyscallSucceeds());
          SAFE_SYSCALL(raise(SIGSTOP));
          _exit(EXIT_FAILURE);
        }
        tracee_pid.store(pid);

        int status;
        SAFE_SYSCALL(waitpid(pid, &status, 0));
        ASSERT_TRUE(WIFSTOPPED(status));
        EXPECT_THAT(ptrace(PTRACE_SETOPTIONS, pid, nullptr, PTRACE_O_EXITKILL), SyscallSucceeds());
      });

      ptracer.join();

      // Tracee should exit once the thread that spawned it exited.
      int status;
      SAFE_SYSCALL(waitpid(tracee_pid.load(), &status, 0));
      EXPECT_TRUE(WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
      _exit(EXIT_SUCCESS);
    }

    int status;
    SAFE_SYSCALL(waitpid(tgl_pid, &status, 0));
    EXPECT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 0);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

// Tests that a tracer thread group detaches from its running tracees upon normal exit (i.e. when
// the last thread in the tracer thread group exits).
TEST(PtraceTest, RunningTraceeDetachedOnNormalTracerExit) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([]() {
    test_helper::ForkHelper trace_fork_helper;
    trace_fork_helper.OnlyWaitForForkedChildren();

    test_helper::Rendezvous tracee_ready = test_helper::MakeRendezvous();

    // Create the tracee as its own process. The tracer thread group must only have one thread.
    pid_t tracee_pid = trace_fork_helper.RunInForkedProcess(
        [tracee_ready = std::move(tracee_ready.poker)]() mutable {
          // Allow a non-parent tracer.
          SAFE_SYSCALL(prctl(PR_SET_PTRACER, PR_SET_PTRACER_ANY, 0, 0, 0));
          tracee_ready.poke();
          // Wait for the parent to kill the tracee.
          while (true) {
            pause();
          }
        });

    pid_t tracer_pid = trace_fork_helper.RunInForkedProcess(
        [tracee_ready = std::move(tracee_ready.holder), tracee_pid]() mutable {
          // Attach to the tracee once it's ready.
          tracee_ready.hold();
          ASSERT_THAT(ptrace(PTRACE_ATTACH, tracee_pid, nullptr, nullptr), SyscallSucceeds());

          // Wait for the tracee to start and stop, then resume it so it is running when the tracer
          // exits.
          int status;
          SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
          ASSERT_TRUE(WIFSTOPPED(status));
          ASSERT_THAT(ptrace(PTRACE_CONT, tracee_pid, nullptr, 0), SyscallSucceeds());

          // Cause the tracer thread group to exit normally by exiting its last thread directly with
          // SYS_exit.
          SAFE_SYSCALL(syscall(SYS_exit, EXIT_SUCCESS));
        });

    // Wait for tracer to exit.
    int status;
    SAFE_SYSCALL(waitpid(tracer_pid, &status, 0));
    EXPECT_TRUE(WIFEXITED(status));
    EXPECT_EQ(WEXITSTATUS(status), EXIT_SUCCESS);

    // Verify that we (the parent) can attach to the tracee.
    // If the tracer did not detach, this will fail.
    ASSERT_THAT(ptrace(PTRACE_ATTACH, tracee_pid, nullptr, nullptr), SyscallSucceeds());

    // Detach and kill the tracee.
    SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
    EXPECT_TRUE(WIFSTOPPED(status));
    ASSERT_THAT(ptrace(PTRACE_DETACH, tracee_pid, nullptr, 0), SyscallSucceeds());
    SAFE_SYSCALL(kill(tracee_pid, SIGKILL));
    trace_fork_helper.ExpectSignal(SIGKILL);
    ASSERT_TRUE(trace_fork_helper.WaitForChild(tracee_pid).determined_result);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

// Tests that a tracer thread group detaches from its zombie tracees upon normal exit (i.e. when the
// last thread in the tracer thread group exits).
TEST(PtraceTest, ZombieTraceeDetachedOnNormalTracerExit) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  helper.RunInForkedProcess([]() {
    test_helper::ForkHelper trace_fork_helper;
    trace_fork_helper.OnlyWaitForForkedChildren();

    test_helper::Rendezvous tracee_ready = test_helper::MakeRendezvous();

    // Create the tracee as its own process. The tracer thread group must only have one thread.
    pid_t tracee_pid = trace_fork_helper.RunInForkedProcess(
        [tracee_ready = std::move(tracee_ready.poker)]() mutable {
          // Allow a non-parent tracer.
          SAFE_SYSCALL(prctl(PR_SET_PTRACER, PR_SET_PTRACER_ANY, 0, 0, 0));
          tracee_ready.poke();
          // Stop to wait for SIGKILL to make a zombie of this tracee.
          SAFE_SYSCALL(raise(SIGSTOP));
          _exit(EXIT_SUCCESS);
        });

    pid_t tracer_pid = trace_fork_helper.RunInForkedProcess(
        [tracee_ready = std::move(tracee_ready.holder), tracee_pid]() mutable {
          // Attach to the tracee once it's ready.
          tracee_ready.hold();
          ASSERT_THAT(ptrace(PTRACE_ATTACH, tracee_pid, nullptr, nullptr), SyscallSucceeds());

          // Wait for the tracee to start and stop, then kill it and wait for it to become a zombie.
          int status;
          SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
          ASSERT_TRUE(WIFSTOPPED(status));
          siginfo_t siginfo;
          SAFE_SYSCALL(kill(tracee_pid, SIGKILL));
          SAFE_SYSCALL(waitid(P_PID, tracee_pid, &siginfo, WEXITED | WNOWAIT));
          ASSERT_EQ(siginfo.si_code, CLD_KILLED);
          ASSERT_EQ(siginfo.si_status, SIGKILL);

          // Once the tracee is a zombie, cause the tracer thread group to exit normally by exiting
          // its last thread directly with SYS_exit.
          SAFE_SYSCALL(syscall(SYS_exit, EXIT_SUCCESS));
        });

    // Wait for the tracer to become a zombie.
    siginfo_t siginfo;
    SAFE_SYSCALL(waitid(P_PID, tracer_pid, &siginfo, WEXITED | WNOWAIT));
    ASSERT_EQ(siginfo.si_code, CLD_EXITED);
    ASSERT_EQ(siginfo.si_status, EXIT_SUCCESS);

    // Wait for the tracee to be waitable by us again because its tracer exited and detached.
    SAFE_SYSCALL(waitid(P_PID, tracee_pid, &siginfo, WEXITED | WNOWAIT));
    ASSERT_EQ(siginfo.si_code, CLD_KILLED);
    ASSERT_EQ(siginfo.si_status, SIGKILL);

    // Reap the tracer before the tracee to ensure they are fully disconnected.
    ASSERT_TRUE(trace_fork_helper.WaitForChild(tracer_pid).determined_result);
    trace_fork_helper.ExpectSignal(SIGKILL);
    ASSERT_TRUE(trace_fork_helper.WaitForChild(tracee_pid).determined_result);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(PtraceTest, PtraceAttachesToParentThread) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    SAFE_SYSCALL(prctl(PR_SET_CHILD_SUBREAPER, 1));
    std::latch fork_done(1);
    std::latch should_exit(1);
    std::atomic<pid_t> tracee_pid;

    std::thread ptracer([&tracee_pid, &fork_done, &should_exit]() {
      pid_t pid = SAFE_SYSCALL(fork());
      if (pid == 0) {
        ASSERT_THAT(ptrace(PTRACE_TRACEME, 0, nullptr, nullptr), SyscallSucceeds());
        // Can be controlled by the thread that spawned it.
        SAFE_SYSCALL(raise(SIGSTOP));

        // But no one else can make it continue.
        SAFE_SYSCALL(raise(SIGSTOP));
      }

      int status;
      SAFE_SYSCALL(waitpid(pid, &status, 0));
      ASSERT_TRUE(WIFSTOPPED(status));
      EXPECT_THAT(ptrace(PTRACE_SETOPTIONS, pid, nullptr, PTRACE_O_EXITKILL), SyscallSucceeds());
      EXPECT_THAT(ptrace(PTRACE_CONT, pid, nullptr, nullptr), SyscallSucceeds());

      tracee_pid.store(pid);
      fork_done.count_down();
      should_exit.wait();
    });

    fork_done.wait();

    std::thread another_thread([&tracee_pid]() {
      int status;
      SAFE_SYSCALL(waitpid(tracee_pid.load(), &status, 0));
      ASSERT_TRUE(WIFSTOPPED(status));
      EXPECT_THAT(ptrace(PTRACE_CONT, tracee_pid.load(), nullptr, nullptr),
                  SyscallFailsWithErrno(ESRCH));
    });
    another_thread.join();

    int status;
    // tracee is stopped, we know because of the waitpid in another_thread.
    EXPECT_THAT(ptrace(PTRACE_CONT, tracee_pid.load(), nullptr, nullptr),
                SyscallFailsWithErrno(ESRCH));

    should_exit.count_down();
    ptracer.join();

    SAFE_SYSCALL(waitpid(tracee_pid.load(), &status, 0));
    EXPECT_TRUE(WIFSIGNALED(status) && WTERMSIG(status) == SIGKILL);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

__attribute__((noinline)) void FunctionToBreak() {
  // Placeholder instruction to be replaced with a breakpoint by tracer.
  // Depending on the architecture, `nop` can be as small as 1 byte, so pad the function with
  // multiple `nop` instructions to ensure that the breakpoint does not overwrite the next
  // instruction.
  asm volatile(
      "nop\n"
      "nop\n"
      "nop\n"
      "nop\n"
      "nop\n"
      "nop\n"
      "nop\n"
      "nop\n");
}

// Sets a breakpoint in the child process using PTRACE_POKEDATA and expects that the child process
// triggers the breakpoint.
class SoftwareBreakpointTest : public ::testing::Test {
 public:
  void TearDown() override {
#if defined(__x86_64__)
    // On x86_64, PC is advanced after a breakpoint instruction, so the child should continue
    // execution and terminate successfully.
    helper_.ExpectSignal(0);
#else
    helper_.ExpectSignal(SIGTRAP);
#endif
    SAFE_SYSCALL(ptrace(PTRACE_DETACH, child_pid_, 0, 0));
    EXPECT_TRUE(helper_.WaitForChildren());

    ResetSignalReceived();
    RestoreSignalMask();
    RestoreSignalAction();
    ClosePipe();
  }

 protected:
  // Set a breakpoint at the given address in the process with ptrace POKEDATA.
  void SetBreakpointAndContinue() const {
    const void *breakpoint_addr = reinterpret_cast<void *>(&FunctionToBreak);
    errno = 0;
    long original_data = ptrace(PTRACE_PEEKDATA, child_pid_, breakpoint_addr, 0);
    if (original_data == -1 && errno != 0) {
      kill(child_pid_, SIGKILL);
      FAIL() << "PTRACE_PEEKDATA failed: " << strerror(errno) << "(" << errno << ")";
    }

    // Depending on the architecture and bitness, the breakpoint instruction could be smaller than
    // word length. Read original word, and overwrite the breakpoint instruction to it but keep the
    // rest as is.
#if defined(__x86_64__)
    const long break_insn = 0xCC;
    long breakpoint_data = (original_data & ~0xFFL) | break_insn;
#elif defined(__aarch64__)
    const long break_insn = 0xD4200000;
    long breakpoint_data = (original_data & ~0xFFFFFFFFL) | break_insn;
#elif defined(__arm__)
    const long break_insn = 0xE1200070;
    long breakpoint_data = (original_data & ~0xFFFFFFFFL) | break_insn;
#elif defined(__riscv)
    const long break_insn = 0x00100073;
    long breakpoint_data = (original_data & ~0xFFFFFFFFL) | break_insn;
#else
#error "Unsupported architecture"
#endif

    SAFE_SYSCALL(ptrace(PTRACE_POKEDATA, child_pid_, breakpoint_addr, breakpoint_data));
    SAFE_SYSCALL(ptrace(PTRACE_CONT, child_pid_, 0, 0));
  }

  void RunChildInForkedProcess() {
    child_pid_ = helper_.RunInForkedProcess([] {
      SAFE_SYSCALL(ptrace(PTRACE_TRACEME, 0, 0, 0));
      raise(SIGSTOP);
      FunctionToBreak();
    });
    ASSERT_NE(child_pid_, 0);
  }

  void WaitForChildStop(int expected_status) const {
    int status;
    ASSERT_EQ(SAFE_SYSCALL(waitpid(child_pid_, &status, 0)), child_pid_);
    ASSERT_TRUE(WIFSTOPPED(status));
    ASSERT_EQ(WSTOPSIG(status), expected_status);
  }

  void CheckSignalInfo(int signal, int code) const {
    siginfo_t info;
    SAFE_SYSCALL(ptrace(PTRACE_GETSIGINFO, child_pid_, nullptr, &info));
    EXPECT_EQ(info.si_signo, signal);
    EXPECT_EQ(info.si_code, code);
  }

  pid_t ChildPid() const { return child_pid_; }

  int CreateSignalFd(int signum) {
    sigset_t new_mask, old_mask;
    sigemptyset(&new_mask);
    sigaddset(&new_mask, signum);
    SAFE_SYSCALL(sigprocmask(SIG_BLOCK, &new_mask, &old_mask));
    old_sigmask_ = old_mask;

    return SAFE_SYSCALL(signalfd(-1, &new_mask, SFD_CLOEXEC));
  }

  void RestoreSignalMask() {
    if (old_sigmask_) {
      SAFE_SYSCALL(sigprocmask(SIG_SETMASK, &old_sigmask_.value(), nullptr));
      old_sigmask_.reset();
    }
  }

  // Create a pipe and set a signal handler for `signum` that writes a byte to the pipe and
  // records the signal code and status. The old signal handler is saved and restored by
  // `RestoreSignalAction`.
  void CreatePipeAndSetSignalAction(int signum) {
    ASSERT_TRUE(old_signum_and_sa_ == std::nullopt) << "Signal action already set";

    SAFE_SYSCALL(pipe2(pipefds_, O_CLOEXEC));
    pipe_write_fd_ = pipefds_[1];

    struct sigaction new_sa, old_sa;
    new_sa.sa_sigaction = [](int, siginfo_t *info, void *) {
      ASSERT_THAT(write(pipe_write_fd_, ".", 1), SyscallSucceedsWithValue(1));
      signal_code_ = info->si_code;
      signal_status_ = info->si_status;
    };
    new_sa.sa_flags = SA_SIGINFO | SA_RESTART;

    SAFE_SYSCALL(sigaction(signum, &new_sa, &old_sa));
    old_signum_and_sa_ = std::make_pair(signum, old_sa);
  }

  void RestoreSignalAction() {
    if (old_signum_and_sa_) {
      auto [signum, sa] = old_signum_and_sa_.value();
      SAFE_SYSCALL(sigaction(signum, &sa, nullptr));
      old_signum_and_sa_.reset();
    }
  }

  void ClosePipe() {
    if (pipefds_[0] != -1) {
      close(pipefds_[0]);
      pipefds_[0] = -1;
    }
    if (pipefds_[1] != -1) {
      close(pipefds_[1]);
      pipefds_[1] = -1;
      pipe_write_fd_ = -1;
    }
  }

  int pipe_read_fd() const { return pipefds_[0]; }

  static void ResetSignalReceived() {
    signal_code_ = 0;
    signal_status_ = 0;
  }

  // Static inline so that they can be used in signal handlers.
  static inline volatile sig_atomic_t pipe_write_fd_ = -1;
  static inline volatile sig_atomic_t signal_code_ = 0;
  static inline volatile sig_atomic_t signal_status_ = 0;

 private:
  test_helper::ForkHelper helper_;
  pid_t child_pid_;

  std::optional<sigset_t> old_sigmask_;
  std::optional<std::pair<int, struct sigaction>> old_signum_and_sa_;
  int pipefds_[2] = {-1, -1};
};

TEST_F(SoftwareBreakpointTest, Waitpid) {
  RunChildInForkedProcess();

  // Wait for initial SIGSTOP.
  WaitForChildStop(SIGSTOP);

  CheckSignalInfo(SIGSTOP, SI_TKILL);

  SetBreakpointAndContinue();

  // Wait for breakpoint.
  WaitForChildStop(SIGTRAP);

#if defined(__x86_64__)
  CheckSignalInfo(SIGTRAP, SI_KERNEL);
#else
  CheckSignalInfo(SIGTRAP, TRAP_BRKPT);
#endif
}

TEST_F(SoftwareBreakpointTest, SignalHandlerWithPipe) {
  CreatePipeAndSetSignalAction(SIGCHLD);

  RunChildInForkedProcess();

  // Wait for initial SIGSTOP.
  char c;
  ASSERT_THAT(read(pipe_read_fd(), &c, 1), SyscallSucceedsWithValue(1));
  EXPECT_EQ(c, '.');

  EXPECT_EQ(signal_code_, CLD_TRAPPED);
  EXPECT_EQ(signal_status_, SIGSTOP);

  CheckSignalInfo(SIGSTOP, SI_TKILL);

  ResetSignalReceived();

  SetBreakpointAndContinue();

  // Wait for breakpoint.
  ASSERT_THAT(read(pipe_read_fd(), &c, 1), SyscallSucceedsWithValue(1));
  EXPECT_EQ(c, '.');

  EXPECT_EQ(signal_code_, CLD_TRAPPED);
  EXPECT_EQ(signal_status_, SIGTRAP);

#if defined(__x86_64__)
  CheckSignalInfo(SIGTRAP, SI_KERNEL);
#else
  CheckSignalInfo(SIGTRAP, TRAP_BRKPT);
#endif
}

// Similar to SignalHandlerWithPipe, but waits on a ppoll of the pipe, instead of a blocking read.
// This emulates LLDB's main loop on Posix systems.
TEST_F(SoftwareBreakpointTest, SignalHandlerWithPoll) {
  CreatePipeAndSetSignalAction(SIGCHLD);

  RunChildInForkedProcess();

  // Wait for initial SIGSTOP.
  struct pollfd pfds[] = {{.fd = pipe_read_fd(), .events = POLLIN}};
  int ret = HANDLE_EINTR(ppoll(pfds, 1, nullptr, nullptr));
  ASSERT_EQ(ret, 1);

  char c;
  ASSERT_THAT(read(pipe_read_fd(), &c, 1), SyscallSucceedsWithValue(1));
  EXPECT_EQ(c, '.');

  EXPECT_EQ(signal_code_, CLD_TRAPPED);
  EXPECT_EQ(signal_status_, SIGSTOP);

  CheckSignalInfo(SIGSTOP, SI_TKILL);

  ResetSignalReceived();

  SetBreakpointAndContinue();

  // Wait for breakpoint.
  ret = HANDLE_EINTR(ppoll(pfds, 1, nullptr, nullptr));
  ASSERT_EQ(ret, 1);

  ASSERT_THAT(read(pipe_read_fd(), &c, 1), SyscallSucceedsWithValue(1));
  EXPECT_EQ(c, '.');

  EXPECT_EQ(signal_code_, CLD_TRAPPED);
  EXPECT_EQ(signal_status_, SIGTRAP);

#if defined(__x86_64__)
  CheckSignalInfo(SIGTRAP, SI_KERNEL);
#else
  CheckSignalInfo(SIGTRAP, TRAP_BRKPT);
#endif
}

TEST_F(SoftwareBreakpointTest, Signalfd) {
  int sfd = CreateSignalFd(SIGCHLD);

  RunChildInForkedProcess();

  // Expect initial SIGSTOP via signalfd
  struct signalfd_siginfo ssi;
  ASSERT_EQ(SAFE_SYSCALL(read(sfd, &ssi, sizeof(ssi))), static_cast<ssize_t>(sizeof(ssi)));
  EXPECT_EQ(ssi.ssi_signo, static_cast<uint32_t>(SIGCHLD));
  EXPECT_EQ(ssi.ssi_pid, static_cast<uint32_t>(ChildPid()));
  EXPECT_EQ(ssi.ssi_code, CLD_TRAPPED);
  EXPECT_EQ(ssi.ssi_status, SIGSTOP);

  WaitForChildStop(SIGSTOP);
  CheckSignalInfo(SIGSTOP, SI_TKILL);

  SetBreakpointAndContinue();

  // Expect breakpoint SIGCHLD via signalfd
  ASSERT_EQ(SAFE_SYSCALL(read(sfd, &ssi, sizeof(ssi))), static_cast<ssize_t>(sizeof(ssi)));
  EXPECT_EQ(ssi.ssi_signo, static_cast<uint32_t>(SIGCHLD));
  EXPECT_EQ(ssi.ssi_pid, static_cast<uint32_t>(ChildPid()));
  EXPECT_EQ(ssi.ssi_code, CLD_TRAPPED);
  EXPECT_EQ(ssi.ssi_status, SIGTRAP);

  WaitForChildStop(SIGTRAP);
#if defined(__x86_64__)
  CheckSignalInfo(SIGTRAP, SI_KERNEL);
#else
  CheckSignalInfo(SIGTRAP, TRAP_BRKPT);
#endif

  close(sfd);
}

// On ARM32 Linux, the specific instruction `0xe7f001f0` is used as a breakpoint, and should report
// SIGTRAP, instead of SIGILL. On other architectures, their architecture-specific breakpoint
// instruction is used instead (e.g. `0xcc` on x86_64, and `brk` on AArch64).
TEST(PtraceTest, UndefinedInstructionSignal) {
  // This test verifies that specific undefined instructions generate SIGTRAP
  // on ARM32 (even when not ptraced), and SIGILL on other architectures.

  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();

  pid_t child_pid = helper.RunInForkedProcess([] {
#if defined(__arm__)
    // Execute the specific undefined instruction used as breakpoint on ARM32
    asm volatile(".inst 0xe7f001f0");
#elif defined(__aarch64__)
    // Execute a generic undefined instruction on ARM64 (not a breakpoint)
    asm volatile(".inst 0x00000000");
#elif defined(__x86_64__)
    // Execute a generic undefined instruction on x86_64 (ud2)
    asm volatile("ud2");
#elif defined(__riscv)
    // Execute a generic undefined instruction on RISC-V (0x00000000 is always illegal)
    asm volatile(".word 0x00000000");
#else
    // Fallback or skip
    GTEST_SKIP() << "Unsupported architecture";
#endif
    _exit(1);  // Should not be reached
  });

  ASSERT_NE(child_pid, 0);

  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));

#if defined(__arm__)
  // We expect SIGTRAP for this specific instruction on ARM32
  EXPECT_TRUE(WIFSIGNALED(status) && WTERMSIG(status) == SIGTRAP)
      << "Expected SIGTRAP on ARM32, got " << WTERMSIG(status);
#else
  // We expect SIGILL on other architectures
  EXPECT_TRUE(WIFSIGNALED(status) && WTERMSIG(status) == SIGILL)
      << "Expected SIGILL, got " << WTERMSIG(status);
#endif
}

#if defined(__arm__)
__attribute__((noinline, target("thumb"))) void ExecuteThumbBreak1() {
  asm volatile(".inst.n 0xde01");
}

__attribute__((noinline, target("thumb"))) void ExecuteThumbBreak2() {
  asm volatile(".inst.w 0xf7f0a000");
}

TEST(PtraceTest, ThumbUndefinedInstructionSignal) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();

  // Test Thumb-1 breakpoint (0xde01)
  pid_t child_pid1 = helper.RunInForkedProcess([] {
    ExecuteThumbBreak1();
    _exit(1);
  });

  int status;
  ASSERT_EQ(child_pid1, waitpid(child_pid1, &status, 0));
  EXPECT_TRUE(WIFSIGNALED(status) && WTERMSIG(status) == SIGTRAP)
      << "Expected SIGTRAP for Thumb-1, got " << WTERMSIG(status);

  // Test Thumb-2 breakpoint (0xf7f0a000)
  pid_t child_pid2 = helper.RunInForkedProcess([] {
    ExecuteThumbBreak2();
    _exit(1);
  });

  ASSERT_EQ(child_pid2, waitpid(child_pid2, &status, 0));
  EXPECT_TRUE(WIFSIGNALED(status) && WTERMSIG(status) == SIGTRAP)
      << "Expected SIGTRAP for Thumb-2, got " << WTERMSIG(status);
}
#endif

// Create 2 memory mappings next to each other, and poke to a memory address that spans both
// mappings. Use file-backed mapping so that they are guaranteed to be distinct VMOs.
//
// |--------- reserved memory region ---------|
// |----- mapping1 -----||----- mapping2 -----|
//                     [word]
TEST(PtraceTest, PokeAcrossMappings) {
  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

  void *memory_region = mmap(nullptr, 2 * page_size, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
  ASSERT_NE(memory_region, MAP_FAILED) << strerror(errno);

  test_helper::ScopedTempFD tempfile1, tempfile2;
  SAFE_SYSCALL(ftruncate(tempfile1.fd(), page_size));
  SAFE_SYSCALL(ftruncate(tempfile2.fd(), page_size));

  void *mapping1 =
      mmap(memory_region, page_size, PROT_READ, MAP_PRIVATE | MAP_FIXED, tempfile1.fd(), 0);
  ASSERT_NE(mapping1, MAP_FAILED) << strerror(errno);

  uintptr_t mapping2_addr = reinterpret_cast<uintptr_t>(memory_region) + page_size;
  void *mapping2 = mmap(reinterpret_cast<void *>(mapping2_addr), page_size, PROT_READ,
                        MAP_PRIVATE | MAP_FIXED, tempfile2.fd(), 0);
  ASSERT_NE(mapping2, MAP_FAILED) << strerror(errno);

  // Set the poke address 1 byte before the start of the second mapping.
  void *poke_addr = reinterpret_cast<void *>(mapping2_addr - 1);

  test_helper::ForkHelper fork_helper;
  pid_t child_pid = fork_helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(ptrace(PTRACE_TRACEME, 0, 0, 0));
    raise(SIGSTOP);
    unsigned long value;
    memcpy(&value, poke_addr, sizeof(unsigned long));
    ASSERT_EQ(value, 0xBEEFUL);
    _exit(0);
  });
  ASSERT_NE(child_pid, 0);

  // Wait for child process to hit SIGSTOP.
  int status = 0;
  SAFE_SYSCALL(waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << std::hex << status;

  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid, poke_addr, 0xBEEFUL), SyscallSucceeds());

  SAFE_SYSCALL(ptrace(PTRACE_DETACH, child_pid, 0, 0));
  ASSERT_TRUE(fork_helper.WaitForChildren());
}

enum class BackingType {
  ANONYMOUS,
  MEMFD,
  READ_ONLY_FILE,
  WRITABLE_FILE,
};

std::string BackingTypeName(const testing::TestParamInfo<BackingType> &info) {
  switch (info.param) {
    case BackingType::ANONYMOUS:
      return "Anonymous";
    case BackingType::MEMFD:
      return "Memfd";
    case BackingType::READ_ONLY_FILE:
      return "ReadOnlyFile";
    case BackingType::WRITABLE_FILE:
      return "WritableFile";
  }
}

template <int map_flags, int prot_flags>
class PokeInMappingTest : public testing::TestWithParam<BackingType> {
 public:
  void SetUp() override {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
    len_ = 2 * page_size;
    int flags = map_flags;
    int fd = -1;
    switch (GetParam()) {
      case BackingType::ANONYMOUS:
        flags |= MAP_ANONYMOUS;
        break;
      case BackingType::MEMFD:
        fd = memfd_create("ptrace_test", 0);
        SAFE_SYSCALL(ftruncate(fd, len_));
        ASSERT_NE(fd, -1) << strerror(errno);
        break;
      case BackingType::READ_ONLY_FILE:
        fd = open("/proc/self/exe", O_RDONLY);
        ASSERT_NE(fd, -1) << strerror(errno);
        break;
      case BackingType::WRITABLE_FILE:
        fd = temp_file_.fd();
        ASSERT_NE(fd, -1) << "ScopedTempFD is -1";
        SAFE_SYSCALL(ftruncate(fd, len_));
        break;
    }
    mapping_ = mmap(nullptr, len_, prot_flags, flags, fd, 0);
    ASSERT_NE(mapping_, MAP_FAILED) << strerror(errno);

    if (fd != -1) {
      close(fd);
    }
  }

  void TearDown() override {
    SAFE_SYSCALL(ptrace(PTRACE_DETACH, child_pid_, 0, 0));
    ASSERT_TRUE(helper_.WaitForChildren());
  }

  // If `assert_memory_after_stop` is true, checks that the first byte of memory is modified by
  // ptrace_pokedata.
  void CreateChildProcess(bool assert_memory_after_stop) {
    helper_.OnlyWaitForForkedChildren();
    child_pid_ = helper_.RunInForkedProcess([&] {
      SAFE_SYSCALL(ptrace(PTRACE_TRACEME, 0, 0, 0));
      raise(SIGSTOP);
      if (assert_memory_after_stop) {
        SAFE_SYSCALL(mprotect(mapping_, len_, PROT_READ));
        ASSERT_EQ(static_cast<const volatile unsigned long *>(mapping_)[0], 0xBBUL);
      }
      _exit(0);
    });
    ASSERT_NE(child_pid_, 0);
  }

  // Wait for the child process to hit SIGSTOP.
  // After this call, the child process is stopped and ptrace is attached.
  void WaitForChildStop() {
    int status = 0;
    SAFE_SYSCALL(waitpid(child_pid_, &status, 0));
    ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << std::hex << status;
  }

 protected:
  test_helper::ForkHelper helper_;
  test_helper::ScopedTempFD temp_file_;
  pid_t child_pid_ = 0;
  void *mapping_ = nullptr;
  size_t len_ = 0;
};

// Poking in private memory should work, regardless of the backing and permissions.
using PokeInPrivateMappingTest = PokeInMappingTest<MAP_PRIVATE, PROT_NONE>;

TEST_P(PokeInPrivateMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

TEST_P(PokeInPrivateMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInPrivateMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::READ_ONLY_FILE, BackingType::WRITABLE_FILE),
                         &BackingTypeName);

using PokeInPrivateROMappingTest = PokeInMappingTest<MAP_PRIVATE, PROT_READ>;

TEST_P(PokeInPrivateROMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

TEST_P(PokeInPrivateROMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInPrivateROMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::READ_ONLY_FILE, BackingType::WRITABLE_FILE),
                         &BackingTypeName);

using PokeInPrivateRWMappingTest = PokeInMappingTest<MAP_PRIVATE, PROT_READ | PROT_WRITE>;

TEST_P(PokeInPrivateRWMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

TEST_P(PokeInPrivateRWMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInPrivateRWMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::READ_ONLY_FILE, BackingType::WRITABLE_FILE),
                         &BackingTypeName);

using PokeInPrivateRXMappingTest = PokeInMappingTest<MAP_PRIVATE, PROT_READ | PROT_EXEC>;

TEST_P(PokeInPrivateRXMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

TEST_P(PokeInPrivateRXMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInPrivateRXMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::READ_ONLY_FILE, BackingType::WRITABLE_FILE),
                         &BackingTypeName);

using PokeInPrivateRWXMappingTest =
    PokeInMappingTest<MAP_PRIVATE, PROT_READ | PROT_WRITE | PROT_EXEC>;

TEST_P(PokeInPrivateRWXMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

TEST_P(PokeInPrivateRWXMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallSucceeds());
}

INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInPrivateRWXMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::READ_ONLY_FILE, BackingType::WRITABLE_FILE),
                         &BackingTypeName);

// Poking in shared memory doesn't work, unless the process has writable permissions.
using PokeInSharedMappingTest = PokeInMappingTest<MAP_SHARED, PROT_NONE>;

TEST_P(PokeInSharedMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/false);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallFailsWithErrno(EIO));
}

TEST_P(PokeInSharedMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/false);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallFailsWithErrno(EIO));
}

INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInSharedMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::READ_ONLY_FILE, BackingType::WRITABLE_FILE),
                         &BackingTypeName);

using PokeInSharedROMappingTest = PokeInMappingTest<MAP_SHARED, PROT_READ>;

TEST_P(PokeInSharedROMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/false);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallFailsWithErrno(EIO));
}

TEST_P(PokeInSharedROMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/false);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallFailsWithErrno(EIO));
}

INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInSharedROMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::READ_ONLY_FILE, BackingType::WRITABLE_FILE),
                         &BackingTypeName);

using PokeInSharedRWMappingTest = PokeInMappingTest<MAP_SHARED, PROT_READ | PROT_WRITE>;

TEST_P(PokeInSharedRWMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallSucceeds());
  EXPECT_EQ(static_cast<const volatile unsigned long *>(mapping_)[0], 0xBBUL);
}

TEST_P(PokeInSharedRWMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallSucceeds());
  EXPECT_EQ(static_cast<const volatile unsigned long *>(mapping_)[0], 0xBBUL);
}

// Skip READ_ONLY_FILE because we cannot create writable memory from read-only file.
INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInSharedRWMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::WRITABLE_FILE),
                         &BackingTypeName);

using PokeInSharedRXMappingTest = PokeInMappingTest<MAP_SHARED, PROT_READ | PROT_EXEC>;

TEST_P(PokeInSharedRXMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/false);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallFailsWithErrno(EIO));
}

TEST_P(PokeInSharedRXMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/false);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallFailsWithErrno(EIO));
}

INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInSharedRXMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::READ_ONLY_FILE, BackingType::WRITABLE_FILE),
                         &BackingTypeName);

using PokeInSharedRWXMappingTest =
    PokeInMappingTest<MAP_SHARED, PROT_READ | PROT_WRITE | PROT_EXEC>;

TEST_P(PokeInSharedRWXMappingTest, Data) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping_, 0xBB), SyscallSucceeds());
  EXPECT_EQ(static_cast<const volatile unsigned long *>(mapping_)[0], 0xBBUL);
}

TEST_P(PokeInSharedRWXMappingTest, Text) {
  CreateChildProcess(/*assert_memory_after_stop=*/true);
  WaitForChildStop();
  EXPECT_THAT(ptrace(PTRACE_POKETEXT, child_pid_, mapping_, 0xBB), SyscallSucceeds());
  EXPECT_EQ(static_cast<const volatile unsigned long *>(mapping_)[0], 0xBBUL);
}

// Skip READ_ONLY_FILE because we cannot create writable memory from read-only file.
INSTANTIATE_TEST_SUITE_P(PtracePokeMemory, PokeInSharedRWXMappingTest,
                         testing::Values(BackingType::ANONYMOUS, BackingType::MEMFD,
                                         BackingType::WRITABLE_FILE),
                         &BackingTypeName);

class PokeInKernelMappingTest : public testing::Test {
 public:
  void SetUp() override {
    fork_helper_.OnlyWaitForForkedChildren();
    fork_helper_.ExpectSignal(SIGKILL);
    child_pid_ = fork_helper_.RunInForkedProcess([&] {
      SAFE_SYSCALL(ptrace(PTRACE_TRACEME, 0, 0, 0));
      raise(SIGSTOP);
      _exit(0);
    });
    ASSERT_NE(child_pid_, 0);

    int status = 0;
    SAFE_SYSCALL(waitpid(child_pid_, &status, 0));
    ASSERT_TRUE(WIFSTOPPED(status) && WSTOPSIG(status) == SIGSTOP) << std::hex << status;
  }

  void TearDown() override {
    SAFE_SYSCALL(kill(child_pid_, SIGKILL));
    EXPECT_TRUE(fork_helper_.WaitForChildren());
  }

  // Read child process'es mappings to find the region with given mapping_name.
  std::optional<test_helper::MemoryMapping> GetChildMapping(const std::string &mapping_name) const {
    std::string child_maps;
    if (!files::ReadFileToString("/proc/" + std::to_string(child_pid_) + "/maps", &child_maps)) {
      return std::nullopt;
    }

    return test_helper::find_memory_mapping(
        [&](const test_helper::MemoryMapping &mapping) { return mapping.pathname == mapping_name; },
        child_maps);
  }

 protected:
  test_helper::ForkHelper fork_helper_;
  pid_t child_pid_;
};

TEST_F(PokeInKernelMappingTest, VDSO) {
  auto mapping = GetChildMapping("[vdso]");
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping->start, 0xBB), SyscallSucceeds());
}

TEST_F(PokeInKernelMappingTest, VVAR) {
  auto mapping = GetChildMapping("[vvar]");
  EXPECT_THAT(ptrace(PTRACE_POKEDATA, child_pid_, mapping->start, 0xBB),
              SyscallFailsWithErrno(EIO));
}

TEST(PtraceTest, MaskedSignalDelivery) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  pid_t pid = helper.RunInForkedProcess([] {
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGSTOP);  // SIGSTOP cannot be blocked.
    sigaddset(&mask, SIGTRAP);  // SIGTRAP can be blocked.
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &mask, nullptr), SyscallSucceeds());

    SAFE_SYSCALL(ptrace(PTRACE_TRACEME, 0, nullptr, nullptr));
    // Signal with SIGSTOP, expecting that it is not blocked.
    // This should stop this process.
    raise(SIGSTOP);

    // Signal with SIGTRAP, expecting that it is blocked.
    // This should not stop this process, and should remain in the signal queue.
    raise(SIGTRAP);

    sigset_t pending;
    sigemptyset(&pending);
    SAFE_SYSCALL(sigpending(&pending));
    ASSERT_FALSE(sigismember(&pending, SIGSTOP));
    ASSERT_TRUE(sigismember(&pending, SIGTRAP));
  });

  int status;
  ASSERT_EQ(waitpid(pid, &status, 0), pid);
  EXPECT_TRUE(WIFSTOPPED(status)) << "status: " << std::hex << status;
  EXPECT_EQ(WSTOPSIG(status), SIGSTOP) << "status: " << std::hex << status;

  SAFE_SYSCALL(ptrace(PTRACE_CONT, pid, nullptr, nullptr));

  // Child should exit, i.e. not stop at SIGTRAP.
  ASSERT_EQ(waitpid(pid, &status, 0), pid);
  ASSERT_TRUE(WIFEXITED(status));
  EXPECT_EQ(WEXITSTATUS(status), 0);
}

TEST(PtraceTest, PtraceEventStopWithMaskedSigtrap) {
  // Pipe for synchronization, so that the parent can seize the child.
  int pipefds[2];
  ASSERT_THAT(pipe2(pipefds, O_CLOEXEC), SyscallSucceeds());
  fbl::unique_fd pipe_read(pipefds[0]);
  fbl::unique_fd pipe_write(pipefds[1]);

  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  pid_t child_pid = helper.RunInForkedProcess([&] {
    pipe_write.reset();

    // Block SIGTRAP signal.
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGTRAP);
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &mask, nullptr), SyscallSucceeds());

    // Wait for parent to seize this process.
    char c;
    ASSERT_THAT(read(pipe_read.get(), &c, 1), SyscallSucceedsWithValue(1));

    // Fork a grandchild process.
    pid_t grandchild_pid = fork();
    ASSERT_GE(grandchild_pid, 0);

    // Exit immediately. The tracer (test process) will reap the grandchild.
    _exit(0);
  });

  pipe_read.reset();
  ASSERT_THAT(ptrace(PTRACE_SEIZE, child_pid, 0, PTRACE_O_TRACEFORK), SyscallSucceeds());
  ASSERT_THAT(write(pipe_write.get(), "a", 1), SyscallSucceedsWithValue(1));

  int status;
  ASSERT_EQ(HANDLE_EINTR(waitpid(child_pid, &status, 0)), child_pid);
  EXPECT_TRUE(WIFSTOPPED(status)) << "status: " << std::hex << status;
  EXPECT_EQ(status >> 8, (SIGTRAP | (PTRACE_EVENT_FORK << 8))) << "status: " << std::hex << status;

  pid_t grandchild_pid = 0;
  ASSERT_THAT(get_event_msg<pid_t>(child_pid, &grandchild_pid), SyscallSucceeds());
  ASSERT_GT(grandchild_pid, 0);

  ASSERT_EQ(HANDLE_EINTR(waitpid(grandchild_pid, &status, 0)), grandchild_pid);
  EXPECT_TRUE(WIFSTOPPED(status)) << "status: " << std::hex << status;
  EXPECT_EQ(status >> 8, (SIGTRAP | (PTRACE_EVENT_STOP << 8))) << "status: " << std::hex << status;

  ASSERT_THAT(ptrace(PTRACE_CONT, child_pid, 0, 0), SyscallSucceeds());
  ASSERT_THAT(ptrace(PTRACE_CONT, grandchild_pid, 0, 0), SyscallSucceeds());

  ASSERT_EQ(HANDLE_EINTR(waitpid(child_pid, &status, 0)), child_pid);
  EXPECT_TRUE(WIFEXITED(status)) << "Child exit status: " << std::hex << status;
  EXPECT_EQ(WEXITSTATUS(status), 0);

  ASSERT_EQ(HANDLE_EINTR(waitpid(grandchild_pid, &status, 0)), grandchild_pid);
  EXPECT_TRUE(WIFEXITED(status)) << "Grandchild exit status: " << std::hex << status;
  EXPECT_EQ(WEXITSTATUS(status), 0);

  ptrace(PTRACE_DETACH, grandchild_pid, 0, 0);
  ptrace(PTRACE_DETACH, child_pid, 0, 0);
}

TEST(PtraceTest, PtraceAttachesDuringPpoll) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  pid_t child_pid = helper.RunInForkedProcess([&] {
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &mask, nullptr), SyscallSucceeds());

    // Handle SIGUSR1.
    static volatile sig_atomic_t sigusr1_received = 0;
    struct sigaction sa = {};
    sa.sa_handler = [](int) { sigusr1_received = 1; };
    sigaction(SIGUSR1, &sa, nullptr);

    // Block on ppoll.
    sigset_t ppoll_mask;
    sigemptyset(&ppoll_mask);
    ASSERT_THAT(ppoll(nullptr, 0, nullptr, &ppoll_mask), SyscallFailsWithErrno(EINTR));

    EXPECT_TRUE(sigusr1_received);
  });
  ASSERT_NE(child_pid, 0);

  ASSERT_TRUE(test_helper::WaitUntilBlocked(child_pid, true));

  ASSERT_THAT(ptrace(PTRACE_ATTACH, child_pid, 0, 0), SyscallSucceeds());
  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status)) << "status: " << std::hex << status;
  EXPECT_EQ(WSTOPSIG(status), SIGSTOP);

  ASSERT_THAT(ptrace(PTRACE_DETACH, child_pid, 0, 0), SyscallSucceeds());

  // Check that child should go back to being blocked on ppoll.
  ASSERT_EQ(0, waitpid(child_pid, &status, WNOHANG));

  // Allow the child to return from ppoll and exit.
  kill(child_pid, SIGUSR1);
}

TEST(PtraceTest, PtraceAttachesDuringPpollAndSignal) {
  test_helper::ForkHelper helper;
  helper.OnlyWaitForForkedChildren();
  pid_t child_pid = helper.RunInForkedProcess([&] {
    sigset_t mask;
    sigemptyset(&mask);
    sigaddset(&mask, SIGUSR1);
    ASSERT_THAT(sigprocmask(SIG_BLOCK, &mask, nullptr), SyscallSucceeds());

    // Handle SIGUSR1.
    static volatile sig_atomic_t sigusr1_received = 0;
    struct sigaction sa = {};
    sa.sa_handler = [](int) { sigusr1_received = 1; };
    sigaction(SIGUSR1, &sa, nullptr);

    // Block on ppoll.
    sigset_t ppoll_mask;
    sigemptyset(&ppoll_mask);
    struct timespec timeout = {.tv_sec = 2, .tv_nsec = 0};

    // While blocked on the first ppoll, the parent will ptrace-attach, and send a SIGUSR1 signal.
    // The parent will intercept this signal and suppress it, so that the signal is not delivered.
    // On Linux, this happens transparently and ppoll returns success after 2 seconds, but on
    // Starnix it returns EINTR after the signal is suppressed.
    // TODO(wintermelons): Fix this so that ppoll returns success.
    ppoll(nullptr, 0, &timeout, &ppoll_mask);

    // Block on ppoll again. By doing 2 ppolls in quick succession, this occasionally triggers a
    // race condition where the signal mask is not updated correctly.
    ASSERT_THAT(ppoll(nullptr, 0, &timeout, &ppoll_mask), SyscallSucceeds());

    // Check if temporary signal mask leaked
    sigset_t current_mask;
    ASSERT_THAT(sigprocmask(SIG_BLOCK, nullptr, &current_mask), SyscallSucceeds());
    EXPECT_TRUE(sigismember(&current_mask, SIGUSR1));

    EXPECT_FALSE(sigusr1_received);
  });
  ASSERT_NE(child_pid, 0);

  ASSERT_TRUE(test_helper::WaitUntilBlocked(child_pid, true));

  ASSERT_THAT(ptrace(PTRACE_ATTACH, child_pid, 0, 0), SyscallSucceeds());
  int status;
  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status)) << "status: " << std::hex << status;
  EXPECT_EQ(WSTOPSIG(status), SIGSTOP);

  // Let child go back to blocking on ppoll.
  ASSERT_THAT(ptrace(PTRACE_CONT, child_pid, 0, 0), SyscallSucceeds());
  ASSERT_TRUE(test_helper::WaitUntilBlocked(child_pid, true));

  // Now that child is in ppoll and is ptraced, send a signal and check that the child goes into
  // signal-delivery-stop.
  kill(child_pid, SIGUSR1);

  ASSERT_EQ(child_pid, waitpid(child_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status)) << "status: " << std::hex << status;
  EXPECT_EQ(WSTOPSIG(status), SIGUSR1);

  // Continue the child without delivering the captured signal.
  ASSERT_THAT(ptrace(PTRACE_CONT, child_pid, 0, 0), SyscallSucceeds());
}

TEST(PtraceTest, AttachDeniedWithoutCapSysPtrace) {
  if (!test_helper::HasCapabilityPermitted(CAP_SYS_PTRACE)) {
    GTEST_SKIP() << "Needs the CAP_SYS_PTRACE capability.";
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    pid_t parent_pid = getppid();

    // Drop all capabilities to ensure the child is less privileged than the parent.
    test_helper::DropAllCapabilities();

    // Verify that a less privileged process cannot attach without CAP_SYS_PTRACE.
    EXPECT_THAT(ptrace(PTRACE_ATTACH, parent_pid, nullptr, nullptr), SyscallFailsWithErrno(EPERM));
  });
}

TEST(PtraceTest, AttachAllowedWithCapSysPtrace) {
  if (!test_helper::HasCapabilityPermitted(CAP_SYS_PTRACE) ||
      !test_helper::HasCapabilityPermitted(CAP_SYSLOG)) {
    GTEST_SKIP() << "Needs CAP_SYS_PTRACE and CAP_SYSLOG capabilities.";
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    pid_t parent_pid = getppid();

    // Drop a specific capability to make child's caps a non-superset of parent's.
    test_helper::UnsetCapabilityEffective(CAP_SYSLOG);
    test_helper::UnsetCapabilityPermitted(CAP_SYSLOG);

    // Verify that CAP_SYS_PTRACE allows attaching to a process with non-superset capabilities.
    ASSERT_THAT(ptrace(PTRACE_ATTACH, parent_pid, nullptr, nullptr), SyscallSucceeds());

    // Clean up: detach so parent can continue.
    int status;
    ASSERT_EQ(parent_pid, waitpid(parent_pid, &status, 0));
    ASSERT_TRUE(WIFSTOPPED(status));

    ASSERT_THAT(ptrace(PTRACE_DETACH, parent_pid, nullptr, nullptr), SyscallSucceeds());
  });
}

class PtraceThreadTracerTest : public ::testing::Test {
 protected:
  pid_t tracee_pid() const { return tracee_pid_; }
  test_helper::ForkHelper &helper() { return helper_; }

  // Starts the child tracee process. It sets up PR_SET_PTRACER, signals that the tracee is ready,
  // waits for the exit signal on `tracee_exit`, and exits.
  void StartTracee() {
    ASSERT_EQ(tracee_pid_, 0);
    tracee_pid_ =
        helper_.RunInForkedProcess([tracee_ready = std::move(tracee_ready_.poker),
                                    tracee_exit = std::move(tracee_exit_.holder), this]() mutable {
          // Close child's inherited write-end so closing the parent's poker delivers POLLHUP.
          tracee_exit_.poker = {};
          SAFE_SYSCALL(prctl(PR_SET_PTRACER, PR_SET_PTRACER_ANY, 0, 0, 0));
          tracee_ready.poke();
          tracee_exit.hold();
          _exit(0);
        });
    tracee_ready_.holder.hold();
  }

  // Starts the tracer thread. It attaches to the tracee, optionally waits for the initial SIGSTOP
  // stop notification if `wait_for_stop` is true, and then runs `action(tracee_pid_)`.
  void StartTracerThread(fit::function<void(pid_t tracee_pid)> action, bool wait_for_stop = true) {
    ASSERT_FALSE(tracer_thread_.has_value());
    ASSERT_TRUE(action);
    tracer_thread_.emplace([this, action = std::move(action), wait_for_stop]() {
      ASSERT_THAT(ptrace(PTRACE_ATTACH, tracee_pid_, nullptr, nullptr), SyscallSucceeds());
      if (wait_for_stop) {
        int status;
        SAFE_SYSCALL(waitpid(tracee_pid_, &status, 0));
        ASSERT_TRUE(WIFSTOPPED(status));
        ASSERT_EQ(WSTOPSIG(status), SIGSTOP);
      }

      action(tracee_pid_);
    });
  }

  // Joins the tracer thread if running.
  void JoinTracerThread() {
    if (tracer_thread_) {
      tracer_thread_->join();
      tracer_thread_.reset();
    }
  }

  // Signals the tracee that it may exit and verifies all children.
  void FinishAndVerifyTracee() {
    JoinTracerThread();
    tracee_exit_.poker.poke();
    EXPECT_TRUE(helper_.WaitForChildren());
  }

  // Similar to `FinishAndVerifyTracee()` but releases the exit barrier without writing to the
  // rendezvous pipe and verifies all children.
  //
  // In tests where the tracee is expected to be terminated by the kernel (e.g. via SIGKILL),
  // the tracee's read end of the pipe is closed upon death. Calling poke() in that case would
  // fail with EPIPE (Broken pipe) on Linux. Resetting the poker closes the write end instead,
  // which avoids EPIPE while still sending POLLHUP to allow any surviving tracee to unblock
  // cleanly without hanging.
  void ReleaseAndVerifyTracee() {
    JoinTracerThread();
    tracee_exit_.poker = {};
    EXPECT_TRUE(helper_.WaitForChildren());
  }

  void TearDown() override { JoinTracerThread(); }

 private:
  test_helper::ForkHelper helper_;
  test_helper::Rendezvous tracee_ready_ = test_helper::MakeRendezvous();
  test_helper::Rendezvous tracee_exit_ = test_helper::MakeRendezvous();
  pid_t tracee_pid_ = 0;
  std::optional<std::thread> tracer_thread_;
};

TEST_F(PtraceThreadTracerTest, SiblingThreadCannotIssuePtraceCommands) {
  StartTracee();

  std::latch attached_and_stopped(1);
  std::latch sibling_done(1);

  StartTracerThread([&](pid_t pid) {
    // Notify the sibling thread that the tracee is attached and stopped.
    attached_and_stopped.count_down();

    // Wait for the sibling thread to attempt its ptrace command.
    sibling_done.wait();

    // Clean up the tracee by detaching it so it resumes.
    ASSERT_THAT(ptrace(PTRACE_DETACH, pid, nullptr, 0), SyscallSucceeds());
  });

  attached_and_stopped.wait();

  // In Linux, the tracer is a specific thread (task), not the whole thread group.
  // A sibling thread in the tracer thread group cannot issue ptrace commands to the tracee.
  EXPECT_THAT(ptrace(PTRACE_SETOPTIONS, tracee_pid(), nullptr, PTRACE_O_TRACESYSGOOD),
              SyscallFailsWithErrno(ESRCH));

  sibling_done.count_down();
  FinishAndVerifyTracee();
}

TEST_F(PtraceThreadTracerTest, SiblingThreadCanWaitOnTracee) {
  StartTracee();

  std::latch attached(1);
  std::latch waited(1);

  // The tracer thread only attaches (does not waitpid), allowing the sibling thread to wait on it.
  StartTracerThread(
      [&](pid_t pid) {
        attached.count_down();
        waited.wait();
        ASSERT_THAT(ptrace(PTRACE_DETACH, pid, nullptr, 0), SyscallSucceeds());
      },
      /*wait_for_stop=*/false);

  attached.wait();

  // In Linux, any thread in the tracer's thread group can wait on and reap ptrace
  // stop notifications for the tracee via waitpid().
  int status = 0;
  EXPECT_EQ(SAFE_SYSCALL(waitpid(tracee_pid(), &status, WUNTRACED)), tracee_pid());
  EXPECT_TRUE(WIFSTOPPED(status));
  EXPECT_EQ(WSTOPSIG(status), SIGSTOP);

  waited.count_down();
  FinishAndVerifyTracee();
}

TEST_F(PtraceThreadTracerTest, TraceeDetachesWhenTracerThreadExits) {
  StartTracee();

  // Tracer thread attaches and immediately exits.
  StartTracerThread([](pid_t) {});

  JoinTracerThread();

  // In Linux, when the tracer thread exits, the tracee is automatically detached.
  // Therefore, issuing PTRACE_DETACH to the detached tracee fails with ESRCH.
  // On Starnix, the tracee was not detached, so this PTRACE_DETACH succeeds (failing the
  // assertion as expected) and detaches the tracee so it can exit cleanly.
  EXPECT_THAT(ptrace(PTRACE_DETACH, tracee_pid(), nullptr, 0), SyscallFailsWithErrno(ESRCH));

  FinishAndVerifyTracee();
}

TEST_F(PtraceThreadTracerTest, TraceeKilledWhenTracerThreadExitsWithExitKill) {
  StartTracee();

  StartTracerThread([](pid_t pid) {
    // Set PTRACE_O_EXITKILL on the tracee.
    ASSERT_THAT(ptrace(PTRACE_SETOPTIONS, pid, nullptr, PTRACE_O_EXITKILL), SyscallSucceeds());
    // Tracer thread exits here.
  });

  JoinTracerThread();

  // In Linux, when a tracer thread with PTRACE_O_EXITKILL exits, the tracee is sent SIGKILL.
  // Therefore, issuing PTRACE_DETACH to the dead tracee fails with ESRCH.
  // On Starnix, the tracee was not killed, so this PTRACE_DETACH succeeds (failing the
  // assertion as expected) and detaches the tracee so it can exit cleanly.
  EXPECT_THAT(ptrace(PTRACE_DETACH, tracee_pid(), nullptr, 0), SyscallFailsWithErrno(ESRCH));
  helper().ExpectSignal(SIGKILL);

  ReleaseAndVerifyTracee();
}

TEST(PtraceTest, SigcontDoesNotWakePtraceEventStop) {
  test_helper::ForkHelper helper;

  pid_t tracee_pid = helper.RunInForkedProcess([]() {
    ASSERT_THAT(ptrace(PTRACE_TRACEME, 0, nullptr, nullptr), SyscallSucceeds());
    SAFE_SYSCALL(raise(SIGSTOP));
    _exit(42);
  });

  // Wait for the initial SIGSTOP.
  int status;
  SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status));
  ASSERT_EQ(WSTOPSIG(status), SIGSTOP);

  // Enable PTRACE_O_TRACEEXIT and resume tracee.
  ASSERT_THAT(ptrace(PTRACE_SETOPTIONS, tracee_pid, nullptr, PTRACE_O_TRACEEXIT),
              SyscallSucceeds());
  ASSERT_THAT(ptrace(PTRACE_CONT, tracee_pid, nullptr, 0), SyscallSucceeds());

  // Wait for the PTRACE_EVENT_EXIT stop.
  SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status));
  ASSERT_EQ(status >> 8, (SIGTRAP | (PTRACE_EVENT_EXIT << 8)));

  // Send SIGCONT to the tracee while it is stopped in PTRACE_EVENT_EXIT.
  SAFE_SYSCALL(kill(tracee_pid, SIGCONT));

  // In Linux, SIGCONT has no effect on a ptrace event stop.
  // The tracee must remain stopped in PTRACE_EVENT_EXIT and must NOT exit yet.
  int check_status;
  EXPECT_EQ(waitpid(tracee_pid, &check_status, WNOHANG), 0);

  // Resume the tracee properly with PTRACE_CONT.
  ASSERT_THAT(ptrace(PTRACE_CONT, tracee_pid, nullptr, 0), SyscallSucceeds());

  // Now wait for the tracee to actually exit with code 42.
  SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
  EXPECT_TRUE(WIFEXITED(status) && WEXITSTATUS(status) == 42);

  helper.ExpectExitValue(42);
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(PtraceTest, SyscallExecutesSideEffectBeforeSigstop) {
  test_helper::ForkHelper helper;
  pid_t tracee_pid = helper.RunInForkedProcess([]() {
    ASSERT_THAT(ptrace(PTRACE_TRACEME, 0, nullptr, nullptr), SyscallSucceeds());
    SAFE_SYSCALL(raise(SIGSTOP));

    // Execute setpgid syscall with an observable side effect.
    SAFE_SYSCALL(syscall(__NR_setpgid, 0, 0));
    _exit(0);
  });

  // Wait for initial raise(SIGSTOP).
  int status;
  SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status));
  ASSERT_EQ(WSTOPSIG(status), SIGSTOP);

  // Enable PTRACE_O_TRACESYSGOOD to distinguish syscall stops.
  ASSERT_THAT(ptrace(PTRACE_SETOPTIONS, tracee_pid, nullptr, PTRACE_O_TRACESYSGOOD),
              SyscallSucceeds());

  // Step tracee to the syscall-enter stop for __NR_setpgid.
  ASSERT_NO_FATAL_FAILURE(StepToSyscallEntry(tracee_pid, {__NR_setpgid}));

  // At syscall-enter stop, the setpgid syscall has not executed yet.
  // The tracee's PGID should still be the parent's PGID.
  pid_t parent_pgid = SAFE_SYSCALL(getpgid(0));
  EXPECT_EQ(SAFE_SYSCALL(getpgid(tracee_pid)), parent_pgid);
  EXPECT_NE(parent_pgid, tracee_pid);

  // Send SIGSTOP to the tracee while it is stopped in syscall-enter-stop.
  SAFE_SYSCALL(kill(tracee_pid, SIGSTOP));

  // Resume the tracee with PTRACE_CONT.
  ASSERT_THAT(ptrace(PTRACE_CONT, tracee_pid, nullptr, 0), SyscallSucceeds());

  // In Linux, the tracee executes the syscall body (changing its PGID to its own PID),
  // then reaches syscall exit where the pending SIGSTOP is intercepted in a signal-delivery-stop.
  SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status));
  EXPECT_EQ(WSTOPSIG(status), SIGSTOP);

  // Verify that the syscall's side effect DID happen: PGID is now tracee_pid.
  EXPECT_EQ(SAFE_SYSCALL(getpgid(tracee_pid)), tracee_pid);

  // Resume tracee from signal-delivery-stop, delivering the SIGSTOP so it enters group-stop.
  ASSERT_THAT(ptrace(PTRACE_CONT, tracee_pid, nullptr, SIGSTOP), SyscallSucceeds());
  SAFE_SYSCALL(waitpid(tracee_pid, &status, 0));
  ASSERT_TRUE(WIFSTOPPED(status));
  EXPECT_EQ(WSTOPSIG(status), SIGSTOP);

  // Detach tracee and wake it from group-stop with SIGCONT so it exits cleanly.
  ASSERT_THAT(ptrace(PTRACE_DETACH, tracee_pid, nullptr, 0), SyscallSucceeds());
  SAFE_SYSCALL(kill(tracee_pid, SIGCONT));

  EXPECT_TRUE(helper.WaitForChildren());
}

}  // namespace
