// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/starnix/tests/syscalls/cpp/task_test.h"

#include <fcntl.h>
#include <grp.h>
#include <limits.h>
#include <sched.h>
#include <strings.h>
#include <sys/fsuid.h>
#include <sys/mman.h>
#include <sys/prctl.h>
#include <sys/select.h>
#include <sys/syscall.h>
#include <sys/time.h>
#include <sys/types.h>
#include <sys/wait.h>
#include <unistd.h>

#include <cerrno>
#include <cstdint>
#include <filesystem>
#include <thread>

#include <fbl/algorithm.h>
#include <gtest/gtest.h>
#include <linux/capability.h>
#include <linux/sched.h>
#include <linux/securebits.h>

#include "src/lib/files/directory.h"
#include "src/lib/files/file.h"
#include "src/lib/files/path.h"
#include "src/lib/fxl/strings/split_string.h"
#include "src/lib/fxl/strings/string_number_conversions.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/starnix/tests/syscalls/cpp/capabilities_helper.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

namespace {

#define MAX_PAGE_ALIGNMENT (1 << 21)

// Unless the split between the data and bss sections happens to be page-aligned, initial part
// of the bss section will be in the same page as the last part of the data section.
// By aligning g_global_variable_bss to a value bigger than the page size, we prevent it from
// ending up in this shared page
alignas(MAX_PAGE_ALIGNMENT) volatile int g_global_variable_bss = 0;

volatile size_t g_global_variable_data = 15;
volatile size_t g_fork_doesnt_drop_writes = 0;

// As of this writing, our sysroot's syscall.h lacks the SYS_clone3 definition.
#ifndef SYS_clone3
#if defined(__aarch64__) || defined(__x86_64__) || defined(__riscv)
#define SYS_clone3 435
#else
#error SYS_clone3 needs a definition for this architecture.
#endif
#endif

constexpr int kChildExpectedExitCode = 21;
constexpr int kChildErrorExitCode = kChildExpectedExitCode + 1;

constexpr uid_t kUser1Uid = 65533;
constexpr gid_t kUser1Gid = 65534;

bool change_ids(uid_t user, gid_t group) {
  return (setresgid(group, group, group) == 0) && (setresuid(user, user, user) == 0);
}

void WaitForReadFd(int fd) {
  fd_set read_fds;
  FD_ZERO(&read_fds);
  FD_SET(fd, &read_fds);

  SAFE_SYSCALL(TEMP_FAILURE_RETRY(select(fd + 1, &read_fds, nullptr, nullptr, nullptr)));
  EXPECT_TRUE(FD_ISSET(fd, &read_fds));
}

pid_t ForkUsingClone3(const clone_args* cl_args, size_t size) {
  return static_cast<pid_t>(syscall(SYS_clone3, cl_args, size));
}

// calls clone3 and executes a function, calling exit with its return value.
pid_t DoClone3(const clone_args* cl_args, size_t size, int (*func)(void*), void* param) {
  pid_t pid;
  // clone3 lets you specify a new stack, but not which function to run.
  // This means that after the clone3 syscall, the child will be running on a
  // new stack, not being able to access any local variables from before the
  // clone.
  //
  // We have to manually call into the new function in assembly, being careful
  // to not refer to any variables from the stack.
#if defined(__aarch64__)
  __asm__ volatile(
      "mov x0, %[cl_args]\n"
      "mov x1, %[size]\n"
      "mov w8, %[clone3]\n"
      "svc #0\n"
      "cbnz x0, 1f\n"
      "mov x0, %[param]\n"
      "blr %[func]\n"
      "mov w8, %[exit]\n"
      "svc #0\n"
      "brk #1\n"
      "1:\n"
      "mov %w[pid], w0\n"
      : [pid] "=r"(pid)
      : [cl_args] "r"(cl_args), "m"(*cl_args), [size] "r"(size), [func] "r"(func),
        [param] "r"(param), [clone3] "i"(SYS_clone3), [exit] "i"(SYS_exit)
      : "x0", "x1", "x2", "x3", "x4", "x5", "x6", "x7", "x8", "x9", "x10", "x11", "x12", "x13",
        "x14", "x15", "x16", "x17", "cc", "memory");
#elif defined(__arm__)
  __asm__ volatile(
      "mov r0, %[cl_args]\n"
      "mov r1, %[size]\n"
      "mov r7, %[clone3]\n"
      "svc #0\n"
      "cmp r0, #0\n"
      "bne 1f\n"
      "mov r0, %[param]\n"
      "blx %[func]\n"
      "mov r7, %[exit]\n"
      "svc #0\n"
      "bkpt\n"
      "1:\n"
      "mov %[pid], r0\n"
      : [pid] "=r"(pid)
      : [cl_args] "r"(cl_args), "m"(*cl_args), [size] "r"(size), [func] "r"(func),
        [param] "r"(param), [clone3] "i"(SYS_clone3), [exit] "i"(SYS_exit)
      : "r0", "r1", "r2", "r3", "r7", "r12", "lr", "cc", "memory");
#elif defined(__x86_64__)
  __asm__ volatile(
      "syscall\n"
      "test %%rax, %%rax\n"
      "jnz 1f\n"
      "movq %[param], %%rdi\n"
      "callq *%[func]\n"
      "movl %%eax, %%edi\n"
      "movl %[exit], %%eax\n"
      "syscall\n"
      "ud2\n"
      "1:\n"
      "movl %%eax, %[pid]\n"
      : [pid] "=g"(pid)
      : "D"(cl_args), "m"(*cl_args), "S"(size),
        "a"(SYS_clone3), [func] "r"(func), [param] "r"(param), [exit] "i"(SYS_exit)
      : "rcx", "rdx", "r8", "r9", "r10", "r11", "cc", "memory");
#elif defined(__riscv)
  __asm__ volatile(
      "mv   a0, %[cl_args]\n"
      "mv   a1, %[size]\n"
      "li   a7, %[sys_clone3]\n"
      "ecall\n"
      "bnez a0, 1f\n"
      "mv   a0, %[param]\n"
      "jalr %[func]\n"
      "li   a7, %[sys_exit]\n"
      "ecall\n"
      "ebreak\n"
      "1:\n"
      "mv   %[pid], a0\n"
      : [pid] "=r"(pid)
      : [cl_args] "r"(cl_args), "m"(*cl_args), [size] "r"(size), [func] "r"(func),
        [param] "r"(param), [sys_clone3] "i"(SYS_clone3), [sys_exit] "i"(SYS_exit)
      : "ra", "t0", "t1", "t2", "t3", "t4", "t5", "t6", "a0", "a1", "a2", "a3", "a4", "a5", "a6",
        "a7", "s1", "memory");
#else
#error clone3 needs a manual asm wrapper.
#endif

  if (pid < 0) {
    errno = -pid;
    pid = -1;
  }
  return pid;
}

int stack_test_func(void* a) {
  // Force a stack write by creating an asm block
  // that has an input that needs to come from memory.
  int pid = *reinterpret_cast<int*>(a);
  __asm__("" ::"m"(pid));

  if (getpid() != pid)
    return kChildErrorExitCode;

  return kChildExpectedExitCode;
}

int empty_func(void*) { return 0; }

void ReadStackStart(pid_t pid, uintptr_t* result) {
  std::string contents, path = fxl::StringPrintf("/proc/%ld/stat", static_cast<long>(pid));
  ASSERT_TRUE(files::ReadFileToString(path, &contents));

  auto parts = fxl::SplitString(contents, " ", fxl::kTrimWhitespace, fxl::kSplitWantAll);
  ASSERT_GE(parts.size(), 28U) << contents;
  ASSERT_TRUE(fxl::StringToNumberWithError(parts[27], result)) << parts[27];
}

void ReadStackSize(pid_t pid, uintptr_t* result) {
  std::string contents, path = fxl::StringPrintf("/proc/%ld/status", static_cast<long>(pid));
  ASSERT_TRUE(files::ReadFileToString(path, &contents));
  std::vector<std::string_view> lines =
      fxl::SplitString(contents, "\n", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);

  bool vm_stk_found = false;
  for (auto line : lines) {
    auto key_and_value =
        fxl::SplitString(line, ": ", fxl::kTrimWhitespace, fxl::kSplitWantNonEmpty);
    if (key_and_value.size() >= 2 && key_and_value[0] == "VmStk") {
      ASSERT_TRUE(fxl::StringToNumberWithError(key_and_value[1], result)) << line;
      vm_stk_found = true;
      break;
    }
  }

  ASSERT_TRUE(vm_stk_found) << contents;
}

void ReadProcPidFile(pid_t pid, const char* name, std::vector<uint8_t>* result) {
  std::string path = fxl::StringPrintf("/proc/%ld/%s", static_cast<long>(pid), name);
  ASSERT_TRUE(files::ReadFileToVector(path, result));
}

}  // namespace

