// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zx/process.h>
#include <lib/zx/thread.h>

#include <array>
#include <cstdint>
#include <string_view>

#include <zxtest/zxtest.h>

namespace {

// This checks every other expected starting register value (all zeros),
// then does `*ptr = value;` to check the two argument registers, and
// pushes kStackTopValue right below the starting SP value to check that.
extern "C" [[noreturn]] void InitialStateTestThread(uint64_t* ptr, uint64_t value);

constexpr uint64_t kStackTopValue = 0x1234567890abcdef;

constexpr uint64_t kArgumentValue = 0xabcdef1234567890;

alignas(16) std::array<uint64_t, 128> gThreadStack;

TEST(ThreadInitialStateTests, RegisterValues) {
  constexpr std::string_view kThreadName = "thread-initial-state-test-thread";
  zx::thread thread;
  ASSERT_OK(
      zx::thread::create(*zx::process::self(), kThreadName.data(), kThreadName.size(), 0, &thread));

  // Start the thread at the exact PC of the assembly entry point,
  // with the SP exactly at the top of the stack.  Thus every register
  // including PC, SP, and the two argument registers has a known value.

  const uintptr_t pc = reinterpret_cast<uintptr_t>(&InitialStateTestThread);
  const uintptr_t sp = reinterpret_cast<uintptr_t>(&(*std::end(gThreadStack)));
  uint64_t arg1_points_to = 0;
  const uint64_t arg1 = reinterpret_cast<uintptr_t>(&arg1_points_to);
  const uint64_t arg2 = kArgumentValue;

  ASSERT_OK(thread.start(pc, sp, arg1, arg2));

  constexpr zx_signals_t kWaitFor = ZX_THREAD_TERMINATED;
  zx_signals_t signals;
  ASSERT_OK(thread.wait_one(kWaitFor, zx::time::infinite(), &signals));
  EXPECT_TRUE(signals & ZX_THREAD_TERMINATED);

  // The thread exits no matter what.  The last things it does are write the
  // kStackTopValue, so we know the SP was where we put it; and store the
  // second argument register's value in the first argument register's pointer,
  // so we know both argument registers were what we passed to the syscall.

  EXPECT_EQ(arg1_points_to, kArgumentValue);
  EXPECT_EQ(gThreadStack.back(), kStackTopValue);
}

}  // namespace
