// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fcntl.h>
#include <lib/fit/defer.h>
#include <sys/mman.h>
#include <sys/syscall.h>
#include <unistd.h>

#include <string>

#include <fbl/unique_fd.h>
#include <gtest/gtest.h>

#include "src/lib/files/file.h"
#include "src/starnix/tests/selinux/userspace/util.h"
#include "src/starnix/tests/syscalls/cpp/syscall_matchers.h"
#include "src/starnix/tests/syscalls/cpp/test_helper.h"

extern std::string DoPrePolicyLoadWork() { return "mprotect_policy"; }

namespace {

constexpr int kReadIndex = 0;
constexpr int kWriteIndex = 1;

void *GetCurrentStackPage() {
  const long pagesize = sysconf(_SC_PAGESIZE);
  int stack_variable;
  return reinterpret_cast<void *>(((unsigned long)&stack_variable) & ~(pagesize - 1));
}

}  // namespace

/// Check that `execmem` allows making a MAP_STACK mapping executable.
TEST(MProtectTest, ExecMemWorksForMapStack) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  long pagesize = sysconf(_SC_PAGESIZE);
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:mprotect_execmem_test_t:s0", [&] {
    auto mapping = test_helper::ScopedMMap::MMap(nullptr, pagesize, PROT_NONE,
                                                 MAP_STACK | MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    ASSERT_TRUE(mapping.is_ok()) << mapping.error_value();
    auto addr = mapping->mapping();
    auto result = mprotect(addr, pagesize, PROT_EXEC);
#if defined(__riscv)
    // TODO(https://fxbug.dev/418975186): Fix mprotect returning -1 on RISC-V
    EXPECT_EQ(result, -1);
#else
    EXPECT_EQ(result, 0);
#endif
  }));
}

/// Check that `execmem` allows the initial thread to make a child thread's stack
/// executable.
TEST(MProtectTest, ExecStackOfChildThread) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:mprotect_execmem_test_t:s0", [&] {
    // Create the thread that will return the address of its stack
    struct ThreadArgs {
      // The pipe to notify the parent thread that the child has set the stack
      int stack_was_set_fd[2];
      // The pipe to notify the child thread that it can exit
      int child_can_exist_fd[2];
      void *stack = nullptr;
    };

    ThreadArgs thread_args;
    SAFE_SYSCALL(pipe(thread_args.stack_was_set_fd));
    SAFE_SYSCALL(pipe(thread_args.child_can_exist_fd));
    auto cleanup = fit::defer([&]() {
      SAFE_SYSCALL(close(thread_args.stack_was_set_fd[0]));
      SAFE_SYSCALL(close(thread_args.stack_was_set_fd[1]));
      SAFE_SYSCALL(close(thread_args.child_can_exist_fd[0]));
      SAFE_SYSCALL(close(thread_args.child_can_exist_fd[1]));
    });

    pthread_t thread;
    auto thread_lambda = [](void *ptr) -> void * {
      ThreadArgs *args = reinterpret_cast<ThreadArgs *>(ptr);
      args->stack = GetCurrentStackPage();

      // Signal to the parent that the stack address is set
      char ready_signal = 'R';
      SAFE_SYSCALL(write(args->stack_was_set_fd[kWriteIndex], &ready_signal, sizeof(ready_signal)));

      // Wait for the parent to signal that it's done with mprotect
      char exit_signal = 0;
      SAFE_SYSCALL(read(args->child_can_exist_fd[kReadIndex], &exit_signal, sizeof(exit_signal)));
      EXPECT_EQ(exit_signal, 'E');
      return nullptr;
    };

    pthread_create(&thread, nullptr, +thread_lambda, &thread_args);

    // Wait until the child thread has set the stack and signaled us
    unsigned char ready_ack = 0;
    SAFE_SYSCALL(read(thread_args.stack_was_set_fd[kReadIndex], &ready_ack, sizeof(ready_ack)));
    ASSERT_EQ(ready_ack, 'R');

    long pagesize = sysconf(_SC_PAGESIZE);
    int result = mprotect(thread_args.stack, pagesize, PROT_READ | PROT_WRITE | PROT_EXEC);
    EXPECT_EQ(result, 0);

    // Signal child thread to finish
    char exit_signal = 'E';
    SAFE_SYSCALL(
        write(thread_args.child_can_exist_fd[kWriteIndex], &exit_signal, sizeof(exit_signal)));

    pthread_join(thread, nullptr);
  }));
}