// Creates a child process using the "clone3()" syscall and waits on it.
// The child uses a different stack than the parent.
TEST(Task, Clone3_ChangeStack) {
  struct clone_args ca;
  bzero(&ca, sizeof(ca));

  ca.flags = CLONE_PARENT_SETTID | CLONE_CHILD_SETTID;
  ca.exit_signal = SIGCHLD;  // Needed in order to wait on the child.

  // Ask for the child PID to be reported to both the parent and the child for validation.
  uint64_t child_pid_from_clone = 0;
  ca.parent_tid = reinterpret_cast<uint64_t>(&child_pid_from_clone);
  ca.child_tid = reinterpret_cast<uint64_t>(&child_pid_from_clone);

  constexpr size_t kStackSize = 0x5000;
  void* stack_addr = mmap(nullptr, kStackSize, PROT_WRITE | PROT_READ,
                          MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
  ASSERT_NE(MAP_FAILED, stack_addr);

  ca.stack = reinterpret_cast<uint64_t>(stack_addr);
  ca.stack_size = kStackSize;

  auto child_pid = DoClone3(&ca, sizeof(ca), &stack_test_func, &child_pid_from_clone);
  ASSERT_NE(child_pid, -1);

  EXPECT_EQ(static_cast<pid_t>(child_pid_from_clone), child_pid);

  // Wait for the child to terminate and validate the exit code. Note that it returns a different
  // exit code above to indicate its state wasn't as expected.
  int wait_status = 0;
  pid_t wait_result = waitpid(child_pid, &wait_status, 0);
  EXPECT_EQ(wait_result, child_pid);

  EXPECT_TRUE(WIFEXITED(wait_status));
  auto exit_status = WEXITSTATUS(wait_status);
  EXPECT_NE(exit_status, kChildErrorExitCode) << "Child process reported state was unexpected.";
  EXPECT_EQ(exit_status, kChildExpectedExitCode) << "Wrong exit code from child process.";

  ASSERT_EQ(0, munmap(stack_addr, kStackSize));
}

// Forks a child process using the "clone3()" syscall and waits on it, validating some parameters.
TEST(Task, Clone3_Fork) {
  struct clone_args ca;
  bzero(&ca, sizeof(ca));

  ca.flags = CLONE_PARENT_SETTID | CLONE_CHILD_SETTID;
  ca.exit_signal = SIGCHLD;  // Needed in order to wait on the child.

  // Ask for the child PID to be reported to both the parent and the child for validation.
  uint64_t child_pid_from_clone = 0;
  ca.parent_tid = reinterpret_cast<uint64_t>(&child_pid_from_clone);
  ca.child_tid = reinterpret_cast<uint64_t>(&child_pid_from_clone);

  auto child_pid = ForkUsingClone3(&ca, sizeof(ca));
  ASSERT_NE(child_pid, -1);
  if (child_pid == 0) {
    // In child process. We'd like to EXPECT_EQ the pid but this is a child process and the gtest
    // failure won't get caught. Instead, return a different result code and the parent will notice
    // and issue an error about the state being unexpected.
    if (getpid() != static_cast<pid_t>(child_pid_from_clone))
      exit(kChildErrorExitCode);
    exit(kChildExpectedExitCode);
  } else {
    EXPECT_EQ(static_cast<pid_t>(child_pid_from_clone), child_pid);

    // Wait for the child to terminate and validate the exit code. Note that it returns a different
    // exit code above to indicate its state wasn't as expected.
    int wait_status = 0;
    pid_t wait_result = waitpid(child_pid, &wait_status, 0);
    EXPECT_EQ(wait_result, child_pid);

    EXPECT_TRUE(WIFEXITED(wait_status));
    auto exit_status = WEXITSTATUS(wait_status);
    EXPECT_NE(exit_status, kChildErrorExitCode) << "Child process reported state was unexpected.";
    EXPECT_EQ(exit_status, kChildExpectedExitCode) << "Wrong exit code from child process.";
  }
}

// Forks a child process using the "clone3()" syscall and requests a PID-FD for it.
TEST(Task, Clone3_PidFd) {
  struct clone_args ca;
  bzero(&ca, sizeof(ca));

  ca.flags = CLONE_PIDFD;
  ca.exit_signal = SIGCHLD;  // Needed in order to wait on the child.

  // Ask for a PID FD through which the child process can be observed.
  fbl::unique_fd pid_fd;
  ca.pidfd = reinterpret_cast<uint64_t>(pid_fd.reset_and_get_address());

  auto child_pid = ForkUsingClone3(&ca, sizeof(ca));
  ASSERT_NE(child_pid, -1) << strerror(errno);
  if (child_pid == 0) {
    exit(kChildExpectedExitCode);
  } else {
    EXPECT_TRUE(pid_fd.is_valid());

    // Wait for the child to terminate.
    int wait_status = 0;
    pid_t wait_result = waitpid(child_pid, &wait_status, 0);
    EXPECT_EQ(wait_result, child_pid) << strerror(errno);
  }
}

static int ClonePidFdFunctionExit(void*) { exit(kChildExpectedExitCode); }

// Forks a child process using the "clone()" syscall and requests a PID-FD for it.
TEST(Task, Clone_PidFd) {
  constexpr size_t kStackSize = 1024 * 16;
  void* stack_low = mmap(nullptr, kStackSize, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
  ASSERT_NE(stack_low, MAP_FAILED);
  void* stack_high = static_cast<char*>(stack_low) + kStackSize;  // Pass in the top of the stack.

  fbl::unique_fd pid_fd;
  auto child_pid = clone(&ClonePidFdFunctionExit, stack_high, SIGCHLD | CLONE_PIDFD,
                         /*arg=*/nullptr, pid_fd.reset_and_get_address());
  ASSERT_NE(child_pid, -1) << strerror(errno);
  EXPECT_TRUE(pid_fd.is_valid());

  // Wait for the child to terminate.
  int wait_status = 0;
  pid_t wait_result = waitpid(child_pid, &wait_status, 0);
  EXPECT_EQ(wait_result, child_pid) << strerror(errno);
}

// Forks a child process using the "clone()" syscall and requests both PID-FD and parent TID.
TEST(Task, Clone_PidFdAndParentTid) {
  constexpr size_t kStackSize = 1024 * 16;
  void* stack_low = mmap(nullptr, kStackSize, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
  ASSERT_NE(stack_low, MAP_FAILED);
  void* stack_high = static_cast<char*>(stack_low) + kStackSize;  // Pass in the top of the stack.

  uintptr_t out_arg{};
  auto child_pid =
      clone(&ClonePidFdFunctionExit, stack_high, SIGCHLD | CLONE_PIDFD | CLONE_PARENT_SETTID,
            /*arg=*/nullptr, &out_arg);
  EXPECT_EQ(child_pid, -1);
}

TEST(Task, Clone3_InvalidSize) {
  struct clone_args ca;
  bzero(&ca, sizeof(ca));

  // Pass a structure size smaller than the first supported version, it should report EINVAL.
  EXPECT_EQ(-1, DoClone3(&ca, CLONE_ARGS_SIZE_VER0 - 8, &empty_func, nullptr));
  EXPECT_EQ(EINVAL, errno);
}

static int CloneVForkFunctionSleepExit(void* param) {
  struct timespec wait{.tv_sec = 0, .tv_nsec = kCloneVforkSleepUS * 1000};
  nanosleep(&wait, nullptr);
  // Note: exit() is a stdlib function that exits the whole process which we don't want.
  // _exit just exits the current thread which is what matches clone().
  _exit(1);
  return 0;
}

// Tests a CLONE_VFORK and the cloned thread exits before calling execve. The clone() call should
// block until the thread exits.
TEST(Task, CloneVfork_exit) {
  constexpr size_t kStackSize = 1024 * 16;
  void* stack_low = mmap(nullptr, kStackSize, PROT_READ | PROT_WRITE,
                         MAP_PRIVATE | MAP_ANONYMOUS | MAP_STACK, -1, 0);
  ASSERT_NE(stack_low, MAP_FAILED);
  void* stack_high = static_cast<char*>(stack_low) + kStackSize;  // Pass in the top of the stack.

  struct timeval begin;
  struct timezone tz;
  gettimeofday(&begin, &tz);

  // This uses the glibc "clone()" wrapper function which takes a function pointer.
  int result = clone(&CloneVForkFunctionSleepExit, stack_high, CLONE_VFORK, 0);
  ASSERT_NE(result, -1);

  struct timeval end;
  gettimeofday(&end, &tz);

  // The clone function should have been blocked for at least as long as the sleep was for.
  uint64_t elapsed_us = ((int64_t)end.tv_sec - (int64_t)begin.tv_sec) * 1000000ll +
                        ((int64_t)end.tv_usec - (int64_t)begin.tv_usec);
  EXPECT_GT(elapsed_us, kCloneVforkSleepUS);
}

// Invokes `brk()` syscall directly. The syscall returns the requested `new_break` on success,
// or the old break on failure. Setting `new_break` to null always fails, so can be used to
// retrieve the current break.
// Some tests invoke this directly so as to bypass the `brk()` wrapper provided by some libc
// implementations, that has different behaviour, matching the POSIX specification.
uintptr_t brk_syscall(uintptr_t new_break) { return syscall(SYS_brk, new_break); }

TEST(Task, BrkReturnsCurrentBreakOnFailure) {
  // Tests that the brk system call doesn't return an error, instead it returns
  // the current value of the program break.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

    // This should always fail, returning the program_break
    uintptr_t program_break = brk_syscall(UINTPTR_MAX);

    // Try to reserve something beyond the program break, aligned to page size.
    uintptr_t map_addr = (program_break & ~(page_size - 1)) + page_size;
    uintptr_t res =
        reinterpret_cast<uintptr_t>(mmap(reinterpret_cast<void*>(map_addr), page_size, PROT_NONE,
                                         MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED_NOREPLACE, -1, 0));
    ASSERT_EQ(res, map_addr);

    uintptr_t new_break = brk_syscall(map_addr + page_size);
    // brk should fail.
    EXPECT_EQ(new_break, program_break);

    _exit(testing::Test::HasFailure());
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(Task, BrkShrinkAfterFork) {
  // Tests that a program can shrink their break after forking.
  const void* kSbrkError = reinterpret_cast<void*>(-1);
  test_helper::ForkHelper helper;

  constexpr int brk_increment = 0x4000;
  ASSERT_NE(kSbrkError, sbrk(brk_increment));
  void* old_brk = sbrk(0);
  ASSERT_NE(kSbrkError, old_brk);
  helper.RunInForkedProcess([&] {
    ASSERT_EQ(old_brk, sbrk(0));
    ASSERT_NE(kSbrkError, sbrk(-brk_increment));
  });

  // Make sure fork didn't change our current break.
  ASSERT_EQ(old_brk, sbrk(0));

  // Restore the old brk.
  ASSERT_NE(kSbrkError, sbrk(-brk_increment));
}

// Returns true if the `len` bytes at `ptr` all have value `value`.
static bool mem_contains(volatile char* ptr, size_t len, char value) {
  for (const auto* end = ptr + len; ptr != end; ++ptr) {
    if (*ptr != value)
      return false;
  }
  return true;
}

// Moves the program break to a page aligned address, and returns the old break address.
static uintptr_t set_brk_page_aligned() {
  size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  uintptr_t original_break = brk_syscall(0);
  uintptr_t aligned_break = (original_break + page_size) & ~(page_size - 1);
  if (brk_syscall(aligned_break) != aligned_break) {
    return 0;
  }
  return original_break;
}

// Verify that if the program break is reduced down to a page-aligned address, and then
// regrown, then all the pages, including the one pointed to by the page-aligned address,
// are zeroed.
TEST(Task, BrkIsZeroedAfterShrinkAndRegrow) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    const void* kSbrkError = reinterpret_cast<void*>(-1);

    ASSERT_NE(set_brk_page_aligned(), 0u);

    // Allocate space via `sbrk()`, and fill it with non-zero bytes.
    constexpr intptr_t kBreakIncrement = 0x4142;
    void* base_break = sbrk(kBreakIncrement);
    ASSERT_NE(base_break, kSbrkError) << "sbrk failed: " << std::strerror(errno);
    char* memory = static_cast<char*>(base_break);
    ASSERT_TRUE(mem_contains(memory, static_cast<size_t>(kBreakIncrement), 0));
    memset(memory, 'a', kBreakIncrement);
    ASSERT_FALSE(mem_contains(memory, static_cast<size_t>(kBreakIncrement), 0));

    // Shrink the `sbrk()` to free the pages, then re-allocate them.
    ASSERT_NE(sbrk(-kBreakIncrement), kSbrkError) << "sbrk failed: " << std::strerror(errno);
    ASSERT_NE(sbrk(kBreakIncrement), kSbrkError) << "sbrk failed: " << std::strerror(errno);

    // The re-allocated range should now be zeroed.
    EXPECT_TRUE(mem_contains(memory, static_cast<size_t>(kBreakIncrement), 0));

    _exit(testing::Test::HasFailure());
  });
}

// Verifies that overwriting the program break with private or shared mappings does not
// prevent the program break from being shrunk, and regrown.
TEST(Task, BrkShrinkUnmapsAnonMmap) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_NE(set_brk_page_aligned(), 0u);

    size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

    // Allocate eight pages from the program break.
    uintptr_t program_break = brk_syscall(0);
    uintptr_t new_break = brk_syscall(program_break + (8 * page_size));
    ASSERT_EQ(new_break, program_break + (8 * page_size));

    // Anonymously map over second and third pages, with RW and read-only private pages.
    uintptr_t addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + page_size), page_size, PROT_WRITE | PROT_READ,
             MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + page_size);
    addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + (2 * page_size)), page_size, PROT_READ,
             MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + (2 * page_size));

    // Anonymously map over fifth and sixth pages, with RW and read-only shared pages.
    addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + (4 * page_size)), page_size,
             PROT_WRITE | PROT_READ, MAP_SHARED | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + (4 * page_size));
    addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + (5 * page_size)), page_size, PROT_READ,
             MAP_SHARED | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + (5 * page_size));

    // Shrink the program break back down.
    uintptr_t final_break = brk_syscall(program_break);
    EXPECT_EQ(final_break, program_break);

    // Validate that all eight pages are no longer available.
    for (size_t i = 0; i < 8; i++) {
      EXPECT_FALSE(test_helper::TryRead(program_break + (i * page_size)))
          << "page " << i + 1 << " is readable";
    }

    // Regrow and validate that all eight pages are then readable.
    new_break = brk_syscall(program_break + (8 * page_size));
    EXPECT_EQ(new_break, program_break + (8 * page_size));
    for (size_t i = 0; i < 8; i++) {
      EXPECT_TRUE(test_helper::TryRead(program_break + (i * page_size)))
          << "page " << i + 1 << " not readable";
    }

    _exit(testing::Test::HasFailure());
  });
}

