// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <pthread.h>
#include <threads.h>
#include <unistd.h>

#include <cstdlib>
#include <type_traits>

#include <gtest/gtest.h>

namespace {

using ::testing::ExitedWithCode;

constexpr int kExitCode = 42;

template <auto Join>
constexpr int kJoinValue = -1;

template <>
constexpr int kJoinValue<thrd_join> = 8675309;

template <>
[[maybe_unused]] void* const kJoinValue<pthread_join> = reinterpret_cast<void*>(uintptr_t{8675309});

// This handler should be called by exit(), but not by _exit().
// The thrd_exit() by the last thread implicitly calls exit(0).
void AtExitHandler() { _exit(kExitCode); }

template <typename Thread, auto Join, auto Success>
auto JoinMainThreadAndExit(void* arg) {
  using JoinValue = std::decay_t<decltype(kJoinValue<Join>)>;

  Thread main_thread = reinterpret_cast<Thread>(arg);

  // Wait until the main thread has called thrd_exit.
  JoinValue join_value{};
  EXPECT_EQ(Join(main_thread, &join_value), Success);
  EXPECT_EQ(join_value, kJoinValue<Join>);

  // When this function returns, this should be the last thread,
  // triggering process exit via exit(), which runs the atexit handler.
  return JoinValue{};
}

template <typename Thread, auto Create, auto Join, auto Success, auto Exit>
void ExitToSecondThread(Thread main_thread, auto&&... create_args) {
  ASSERT_EQ(atexit(AtExitHandler), 0);

  auto* thread_func = JoinMainThreadAndExit<Thread, Join, Success>;
  void* func_arg = reinterpret_cast<void*>(main_thread);
  Thread thread;
  ASSERT_EQ(Create(&thread, create_args..., thread_func, func_arg), Success);

  // After this, the JoinMainThreadAndExit thread should be the only thread.
  Exit(kJoinValue<Join>);

  // This part of the code should be unreachable.
  FAIL() << "thrd_exit did not exit the thread";
}

template <auto Exit, auto Join>
void ThreadExitFromMain() {
  ASSERT_EQ(atexit(AtExitHandler), 0);

  // This should do exit(0) regardless of the value given to thrd_exit().
  Exit(kJoinValue<Join>);

  // This part of the code should be unreachable.
  FAIL() << "thrd_exit did not exit the thread";
}

void PlainExit() {
  ASSERT_EQ(atexit(AtExitHandler), 0);

  exit(0);

  // This part of the code should be unreachable.
  FAIL() << "exit did not exit the process";
}

}  // namespace

TEST(LibcThreadTests, PlainExit) {
  // This basically just tests that the _exit via atexit in gtest death test
  // scenario works at all.
  ASSERT_EXIT(PlainExit(), ExitedWithCode(kExitCode), "");
}

TEST(LibcThreadTests, ThreadExitFromMain) {
  ASSERT_EXIT((ThreadExitFromMain<thrd_exit, thrd_join>()), ExitedWithCode(kExitCode), "");
}

TEST(LibcThreadTests, LastThreadCallsProcessExit) {
  ASSERT_EXIT(
      (ExitToSecondThread<thrd_t, thrd_create, thrd_join, thrd_success, thrd_exit>(thrd_current())),
      ExitedWithCode(kExitCode), "");
}

TEST(LibcPthreadTests, ThreadExitFromMain) {
  ASSERT_EXIT((ThreadExitFromMain<pthread_exit, pthread_join>()), ExitedWithCode(kExitCode), "");
}

TEST(LibcPthreadTests, LastThreadCallsProcessExit) {
  ASSERT_EXIT((ExitToSecondThread<pthread_t, pthread_create, pthread_join, 0, pthread_exit>(
                  thrd_current(), nullptr)),
              ExitedWithCode(kExitCode), "");
}