#if defined(__x86_64__)  // This test relies on x86 assembly

/// Check that changing the stackpointer before calling `mprotect` allows
/// bypassing the `execstack` permission.
TEST(MProtectTest, ExecStackAfterModifyingStackpointer) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:mprotect_execmem_test_t:s0", [&] {
    pthread_t thread;
    auto thread_lambda = [](void *ptr) -> void * {
      // Reserve an area of memory that the stack pointer will be made to point to
      const long pagesize = sysconf(_SC_PAGESIZE);
      auto mapping = test_helper::ScopedMMap::MMap(nullptr, pagesize, PROT_NONE,
                                                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
      EXPECT_TRUE(mapping.is_ok()) << mapping.error_value();
      void *temporary_stackpointer_value = mapping->mapping();

      // Get the pointer to the stack
      void *stack = GetCurrentStackPage();

      int prot = PROT_READ | PROT_WRITE | PROT_EXEC;
      int result = -1;

      // Temporarily change the stack pointer before calling `mprotect`. Pseudo code equivalent:
      //   %%rbx = %%rsp
      //   %%rsp = temporary_stackpointer_value
      //   result = mprotect(stack, pagesize, prot)
      //   %%rsp = %%rbx
      asm volatile(
          "movq %1, %%rdi\n"     // arg1: addr (rdi)
          "movq %2, %%rsi\n"     // arg2: len (rsi)
          "movl %3, %%edx\n"     // arg3: prot (rdx)
          "movl %4, %%eax\n"     // syscall number (eax)
          "movq %%rsp, %%rbx\n"  // store the stack pointer into rbx
          "movq %5, %%rsp\n"     // set the stack pointer to point to the temporary mapping
          "syscall\n"            // execute syscall
          "movq %%rbx, %%rsp\n"  // restore the stack pointer
          "movl %%eax, %0\n"     // move return value from eax to result
          : "=r"(result)
          : "r"(stack), "r"(pagesize), "r"(prot), "i"(__NR_mprotect),
            "r"(temporary_stackpointer_value)
          : "rdi", "rsi", "rdx", "rax", "memory", "cc"  // Clobbered registers
      );
      EXPECT_EQ(result, 0);
      return nullptr;
    };
    pthread_create(&thread, nullptr, +thread_lambda, nullptr);
    pthread_join(thread, nullptr);
  }));
}

#endif  // defined(__x86_64__)