// Verifies that the program break can be extended after `mmap()` has been used to overwrite
// preceding parts of the program break area, and that doing so does not alter those
// mappings.
TEST(Task, BrkGrowsAfterAnonMmap) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_NE(set_brk_page_aligned(), 0u);

    size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

    // Allocate eight pages from the program break, and set them non-zero.
    uintptr_t program_break = brk_syscall(0);
    uintptr_t new_break = brk_syscall(program_break + (8 * page_size));
    ASSERT_EQ(new_break, program_break + (8 * page_size));

    char* break_memory = reinterpret_cast<char*>(program_break);
    memset(break_memory, 'a', 8 * page_size);
    ASSERT_TRUE(mem_contains(break_memory, 8 * page_size, 'a'));

    // Anonymously map over second and third pages, with RW and read-only private pages.
    uintptr_t addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + page_size), page_size, PROT_WRITE | PROT_READ,
             MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + page_size);
    addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + (2 * page_size)), page_size, PROT_READ,
             MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + (2 * page_size));

    // Anonymously map over fifth and sixth pages, with RW and read-only shared pages.
    addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + (4 * page_size)), page_size,
             PROT_WRITE | PROT_READ, MAP_SHARED | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + (4 * page_size));
    addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + (5 * page_size)), page_size, PROT_READ,
             MAP_SHARED | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + (5 * page_size));

    // Fill the writable `mmap()`ed pages with identifiable content.
    memset(break_memory + page_size, 'b', page_size);
    memset(break_memory + 4 * page_size, 'c', page_size);

    // Extend the program break by another eight pages.
    new_break = brk_syscall(program_break + (16 * page_size));
    ASSERT_EQ(new_break, program_break + (16 * page_size));

    // Everything preceding the old break should be preserved.
    EXPECT_TRUE(mem_contains(break_memory, page_size, 'a'));  // first of the old break pages
    EXPECT_TRUE(mem_contains(break_memory + page_size, page_size, 'b'));
    EXPECT_TRUE(mem_contains(break_memory + 4 * page_size, page_size, 'c'));
    EXPECT_TRUE(mem_contains(break_memory + 7 * page_size, page_size,
                             'a'));  // last of the old break pages.

    // The new break area will be zeroed.
    EXPECT_TRUE(mem_contains(break_memory + 8 * page_size, 8 * page_size, 0));

    _exit(testing::Test::HasFailure());
  });
}

// Verify that the program break will not grow over existing private mappings.
TEST(Task, BrkWillNotGrowOverPrivateAnonMmap) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_NE(set_brk_page_aligned(), 0u);

    size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

    // Create anonymous mapped pages after the break.
    uintptr_t program_break = brk_syscall(0);

    // Anonymously map over second and third pages, with RW and read-only private pages.
    uintptr_t addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + page_size), page_size, PROT_WRITE | PROT_READ,
             MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + page_size);
    addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + (2 * page_size)), page_size, PROT_READ,
             MAP_PRIVATE | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + (2 * page_size));

    // Attempt to grow the program break over the private mappings.
    // This should fail, causing the old program break position to remain current.
    uintptr_t new_break = brk_syscall(program_break + (8 * page_size));
    EXPECT_EQ(new_break, program_break);

    _exit(testing::Test::HasFailure());
  });
}

// Verify that the program break will not grow over existing private mappings.
TEST(Task, BrkWillNotGrowOverSharedAnonMmap) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_NE(set_brk_page_aligned(), 0u);

    size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

    // Create shared mapped pages after the break.
    uintptr_t program_break = brk_syscall(0);

    // Anonymously map over second and fourth pages, with RW and read-only shared pages.
    uintptr_t addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + (page_size)), page_size,
             PROT_WRITE | PROT_READ, MAP_SHARED | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + page_size);
    addr = reinterpret_cast<uintptr_t>(
        mmap(reinterpret_cast<void*>(program_break + (3 * page_size)), page_size, PROT_READ,
             MAP_SHARED | MAP_ANONYMOUS | MAP_FIXED, -1, 0));
    ASSERT_EQ(addr, program_break + (3 * page_size));

    // Attempt to grow the program break over the shared mappings.
    // This should fail, causing the old program break position to remain current.
    uintptr_t new_break = brk_syscall(program_break + (8 * page_size));
    EXPECT_EQ(new_break, program_break);

    _exit(testing::Test::HasFailure());
  });
}

// Verify that the `brk()` syscall continues to function after pages within the
// program break have been explicitly unmapped by the caller.
TEST(Task, BrkCanBeUnmapped) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    ASSERT_NE(set_brk_page_aligned(), 0u);

    size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));

    // Allocate a page from the program break.
    uintptr_t program_break = brk_syscall(0);
    uintptr_t new_break = brk_syscall(program_break + page_size);

    // Unmap the last page we just added.
    ASSERT_TRUE(test_helper::TryRead(program_break));
    SAFE_SYSCALL(munmap(reinterpret_cast<void*>(program_break), page_size));
    EXPECT_FALSE(test_helper::TryRead(program_break));

    // The program break hasn't changed.
    uintptr_t break_after_unmap = brk_syscall(0);
    EXPECT_EQ(break_after_unmap, new_break);

    // The program break can still be extended.
    uintptr_t final_break = brk_syscall(new_break + page_size);
    EXPECT_EQ(final_break, new_break + page_size);

    // The final page of the break is readable, and the unmapped page is still not.
    EXPECT_TRUE(test_helper::TryRead(new_break));
    EXPECT_FALSE(test_helper::TryRead(program_break));

    // The break can be rewound over the unmapped region.
    EXPECT_EQ(brk_syscall(program_break), program_break);

    _exit(testing::Test::HasFailure());
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

// Verify that the current program break address is treated as outside the break.
TEST(Task, BrkIsOutOfRange) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    // Set the program break to be page-aligned.
    ASSERT_NE(set_brk_page_aligned(), 0u);

    // The break address should refer to the first byte of the page after the break,
    // and therefore not be mapped.
    uintptr_t aligned_break = brk_syscall(0);
    EXPECT_FALSE(test_helper::TryRead(aligned_break));

    // Increasing the break by one byte effectively "allocates" the byte at the
    // aligned break address, causing that page to now be mapped.
    uintptr_t increased_break = aligned_break + 1;
    EXPECT_EQ(brk_syscall(increased_break), increased_break);
    EXPECT_TRUE(test_helper::TryRead(aligned_break));

    // Reducing the break by one byte "deallocates" the byte at the aligned
    // address, causing the page to be unmapped.
    EXPECT_EQ(brk_syscall(aligned_break), aligned_break);
    EXPECT_FALSE(test_helper::TryRead(aligned_break));

    _exit(testing::Test::HasFailure());
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(Task, ChildCantModifyParent) {
  ASSERT_GT(MAX_PAGE_ALIGNMENT, getpagesize());

  test_helper::ForkHelper helper;

  g_global_variable_data = 1;
  g_global_variable_bss = 10;
  volatile int local_variable = 100;
  volatile int* heap_variable = new volatile int();
  *heap_variable = 1000;

  ASSERT_EQ(g_global_variable_data, 1u);
  ASSERT_EQ(g_global_variable_bss, 10);
  ASSERT_EQ(local_variable, 100);
  ASSERT_EQ(*heap_variable, 1000);

  helper.RunInForkedProcess([&] {
    g_global_variable_data = 2;
    g_global_variable_bss = 20;
    local_variable = 200;
    *heap_variable = 2000;
  });
  ASSERT_TRUE(helper.WaitForChildren());

  EXPECT_EQ(g_global_variable_data, 1u);
  EXPECT_EQ(g_global_variable_bss, 10);
  EXPECT_EQ(local_variable, 100);
  EXPECT_EQ(*heap_variable, 1000);
  delete heap_variable;
}

TEST(Task, ForkDoesntDropWrites) {
  // This tests creates a thread that keeps reading
  // and writing to a pager-backed memory region (the data section of this
  // binary).
  //
  // This is to test that during fork, writes to pager-backed vmos are not
  // dropped if we have concurrent processes writing to memory while the kernel
  // changes those mappings.
  std::atomic<bool> stop = false;
  std::atomic<bool> success = false;
  std::vector<pid_t> pids;

  std::thread writer([&stop, &success, &data = g_fork_doesnt_drop_writes]() {
    data = 0;
    size_t i = 0;
    success = false;
    while (!stop) {
      i++;
      data += 1;
      if (data != i) {
        return;
      }
    }
    success = true;
  });

  for (size_t i = 0; i < 1000; i++) {
    pid_t pid = fork();
    if (pid == 0) {
      _exit(0);
    }
    pids.push_back(pid);
  }

  stop = true;
  writer.join();
  EXPECT_TRUE(success.load());

  for (auto pid : pids) {
    int status;
    EXPECT_EQ(waitpid(pid, &status, 0), pid);
    EXPECT_TRUE(WIFEXITED(status));
    EXPECT_EQ(WEXITSTATUS(status), 0);
  }
}

TEST(Task, ParentCantModifyChild) {
  ASSERT_GT(MAX_PAGE_ALIGNMENT, getpagesize());

  test_helper::ForkHelper helper;

  g_global_variable_data = 1;
  g_global_variable_bss = 10;
  volatile int local_variable = 100;
  volatile int* heap_variable = new volatile int();
  *heap_variable = 1000;

  ASSERT_EQ(g_global_variable_data, 1u);
  ASSERT_EQ(g_global_variable_bss, 10);
  ASSERT_EQ(local_variable, 100);
  ASSERT_EQ(*heap_variable, 1000);

  test_helper::SignalMaskHelper signal_helper = test_helper::SignalMaskHelper();
  signal_helper.blockSignal(SIGUSR1);

  pid_t child_pid = helper.RunInForkedProcess([&] {
    signal_helper.waitForSignal(SIGUSR1);

    EXPECT_EQ(g_global_variable_data, 1u);
    EXPECT_EQ(g_global_variable_bss, 10);
    EXPECT_EQ(local_variable, 100);
    EXPECT_EQ(*heap_variable, 1000);
  });

  g_global_variable_data = 2;
  g_global_variable_bss = 20;
  local_variable = 200;
  *heap_variable = 2000;

  ASSERT_EQ(kill(child_pid, SIGUSR1), 0);

  ASSERT_TRUE(helper.WaitForChildren());
  signal_helper.restoreSigmask();

  delete heap_variable;
}