/// Check that with `execmem` a signal handler can make executable the stack of its thread, but that
/// it can't make executable its own stack.
TEST(MProtectTest, ExecStackInSignal) {
  auto enforce = ScopedEnforcement::SetEnforcing();

  // Static variables used by the signal handler to communicate
  // with the outside world.
  static int fd[2];
  static void *initial_stack = nullptr;
  static int mprotect_signal_stack_result = -2;
  static int mprotect_initial_stack_result = -2;

  ASSERT_TRUE(RunSubprocessAs("test_u:test_r:mprotect_execmem_test_t:s0", [&] {
    // Create a child thread, because the initial thread of the process is special and doesn't
    // rely on the stack pointer to determine whether execstack is required.
    pthread_t thread;
    auto thread_lambda = [](void *ptr) -> void * {
      initial_stack = GetCurrentStackPage();
      SAFE_SYSCALL(pipe(fd));
      auto cleanup = fit::defer([&]() {
        SAFE_SYSCALL(close(fd[0]));
        SAFE_SYSCALL(close(fd[1]));
      });

      // Configure the stack to be used by the signals
      stack_t ss;
      memset(&ss, 0, sizeof(ss));
      auto mapping = test_helper::ScopedMMap::MMap(nullptr, SIGSTKSZ, PROT_READ | PROT_WRITE,
                                                   MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
      EXPECT_TRUE(mapping.is_ok()) << mapping.error_value();
      ss.ss_sp = mapping->mapping();
      ss.ss_size = SIGSTKSZ;
      ss.ss_flags = 0;
      SAFE_SYSCALL(sigaltstack(&ss, nullptr) == -1);

      // Configure the signal handler
      auto signal_handler = [](int signum) -> void {
        void *signal_stack = GetCurrentStackPage();
        long pagesize = sysconf(_SC_PAGESIZE);
        int prot = PROT_READ | PROT_WRITE | PROT_EXEC;
        mprotect_signal_stack_result = mprotect(signal_stack, pagesize, prot);
        mprotect_initial_stack_result = mprotect(initial_stack, pagesize, prot);
        char c = 1;
        write(fd[kWriteIndex], &c, 1);
      };
      struct sigaction sa;
      memset(&sa, 0, sizeof(sa));
      sa.sa_handler = +signal_handler;
      sa.sa_flags = SA_ONSTACK;
      SAFE_SYSCALL(sigaction(SIGUSR1, &sa, nullptr));

      // Send the signal
      pthread_kill(pthread_self(), SIGUSR1);

      // Wait for the signal handler to notify us that it has finished writing the results
      char boolean_result;
      SAFE_SYSCALL(read(fd[kReadIndex], &boolean_result, sizeof(boolean_result)));

      // Check the results
      EXPECT_EQ(mprotect_signal_stack_result, -1);
      EXPECT_EQ(mprotect_initial_stack_result, 0);

      return nullptr;
    };
    pthread_create(&thread, nullptr, +thread_lambda, nullptr);
    pthread_join(thread, nullptr);
  }));
}

struct ExecmodTestParam {
  // Whether the test is executed with the execmod permission.
  bool has_execmod_perm = false;
  // Whether the mapping is private or shared.
  bool private_mapping = false;
  // Whether the test should modify the mapped memory.
  bool modify_mapping = false;
  // Whether mprotect is expected to succeed or not.
  bool expect_success = false;
  std::string TestSuffixGenerator() const {
    std::string suffix;
    if (has_execmod_perm) {
      suffix += "ExecmodYes_";
    } else {
      suffix += "ExecmodNo_";
    }
    if (private_mapping) {
      suffix += "PrivateMapping_";
    } else {
      suffix += "SharedMapping_";
    }
    if (modify_mapping) {
      suffix += "Modification_";
    } else {
      suffix += "NoModification_";
    }
    if (expect_success) {
      suffix += "MprotectSuccess";
    } else {
      suffix += "MprotectFailure";
    }
    return suffix;
  }
};

class MProtectExecmod : public testing::TestWithParam<ExecmodTestParam> {};

// Checks that making executable a mapping of a file requires the
// `execmod` permission if the mapping was private and was modified.
TEST_P(MProtectExecmod, ExecMod) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  const auto param = MProtectExecmod::GetParam();
  auto label = param.has_execmod_perm ? "test_u:test_r:mprotect_execmod_yes_test_t:s0"
                                      : "test_u:test_r:mprotect_execmod_no_test_t:s0";
  ASSERT_TRUE(RunSubprocessAs(label, [&] {
    // Create a temporary file.
    test_helper::ScopedTempDir temp_dir;
    std::string file_path = temp_dir.path() + "/test_file";
    ASSERT_TRUE(files::WriteFile(file_path, "hello"));
    auto fd = fbl::unique_fd(open(file_path.c_str(), O_RDWR));
    ASSERT_TRUE(fd.is_valid());

    // Creates a mapping of the file, either private or shared.
    long pagesize = sysconf(_SC_PAGESIZE);
    auto flags = param.private_mapping ? MAP_PRIVATE : MAP_SHARED;
    auto mapping = test_helper::ScopedMMap::MMap(nullptr, pagesize, PROT_READ | PROT_WRITE, flags,
                                                 fd.get(), 0);
    ASSERT_TRUE(mapping.is_ok()) << mapping.error_value();

    // Modify the mapping.
    if (param.modify_mapping) {
      static_cast<char *>(mapping->mapping())[0] = 'j';
    }
    // Try to make the mapping executable.
    int result = mprotect(mapping->mapping(), pagesize, PROT_READ | PROT_EXEC);
    if (param.expect_success) {
      EXPECT_THAT(result, SyscallSucceeds());
    } else {
      EXPECT_THAT(result, SyscallFailsWithErrno(EACCES));
    }
  }));
}