constexpr size_t kVecSize = 100;
constexpr size_t kPageLimit = 32;

TEST(Task, ExecveArgumentExceedsMaxArgStrlen) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    size_t arg_size = kPageLimit * sysconf(_SC_PAGESIZE);
    std::vector<char> arg(arg_size + 1, 'a');
    arg[arg_size] = '\0';
    char* argv[] = {arg.data(), nullptr};
    char* envp[] = {nullptr};
    EXPECT_NE(execve("/proc/self/exe", argv, envp), 0);
    EXPECT_EQ(errno, E2BIG);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ExecveArgvExceedsLimit) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    size_t arg_size = kPageLimit * sysconf(_SC_PAGESIZE);
    std::vector<char> arg(arg_size, 'a');
    arg[arg_size - 1] = '\0';
    char* argv[kVecSize];
    std::fill_n(argv, kVecSize - 1, arg.data());
    argv[kVecSize - 1] = nullptr;
    char* envp[] = {nullptr};
    EXPECT_NE(execve("/proc/self/exe", argv, envp), 0);
    EXPECT_EQ(errno, E2BIG);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ExecveArgvEnvExceedLimit) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    size_t arg_size = kPageLimit * sysconf(_SC_PAGESIZE);
    std::vector<char> string(arg_size, 'a');
    string[arg_size - 1] = '\0';
    char* argv[] = {string.data(), nullptr};
    char* envp[kVecSize];
    std::fill_n(envp, kVecSize - 1, string.data());
    envp[kVecSize - 1] = nullptr;
    EXPECT_NE(execve("/proc/self/exe", argv, envp), 0);
    EXPECT_EQ(errno, E2BIG);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ExecvePathnameTooLong) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([] {
    constexpr size_t path_size = PATH_MAX + 1;
    // We use '/' here because ////// (...) is a valid path:
    // More than two leading / should be considered as one,
    // So this path resolves to "/".
    //
    // Each path component is limited to NAME_MAX, so if we
    // use a different character we would need to add a delimiter
    // every NAME_MAX characters.
    std::vector<char> pathname(path_size, '/');
    pathname[path_size - 1] = '\0';

    char* argv[] = {nullptr};
    char* envp[] = {nullptr};

    EXPECT_NE(execve(pathname.data(), argv, envp), 0);
    EXPECT_EQ(errno, ENAMETOOLONG);
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ForkPreservesStackStart) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([parent_pid] {
    uintptr_t parent_stack_start, child_stack_start;
    ASSERT_NO_FATAL_FAILURE(ReadStackStart(parent_pid, &parent_stack_start));
    ASSERT_NO_FATAL_FAILURE(ReadStackStart(getpid(), &child_stack_start));
    EXPECT_EQ(parent_stack_start, child_stack_start);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ForkPreservesStackSize) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([parent_pid] {
    uintptr_t parent_stack_size, child_stack_size;
    ASSERT_NO_FATAL_FAILURE(ReadStackSize(parent_pid, &parent_stack_size));
    ASSERT_NO_FATAL_FAILURE(ReadStackSize(getpid(), &child_stack_size));
    EXPECT_EQ(parent_stack_size, child_stack_size);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ForkPreservesAux) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([parent_pid] {
    std::vector<uint8_t> parent_auxv, child_auxv;
    ASSERT_NO_FATAL_FAILURE(ReadProcPidFile(parent_pid, "auxv", &parent_auxv));
    ASSERT_NO_FATAL_FAILURE(ReadProcPidFile(getpid(), "auxv", &child_auxv));
    EXPECT_EQ(parent_auxv, child_auxv);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ForkPreservesCmdline) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([parent_pid] {
    std::vector<uint8_t> parent_cmdline, child_cmdline;
    ASSERT_NO_FATAL_FAILURE(ReadProcPidFile(parent_pid, "cmdline", &parent_cmdline));
    ASSERT_NO_FATAL_FAILURE(ReadProcPidFile(getpid(), "cmdline", &child_cmdline));
    EXPECT_EQ(parent_cmdline, child_cmdline);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ForkPreservesEnviron) {
  pid_t parent_pid = getpid();
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([parent_pid] {
    std::vector<uint8_t> parent_environ, child_environ;
    ASSERT_NO_FATAL_FAILURE(ReadProcPidFile(parent_pid, "environ", &parent_environ));
    ASSERT_NO_FATAL_FAILURE(ReadProcPidFile(getpid(), "environ", &child_environ));
    EXPECT_EQ(parent_environ, child_environ);
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, KillESRCH) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    pid_t pid = getpid();

    // A pid after fork is guaranteed to not collide with process group ids.
    EXPECT_THAT(kill(-pid, SIGCHLD), SyscallFailsWithErrno(ESRCH));
  });
}

TEST(Task, KillChildEPERM) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "test requires root privileges";
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    test_helper::EventFdSem fd(0);

    pid_t child_pid = SAFE_SYSCALL(fork());
    if (child_pid == 0) {
      fd.Wait();
      _exit(EXIT_SUCCESS);
    }

    SAFE_SYSCALL(change_ids(kUser1Uid, kUser1Gid));

    EXPECT_THAT(kill(child_pid, SIGCHLD), SyscallFailsWithErrno(EPERM));
    fd.Notify(1);

    SAFE_SYSCALL(waitpid(child_pid, nullptr, 0));
  });
}

TEST(Task, KillZombie) {
  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([]() {
    pid_t child_pid = SAFE_SYSCALL(fork());
    if (child_pid == 0) {
      _exit(EXIT_SUCCESS);
    }

    int pidfd = test_helper::PidFdOpen(child_pid, 0);
    WaitForReadFd(pidfd);
    close(pidfd);

    EXPECT_THAT(kill(child_pid, SIGCHLD), SyscallSucceeds());
    SAFE_SYSCALL(waitpid(child_pid, nullptr, 0));
  });
}

TEST(Task, KillZombieEPERM) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "test requires root privileges";
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    pid_t child_pid = SAFE_SYSCALL(fork());
    if (child_pid == 0) {
      _exit(EXIT_SUCCESS);
    }

    int pidfd = test_helper::PidFdOpen(child_pid, 0);
    WaitForReadFd(pidfd);
    close(pidfd);

    SAFE_SYSCALL(change_ids(kUser1Uid, kUser1Gid));
    EXPECT_THAT(kill(child_pid, SIGCHLD), SyscallFailsWithErrno(EPERM));
    SAFE_SYSCALL(waitpid(child_pid, nullptr, 0));
  });
}

TEST(Task, KillParentEPERM) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "test requires root privileges";
  }

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    test_helper::EventFdSem fd(0);

    pid_t child_pid = SAFE_SYSCALL(fork());
    if (child_pid == 0) {
      pid_t parent_pid = getppid();
      SAFE_SYSCALL(change_ids(kUser1Uid, kUser1Gid));

      EXPECT_THAT(kill(parent_pid, SIGCHLD), SyscallFailsWithErrno(EPERM));
      SAFE_SYSCALL(fd.Notify(1));
      _exit(EXIT_SUCCESS);
    }

    fd.Wait();
    SAFE_SYSCALL(waitpid(child_pid, nullptr, 0));
  });
}

TEST(Task, CantReadLowAddresses) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP();
  }

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  constexpr uintptr_t kLowMemoryLimit = 16 * 1024 * 1024;
  for (uintptr_t addr = 0x0; addr < kLowMemoryLimit; addr += page_size) {
    EXPECT_FALSE(test_helper::TryRead(addr));
  }
}

TEST(Task, CantWriteLowAddresses) {
  if (!test_helper::IsStarnix()) {
    GTEST_SKIP();
  }

  const size_t page_size = SAFE_SYSCALL(sysconf(_SC_PAGE_SIZE));
  constexpr uintptr_t kLowMemoryLimit = 16 * 1024 * 1024;
  for (uintptr_t addr = 0x0; addr < kLowMemoryLimit; addr += page_size) {
    EXPECT_FALSE(test_helper::TryWrite(addr));
  }
}

class CloneAndExecTest : public ::testing::Test {
 protected:
  void SetUp() { clone_exec_helper_ = test_helper::GetTestResourcePath(kCloneExecHelperBinary); }

  void CloneAndExec(int clone_flags, const std::vector<std::string>& arguments) {
    // Clone the process, with the caller-supplied flags.
    int child_pid = static_cast<int>(SAFE_SYSCALL(
        syscall(SYS_clone, clone_flags | SIGCHLD, nullptr, nullptr, nullptr, nullptr)));

    if (child_pid == 0) {
      // This is the child process, so exec() the specified command.
      std::vector<char*> args;
      args.push_back(clone_exec_helper_.data());
      for (const std::string& argument : arguments) {
        args.push_back(const_cast<char*>(argument.data()));
      }
      args.push_back(nullptr);
      char* const envp[] = {nullptr};

      SAFE_SYSCALL(execve(clone_exec_helper_.c_str(), args.data(), envp));
      _exit(EXIT_FAILURE);
    }

    // This is the parent process. Wait for the child process to complete, then return to the caller
    // to perform test validation.
    int wstatus = 0;
    SAFE_SYSCALL(waitpid(child_pid, &wstatus, 0));
    if (!WIFEXITED(wstatus) || WEXITSTATUS(wstatus) != 0) {
      ADD_FAILURE() << "wait_status: WIFEXITED(wstatus) = " << WIFEXITED(wstatus)
                    << ", WEXITSTATUS(wstatus) = " << WEXITSTATUS(wstatus)
                    << ", WTERMSIG(wstatus) = " << WTERMSIG(wstatus);
    }
  }

  static std::string GetCurrentWorkingDirectory() {
    std::string cwd = get_current_dir_name();
    if (cwd.empty()) {
      _exit(EXIT_FAILURE);
    }
    return cwd;
  }

  static constexpr char kCloneExecHelperBinary[] = "clone_exec_helper";

  std::string clone_exec_helper_;
};