const auto kExecmodSuccessFailureValues = ::testing::Values(
    // `execmod` required if a private mapping is modified.
    ExecmodTestParam{.has_execmod_perm = false,
                     .private_mapping = true,
                     .modify_mapping = true,
                     .expect_success = false},
    ExecmodTestParam{.has_execmod_perm = true,
                     .private_mapping = true,
                     .modify_mapping = true,
                     .expect_success = true},
    // `execmod` not required if the mapping is not private.
    ExecmodTestParam{.has_execmod_perm = false,
                     .private_mapping = false,
                     .modify_mapping = true,
                     .expect_success = true},
    // `execmod` not required if the mapping is not modified.
    ExecmodTestParam{.has_execmod_perm = false,
                     .private_mapping = true,
                     .modify_mapping = false,
                     .expect_success = true});
INSTANTIATE_TEST_SUITE_P(MProtectExecmod, MProtectExecmod, kExecmodSuccessFailureValues,
                         [](const testing::TestParamInfo<ExecmodTestParam> &info) {
                           return info.param.TestSuffixGenerator();
                         });

class MProtectExecstack : public testing::TestWithParam<std::pair<const char *, bool>> {};

// Test making the stack of the initial thread executable from *another* thread.
// This works with `execstack`, but does not work with `execmem`.
TEST_P(MProtectExecstack, MakeInitialStackExecFromOtherThread) {
  auto enforce = ScopedEnforcement::SetEnforcing();
  const auto [label, expect_success] = MProtectExecstack::GetParam();
  ASSERT_TRUE(RunSubprocessAs(label, [&] {
    struct ThreadArgs {
      // Stores a pointer to the stack of the initial thread
      void *stack = nullptr;
      // Stores the result of the call to the `mprotect` syscall
      std::atomic<int> mprotect_result = 0;
    };
    ThreadArgs thread_args;
    thread_args.stack = GetCurrentStackPage();
    pthread_t thread;
    auto thread_lambda = [](void *ptr) -> void * {
      // The child thread will call mprotect on the stack of the initial thread.
      ThreadArgs *args = reinterpret_cast<ThreadArgs *>(ptr);
      long pagesize = sysconf(_SC_PAGESIZE);
      args->mprotect_result = mprotect(args->stack, pagesize, PROT_READ | PROT_WRITE | PROT_EXEC);
      return nullptr;
    };
    pthread_create(&thread, nullptr, +thread_lambda, &thread_args);
    pthread_join(thread, nullptr);
    if (expect_success) {
      EXPECT_EQ(thread_args.mprotect_result, 0);
    } else {
      EXPECT_EQ(thread_args.mprotect_result, -1);
    }
  }));
}

const auto kExecstackSuccessFailureValues =
    ::testing::Values(std::make_pair("test_u:test_r:mprotect_execstack_test_t:s0", true),
                      std::make_pair("test_u:test_r:mprotect_execmem_test_t:s0", false));
INSTANTIATE_TEST_SUITE_P(MProtectExecstack, MProtectExecstack, kExecstackSuccessFailureValues);