TEST_F(CloneAndExecTest, ExecUnsharesCloneFiles) {
  // Fork into a subprocess from which to `clone(CLONE_FILES)`, to avoid breaking the main test
  // process if things go awry.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // Open a temporary file to use as the "canary" for the FD table remaining shared.
    auto fd = test_helper::ScopedTempFD();
    ASSERT_THAT(fcntl(fd.fd(), F_GETFD), SyscallSucceeds());

    // The child process will run the helper with a command to close `fd`.
    std::vector<std::string> args{"close_fd", std::to_string(fd.fd())};
    CloneAndExec(CLONE_FILES, args);

    // Verify that the FD is still open in our FD table.
    EXPECT_THAT(fcntl(fd.fd(), F_GETFD), SyscallSucceeds());

    _exit(testing::Test::HasFailure());
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(CloneAndExecTest, ExecDoesNotUnshareCloneFs) {
  // Fork into a subprocess from which to `clone(CLONE_FS)`, to avoid breaking the main test
  // process if things go awry.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // The child process will run the helper with a command-line to modify the current working
    // directory.
    std::string original_cwd = GetCurrentWorkingDirectory();

    // Create a temporary directory to use as the new CWD.
    test_helper::ScopedTempDir new_cwd;
    std::vector<std::string> args{"set_cwd", new_cwd.path()};
    CloneAndExec(CLONE_FS, args);

    // Verify that this process' current working directory has changed.
    EXPECT_EQ(new_cwd.path(), GetCurrentWorkingDirectory());

    _exit(testing::Test::HasFailure());
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(CloneAndExecTest, ChildWithCloneFsCanChangeCwd) {
  // Fork into a subprocess from which to `clone(CLONE_FS)`, to avoid breaking the main test
  // process.
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    // The child process will change its current working directory from the original value.
    std::string original_cwd = GetCurrentWorkingDirectory();

    // Create a temporary directory to use as the new CWD.
    test_helper::ScopedTempDir new_cwd;

    // Fork the child with CLONE_FS.
    int child_pid = static_cast<int>(
        SAFE_SYSCALL(syscall(SYS_clone, CLONE_FS | SIGCHLD, nullptr, nullptr, nullptr, nullptr)));

    if (child_pid == 0) {
      // This is the child process, so change the current working directory.
      if (chdir(new_cwd.path().c_str()) != 0) {
        fprintf(stderr, "Failed to chdir to %s: %d (%s)\n", new_cwd.path().c_str(), errno,
                strerror(errno));
        exit(EXIT_FAILURE);
      }
      _exit(0);
    }

    // This is the parent process. Wait for the child process to complete, then check whether the
    // current working directory has changed.
    int wstatus = 0;
    SAFE_SYSCALL(waitpid(child_pid, &wstatus, 0));
    if (!WIFEXITED(wstatus) || WEXITSTATUS(wstatus) != 0) {
      ADD_FAILURE() << "wait_status: WIFEXITED(wstatus) = " << WIFEXITED(wstatus)
                    << ", WEXITSTATUS(wstatus) = " << WEXITSTATUS(wstatus)
                    << ", WTERMSIG(wstatus) = " << WTERMSIG(wstatus);
    }

    // Verify that this process' current working directory has changed.
    EXPECT_NE(original_cwd, GetCurrentWorkingDirectory());

    _exit(testing::Test::HasFailure());
  });

  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(CloneAndExecTest, ExecveRequiresCallerCanExecuteScriptAndInterp) {
  test_helper::ScopedTempDir temp_dir;

  const std::string exit_zero_path = test_helper::GetTestResourcePath("exit_zero");
  std::string exit_zero_content;
  ASSERT_TRUE(files::ReadFileToString(exit_zero_path, &exit_zero_content)) << exit_zero_path;

  // Create good (executable) and bad (not executable) "interpreter" binaries by copying the
  // exit_zero helper.
  const std::string bad_interpreter_path = temp_dir.path() + "/bad_interpreter";
  ASSERT_TRUE(files::WriteFile(bad_interpreter_path, exit_zero_content));
  ASSERT_THAT(chmod(bad_interpreter_path.c_str(), 0644), SyscallSucceeds());
  const std::string good_interpreter_path = temp_dir.path() + "/good_interpreter";
  ASSERT_TRUE(files::WriteFile(good_interpreter_path, exit_zero_content));
  ASSERT_THAT(chmod(good_interpreter_path.c_str(), 0744), SyscallSucceeds());

  test_helper::ForkHelper helper;

  // Verify that an non-executable script using an executable interpreter cannot be executed.
  const std::string bad_script_path = temp_dir.path() + "/bad_script";
  const std::string bad_script = "#!" + good_interpreter_path + "\n";
  ASSERT_TRUE(files::WriteFile(bad_script_path, bad_script));
  ASSERT_THAT(chmod(bad_script_path.c_str(), 0644), SyscallSucceeds());
  helper.RunInForkedProcess([&] {
    char* const argv[] = {const_cast<char*>(bad_script_path.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(bad_script_path.c_str(), argv, envp), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());

  // Verify that an executable script using a non-executable interpreter cannot be executed.
  const std::string bad_interp_script_path = temp_dir.path() + "/bad_interp_script";
  const std::string bad_interp_script = "#!" + bad_interpreter_path + "\n";
  ASSERT_TRUE(files::WriteFile(bad_interp_script_path, bad_interp_script));
  ASSERT_THAT(chmod(bad_interp_script_path.c_str(), 0744), SyscallSucceeds());
  helper.RunInForkedProcess([&] {
    char* const argv[] = {const_cast<char*>(bad_interp_script_path.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(bad_interp_script_path.c_str(), argv, envp), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());

  // Verify that an executable script using an executable interpreter can be executed.
  const std::string good_script_path = temp_dir.path() + "/good_script";
  const std::string good_script = "#!" + good_interpreter_path + "\n";
  ASSERT_TRUE(files::WriteFile(good_script_path, good_script));
  ASSERT_THAT(chmod(good_script_path.c_str(), 0744), SyscallSucceeds());
  helper.RunInForkedProcess([&] {
    char* const argv[] = {const_cast<char*>(good_script_path.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(good_script_path.c_str(), argv, envp), SyscallSucceeds());
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(CloneAndExecTest, ExecveFailsIfScriptInterpIsNotRegularFile) {
  test_helper::ScopedTempDir temp_dir;

  // 1. Directory interpreter
  const std::string dir_interp_path = temp_dir.path() + "/dir_interp";
  ASSERT_THAT(mkdir(dir_interp_path.c_str(), 0755), SyscallSucceeds());

  const std::string script1_path = temp_dir.path() + "/script_with_dir_interp";
  const std::string script1_content = "#!" + dir_interp_path + "\n";
  ASSERT_TRUE(files::WriteFile(script1_path, script1_content));
  ASSERT_THAT(chmod(script1_path.c_str(), 0744), SyscallSucceeds());

  // 2. FIFO interpreter
  const std::string fifo_interp_path = temp_dir.path() + "/fifo_interp";
  ASSERT_THAT(mkfifo(fifo_interp_path.c_str(), 0755), SyscallSucceeds());

  const std::string script2_path = temp_dir.path() + "/script_with_fifo_interp";
  const std::string script2_content = "#!" + fifo_interp_path + "\n";
  ASSERT_TRUE(files::WriteFile(script2_path, script2_content));
  ASSERT_THAT(chmod(script2_path.c_str(), 0744), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    char* const argv1[] = {const_cast<char*>(script1_path.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(script1_path.c_str(), argv1, envp), SyscallFailsWithErrno(EACCES));

    char* const argv2[] = {const_cast<char*>(script2_path.c_str()), nullptr};
    EXPECT_THAT(execve(script2_path.c_str(), argv2, envp), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(CloneAndExecTest, ExecveChainedScriptRequiresAllCanBeExecuted) {
  test_helper::ScopedTempDir temp_dir;

  const std::string exit_zero_path = test_helper::GetTestResourcePath("exit_zero");
  std::string exit_zero_content;
  ASSERT_TRUE(files::ReadFileToString(exit_zero_path, &exit_zero_content)) << exit_zero_path;

  const std::string interp_path = temp_dir.path() + "/interpreter";
  ASSERT_TRUE(files::WriteFile(interp_path, exit_zero_content));
  ASSERT_THAT(chmod(interp_path.c_str(), 0755), SyscallSucceeds());

  // Intermediate script2 pointing to interpreter, without execute permission (0644).
  const std::string bad_script2_path = temp_dir.path() + "/bad_script2";
  const std::string script2_content = "#!" + interp_path + "\n";
  ASSERT_TRUE(files::WriteFile(bad_script2_path, script2_content));
  ASSERT_THAT(chmod(bad_script2_path.c_str(), 0644), SyscallSucceeds());

  // Outer script1 pointing to bad_script2, with execute permission (0755).
  const std::string script1_path = temp_dir.path() + "/script1";
  const std::string script1_content = "#!" + bad_script2_path + "\n";
  ASSERT_TRUE(files::WriteFile(script1_path, script1_content));
  ASSERT_THAT(chmod(script1_path.c_str(), 0755), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    char* const argv[] = {const_cast<char*>(script1_path.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(script1_path.c_str(), argv, envp), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(CloneAndExecTest, ExecveChainedScriptRecursionLimit) {
  test_helper::ScopedTempDir temp_dir;

  // Create mutually recursive scripts: script_a -> script_b -> script_a
  const std::string script_a_path = temp_dir.path() + "/script_a";
  const std::string script_b_path = temp_dir.path() + "/script_b";
  ASSERT_TRUE(files::WriteFile(script_a_path, "#!" + script_b_path + "\n"));
  ASSERT_TRUE(files::WriteFile(script_b_path, "#!" + script_a_path + "\n"));
  ASSERT_THAT(chmod(script_a_path.c_str(), 0755), SyscallSucceeds());
  ASSERT_THAT(chmod(script_b_path.c_str(), 0755), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    char* const argv[] = {const_cast<char*>(script_a_path.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(script_a_path.c_str(), argv, envp), SyscallFailsWithErrno(ELOOP));
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ExecveFailsIfPtInterpLacksExecutePermission) {
  test_helper::ScopedTempDir temp_dir;

  const std::string dynamic_linker_path = test_helper::GetSystemDynamicLinkerPath();
  ASSERT_FALSE(dynamic_linker_path.empty());
  std::string dynamic_linker_content;
  ASSERT_TRUE(files::ReadFileToString(dynamic_linker_path, &dynamic_linker_content))
      << dynamic_linker_path;

  // Create ./test_interp in temp_dir without execute permission (0644).
  const std::string interp_path = temp_dir.path() + "/test_interp";
  ASSERT_TRUE(files::WriteFile(interp_path, dynamic_linker_content));
  ASSERT_THAT(chmod(interp_path.c_str(), 0644), SyscallSucceeds());

  const std::string custom_pt_interp_child =
      files::AbsolutePath(test_helper::GetTestResourcePath("custom_pt_interp_child"));

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(chdir(temp_dir.path().c_str()));
    char* const argv[] = {const_cast<char*>(custom_pt_interp_child.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(custom_pt_interp_child.c_str(), argv, envp), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ExecveFailsIfPtInterpIsNotRegularFile) {
  test_helper::ScopedTempDir temp_dir;

  // 1. Directory as ./test_interp
  const std::string dir_path = temp_dir.path() + "/dir_interp";
  ASSERT_THAT(mkdir(dir_path.c_str(), 0755), SyscallSucceeds());
  ASSERT_THAT(mkdir((dir_path + "/test_interp").c_str(), 0755), SyscallSucceeds());

  // 2. FIFO as ./test_interp
  const std::string fifo_path = temp_dir.path() + "/fifo_interp";
  ASSERT_THAT(mkdir(fifo_path.c_str(), 0755), SyscallSucceeds());
  ASSERT_THAT(mkfifo((fifo_path + "/test_interp").c_str(), 0755), SyscallSucceeds());

  const std::string custom_pt_interp_child =
      files::AbsolutePath(test_helper::GetTestResourcePath("custom_pt_interp_child"));

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(chdir(dir_path.c_str()));
    char* const argv[] = {const_cast<char*>(custom_pt_interp_child.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(custom_pt_interp_child.c_str(), argv, envp), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper.WaitForChildren());

  test_helper::ForkHelper helper2;
  helper2.RunInForkedProcess([&] {
    SAFE_SYSCALL(chdir(fifo_path.c_str()));
    char* const argv[] = {const_cast<char*>(custom_pt_interp_child.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(custom_pt_interp_child.c_str(), argv, envp), SyscallFailsWithErrno(EACCES));
  });
  ASSERT_TRUE(helper2.WaitForChildren());
}

TEST(Task, ExecveatFailsWithEloopIfSymlinkNoFollow) {
  test_helper::ScopedTempDir temp_dir;

  const std::string exit_zero_path = test_helper::GetTestResourcePath("exit_zero");
  const std::string symlink_path = temp_dir.path() + "/symlink_to_bin";
  ASSERT_THAT(symlink(exit_zero_path.c_str(), symlink_path.c_str()), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    char* const argv[] = {const_cast<char*>(symlink_path.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(
        syscall(SYS_execveat, AT_FDCWD, symlink_path.c_str(), argv, envp, AT_SYMLINK_NOFOLLOW),
        SyscallFailsWithErrno(ELOOP));
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ExecveSucceedsWithExecutablePtInterp) {
  test_helper::ScopedTempDir temp_dir;

  const std::string dynamic_linker_path = test_helper::GetSystemDynamicLinkerPath();
  ASSERT_FALSE(dynamic_linker_path.empty());
  std::string dynamic_linker_content;
  ASSERT_TRUE(files::ReadFileToString(dynamic_linker_path, &dynamic_linker_content))
      << dynamic_linker_path;

  const std::string interp_path = temp_dir.path() + "/test_interp";
  ASSERT_TRUE(files::WriteFile(interp_path, dynamic_linker_content));
  ASSERT_THAT(chmod(interp_path.c_str(), 0755), SyscallSucceeds());

  const std::string custom_pt_interp_child =
      files::AbsolutePath(test_helper::GetTestResourcePath("custom_pt_interp_child"));

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(chdir(temp_dir.path().c_str()));
    char* const argv[] = {const_cast<char*>(custom_pt_interp_child.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(custom_pt_interp_child.c_str(), argv, envp), SyscallSucceeds());
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST(Task, ExecveSucceedsForExecuteOnlyBinary) {
  test_helper::ScopedTempDir temp_dir;

  const std::string exit_zero_path = test_helper::GetTestResourcePath("exit_zero");
  std::string binary_content;
  ASSERT_TRUE(files::ReadFileToString(exit_zero_path, &binary_content)) << exit_zero_path;

  const std::string exec_only_path = temp_dir.path() + "/exec_only_bin";
  ASSERT_TRUE(files::WriteFile(exec_only_path, binary_content));
  // Mode 0111: execute permission only, no read permission.
  ASSERT_THAT(chmod(exec_only_path.c_str(), 0111), SyscallSucceeds());

  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    char* const argv[] = {const_cast<char*>(exec_only_path.c_str()), nullptr};
    char* const envp[] = {nullptr};
    EXPECT_THAT(execve(exec_only_path.c_str(), argv, envp), SyscallSucceeds());
  });
  ASSERT_TRUE(helper.WaitForChildren());
}

TEST_F(CloneAndExecTest, ExecveAsRootRequiresAnyExecutableBitSet) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "test requires root privileges";
  }

  test_helper::ScopedTempDir temp_dir;
  const std::string binary_path = temp_dir.path() + "/binary";
  char* const argv[] = {const_cast<char*>(binary_path.c_str()), nullptr};
  char* const envp[] = {nullptr};

  const std::string exit_zero_path = test_helper::GetTestResourcePath("exit_zero");
  std::string exit_zero_content;
  ASSERT_TRUE(files::ReadFileToString(exit_zero_path, &exit_zero_content));
  ASSERT_TRUE(files::WriteFile(binary_path, exit_zero_content));

  test_helper::ForkHelper helper;

  // Root cannot execute a file with no eXecutable bits set.
  ASSERT_THAT(chmod(binary_path.c_str(), 0644), SyscallSucceeds());
  helper.RunInForkedProcess(
      [&] { EXPECT_THAT(execve(binary_path.c_str(), argv, envp), SyscallFailsWithErrno(EACCES)); });
  ASSERT_TRUE(helper.WaitForChildren());

  // Root can execute a file so long as any of user, group, other can execute it.
  ASSERT_THAT(chmod(binary_path.c_str(), 0744), SyscallSucceeds());
  helper.RunInForkedProcess(
      [&] { EXPECT_THAT(execve(binary_path.c_str(), argv, envp), SyscallSucceeds()); });
  ASSERT_TRUE(helper.WaitForChildren());

  ASSERT_THAT(chmod(binary_path.c_str(), 0654), SyscallSucceeds());
  helper.RunInForkedProcess(
      [&] { EXPECT_THAT(execve(binary_path.c_str(), argv, envp), SyscallSucceeds()); });
  ASSERT_TRUE(helper.WaitForChildren());

  ASSERT_THAT(chmod(binary_path.c_str(), 0645), SyscallSucceeds());
  helper.RunInForkedProcess(
      [&] { EXPECT_THAT(execve(binary_path.c_str(), argv, envp), SyscallSucceeds()); });
  ASSERT_TRUE(helper.WaitForChildren());
}

class DumpableTest : public ::testing::Test {
 protected:
  void SetUp() {
    if (!test_helper::HasSysAdmin()) {
      GTEST_SKIP() << "test requires root privileges";
    }

    FILE* fp = fopen("/proc/sys/fs/suid_dumpable", "r");
    EXPECT_NE(fp, nullptr);
    // TODO(https://fxbug.dev/322874210): EXPECT that the read succeeds, do not
    // hard-code a value.
    if (fscanf(fp, "%d", &dumpable_default_) != 1) {
      dumpable_default_ = 0;
    }

    fclose(fp);
  }
  int dumpable_default_ = -1;
};

TEST_F(DumpableTest, DumpablePersistsAcrossFork) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([&] {
    SAFE_SYSCALL(prctl(PR_SET_DUMPABLE, 1));

    pid_t child_pid = SAFE_SYSCALL(fork());
    if (child_pid == 0) {
      EXPECT_EQ(prctl(PR_GET_DUMPABLE), 1);
      exit(testing::Test::HasFailure());
    }

    int status;
    SAFE_SYSCALL(waitpid(child_pid, &status, 0));
    EXPECT_TRUE(WIFEXITED(status));
    EXPECT_EQ(WEXITSTATUS(status), 0);
  });

  EXPECT_TRUE(helper.WaitForChildren());
}

TEST_F(DumpableTest, DumpableDropsOnUserChange) {
  for (size_t combination = 0b00000000; combination <= 0b11111111; combination++) {
    uid_t ruid, euid, suid, fuid;
    gid_t rgid, egid, sgid, fgid;

    ruid = (combination & 0b00000001) ? kUser1Uid : 0;
    euid = (combination & 0b00000010) ? kUser1Uid : 0;
    suid = (combination & 0b00000100) ? kUser1Uid : 0;
    fuid = (combination & 0b00001000) ? kUser1Uid : 0;
    rgid = (combination & 0b00010000) ? kUser1Gid : 0;
    egid = (combination & 0b00100000) ? kUser1Gid : 0;
    sgid = (combination & 0b01000000) ? kUser1Gid : 0;
    fgid = (combination & 0b10000000) ? kUser1Gid : 0;

    bool expected_change =
        (euid == kUser1Uid || egid == kUser1Gid || fuid == kUser1Uid || fgid == kUser1Gid);

    test_helper::ForkHelper helper;
    helper.RunInForkedProcess([&] {
      SCOPED_TRACE(fxl::StringPrintf("resfsuid: (%d, %d, %d, %d) resfsgid: (%d, %d, %d, %d)", ruid,
                                     euid, suid, fuid, rgid, egid, sgid, fgid));
      SAFE_SYSCALL(prctl(PR_SET_DUMPABLE, 1));

      SAFE_SYSCALL(setresgid(rgid, egid, sgid));
      SAFE_SYSCALL(setfsgid(fgid));
      SAFE_SYSCALL(setfsuid(fuid));
      SAFE_SYSCALL(setresuid(ruid, euid, suid));

      int dumpable = prctl(PR_GET_DUMPABLE);
      if (expected_change) {
        EXPECT_EQ(dumpable, dumpable_default_);
      } else {
        EXPECT_EQ(dumpable, 1);
      }
    });

    EXPECT_TRUE(helper.WaitForChildren());
  }
}

TEST(Task, SetgroupsWithoutCapSetgidFails) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "test requires root privileges";
  }

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([]() {
    // Dropping CAP_SETGID should be sufficient to cause setgroups() to fail.
    test_helper::UnsetCapability(CAP_SETGID);

    gid_t groups[] = {kUser1Gid};
    EXPECT_THAT(setgroups(1, groups), SyscallFailsWithErrno(EPERM));
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(Task, SetgroupsWithCapSetgidSucceeds) {
  if (!test_helper::HasSysAdmin()) {
    GTEST_SKIP() << "test requires root privileges";
  }

  test_helper::ForkHelper helper;

  helper.RunInForkedProcess([]() {
    // Change to non-root user while preserving capabilities, and re-enable CAP_SETGID.
    SAFE_SYSCALL(prctl(PR_SET_KEEPCAPS, 1));
    SAFE_SYSCALL(setresuid(kUser1Uid, kUser1Uid, kUser1Uid));
    test_helper::SetCapabilityEffective(CAP_SETGID);

    gid_t groups[] = {kUser1Gid};
    EXPECT_THAT(setgroups(1, groups), SyscallSucceeds());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}

TEST(Task, SetpgidRejoinProcessGroup) {
  test_helper::ForkHelper helper;
  helper.RunInForkedProcess([]() {
    pid_t p_pid = getpid();
    // Process P becomes leader of its own process group.
    ASSERT_THAT(setpgid(0, 0), SyscallSucceeds());
    EXPECT_EQ(getpgid(0), p_pid);

    test_helper::ForkHelper child_helper;
    test_helper::EventFdSem c_ready(0);
    test_helper::EventFdSem d_ready(0);
    test_helper::EventFdSem stop_c(0);
    test_helper::EventFdSem stop_d(0);

    // Child C stays in process group P.
    child_helper.RunInForkedProcess([&]() {
      EXPECT_EQ(getpgid(0), p_pid);
      c_ready.Notify(1);
      stop_c.Wait();
    });

    // Child D creates its own process group, leaving process group P.
    pid_t d_pid = child_helper.RunInForkedProcess([&]() {
      ASSERT_THAT(setpgid(0, 0), SyscallSucceeds());
      EXPECT_EQ(getpgid(0), getpid());
      d_ready.Notify(1);
      stop_d.Wait();
    });

    c_ready.Wait();
    d_ready.Wait();

    // P joins D's process group. Process group P is still alive because C is a member.
    ASSERT_THAT(setpgid(0, d_pid), SyscallSucceeds());
    EXPECT_EQ(getpgid(0), d_pid);

    // P rejoins its original process group.
    ASSERT_THAT(setpgid(0, 0), SyscallSucceeds());
    EXPECT_EQ(getpgid(0), p_pid);

    // Release children and wait for them to finish.
    stop_c.Notify(1);
    stop_d.Notify(1);
    EXPECT_TRUE(child_helper.WaitForChildren());
  });
  EXPECT_TRUE(helper.WaitForChildren());
}
