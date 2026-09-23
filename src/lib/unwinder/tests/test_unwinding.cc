// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <algorithm>
#include <cinttypes>
#include <cstdint>
#include <utility>

#include <gtest/gtest.h>

#include "sdk/lib/fit/include/lib/fit/function.h"
#include "src/developer/debug/shared/message_loop.h"
#include "src/developer/debug/shared/test_with_loop.h"
#include "src/lib/fxl/strings/string_printf.h"
#include "src/lib/unwinder/unwind_local.h"

namespace unwinder {

// Declarations that might be put in a header file in the future.
int CfiOnly(std::function<int()>);
int FpOnly(std::function<int()>);
int ScsOnly(std::function<int()>);

namespace {

// Use global variables to avoid captures in lambdas.
std::vector<Frame> frames;
std::vector<uint64_t> expected_pcs;
uint64_t test_start_pc;

// Call a sequence of functions recursively and record the return addresses in |expected_pcs|
// At the end, unwind the stack using |UnwindLocal()| and save to |frames|. For example,
// `call_sequence(FpOnly, ScsOnly)` expands to `FpOnly([]() { ScsOnly([]() { UnwindLocal() }); });`
template <typename... FN>
int call_sequence() {
  frames = UnwindLocal();
  return 0;
}

template <typename F1, typename... FN>
int call_sequence(F1 f1, FN... fn) {
  return f1([=]() {
    int res = call_sequence(fn...);
    expected_pcs.push_back(reinterpret_cast<uint64_t>(__builtin_return_address(0)));
    return res;
  });
}

// Check whether |expected_pcs| is a subsequence of |frames|.
void check_stack() {
  auto expected_it = expected_pcs.begin();
  for (auto& frame : frames) {
    if (uint64_t pc; frame.regs.GetPC(pc).ok() && pc == *expected_it) {
      expected_it++;
      if (expected_it == expected_pcs.end()) {
        break;
      }
    }
  }
  if (expected_it != expected_pcs.end()) {
    std::string msg = "Expect to find the following frames:\n";
    for (auto& pc : expected_pcs) {
      msg += fxl::StringPrintf("%#" PRIx64 "\n", pc);
    }
    msg += "But actually get:\n";
    for (auto& frame : frames) {
      msg += frame.Describe() + "\n";
    }
    FAIL() << msg;
  }
}

template <typename F1, typename... FN>
[[gnu::noinline]] int TestStart(F1 f1, FN... fn) {
  frames.clear();
  expected_pcs.clear();
  test_start_pc = 0;

  int res = call_sequence(f1, fn...);
  test_start_pc = reinterpret_cast<uint64_t>(__builtin_return_address(0));
  expected_pcs.push_back(test_start_pc);
  return res;
}

size_t GetTestStartFrameIndex() {
  size_t start_idx = 0;
  for (auto& frame : frames) {
    if (uint64_t pc; frame.regs.GetPC(pc).ok() && pc == test_start_pc) {
      return start_idx;
    }
    start_idx++;
  }
  return -1;
}

// It should be possible to interoperate between FP unwinder and CFI unwinder.
TEST(Unwinder, HybridUnwinding) {
  int res = TestStart(FpOnly, FpOnly, CfiOnly, FpOnly);

  ASSERT_EQ(res, 31);

  // FpOnly, CfiOnly, FpOnly, FpOnly could be the first four frames before the TestStart().
  EXPECT_GE(GetTestStartFrameIndex(), 4ul);

  check_stack();
}

#if __has_feature(shadow_call_stack)
TEST(Unwinder, Scs) {
  // SCS unwinder cannot be combined with other unwinders, e.g.
  // (CFI -> SCS means unwinding using CFI first, then using SCS.)
  //  * CFI -> SCS will work, because CFI also recovers x18.
  //  * FP -> SCS won't work, because FP doesn't recover x18.
  //  * SCS -> FP and SCS -> CFI won't work, because SCS only recovers PC and x18.
  int res = TestStart(ScsOnly, ScsOnly, ScsOnly, CfiOnly);

  ASSERT_EQ(res, 301);
  // CfiOnly, ScsOnly, ScsOnly, ScsOnly could be the first four frames before the TestStart().
  ASSERT_GE(GetTestStartFrameIndex(), 4ul);

  check_stack();
}
#endif

// Provides a fake implementation of AsyncMemory::Delegate where |FetchMemoryRanges| does nothing
// and |ReadBytes| reads from local process memory.
class FakeAsyncMemory : public AsyncMemory::Delegate {
 public:
  void FetchMemoryRanges(std::vector<std::pair<uint64_t, uint32_t>> ranges,
                         fit::callback<void()> done) override {
    // Note that we cannot actually post tasks to the message loop because we're running in the same
    // process as the stack we're trying to unwind. If we post a task to the loop, then the stack
    // will get modified out from under us while we're unwinding.
    done();
  }

  Error ReadBytes(uint64_t addr, uint64_t size, void* dst) override {
    return memory_.ReadBytes(addr, size, dst);
  }

  // TODO(https://fxbug.dev/484331669): Make this test work without requiring this to be
  // synchronous.
  void PostTask(fit::callback<void()> callback) override {
    ASSERT_TRUE(callback);
    callback();
  }

  Memory* GetLocalMemory() { return &memory_; }

 private:
  LocalMemory memory_;
};

// This class helps manage the lifetime of the |on_done| callback which is fed through to the
// AsyncUnwinder at the bottom of the callstack created via |async_call_sequence|, as well as
// simplify the introduction of the (cross-platform) message loop implementation we're borrowing
// from zxdb.
class AsyncUnwind : public debug::TestWithLoop {
 public:
  template <typename... Fn>
  int async_call_sequence() {
    UnwindLocalAsync(memory_.GetLocalMemory(), &memory_, [=](std::vector<Frame> frames) mutable {
      loop().PostTask(
          FROM_HERE, [this, frames = std::move(frames)]() mutable { on_done_(std::move(frames)); });
    });

    return 0;
  }

  template <typename F1, typename... FN>
  int async_call_sequence(F1 f1, FN... fn) {
    return f1([=]() mutable {
      int res = async_call_sequence(fn...);
      expected_pcs.push_back(reinterpret_cast<uint64_t>(__builtin_return_address(0)));
      return res;
    });
  }

  template <typename F1, typename... FN>
  int AsyncTestStart(size_t expected_frames, F1 f1, FN... fn) {
    frames.clear();
    expected_pcs.clear();
    test_start_pc = 0;

    on_done_ = [=](std::vector<Frame> result) mutable {
      frames = std::move(result);

      ASSERT_GE(GetTestStartFrameIndex(), expected_frames);
      check_stack();

      loop().QuitNow();
    };

    int res = async_call_sequence(f1, fn...);

    test_start_pc = reinterpret_cast<uint64_t>(__builtin_return_address(0));
    expected_pcs.push_back(test_start_pc);
    return res;
  }

 private:
  FakeAsyncMemory memory_;
  fit::callback<void(std::vector<Frame>)> on_done_;
};

TEST_F(AsyncUnwind, CanUnwindCfi) {
  int res = AsyncTestStart(4, CfiOnly, CfiOnly, CfiOnly, CfiOnly);

  // At this point the call stack should be set up for us, but nothing posted to the message loop
  // has been executed yet. Since we set up the call stack synchronously, we should be able to
  // assert that the functions specified above were called.
  ASSERT_EQ(res, 4);

  // Now we run the message loop to consume the unwinder results.
  loop().Run();
}

// TEST_F(AsyncUnwind, CanUnwindCfiMany) {
//   int res =
//       AsyncTestStart(7, CfiOnly, CfiOnly, CfiOnly, CfiOnly, CfiOnly, CfiOnly, CfiOnly);

//   // At this point the call stack should be set up for us, but nothing posted to the message loop
//   // has been executed yet. Since we set up the call stack synchronously, we should be able to
//   // assert that the functions specified above were called.
//   ASSERT_EQ(res, 7);

//   // Now we run the message loop to consume the unwinder results.
//   loop().Run();
// }

TEST_F(AsyncUnwind, UnwindFp) {
  int res = AsyncTestStart(4, FpOnly, FpOnly, FpOnly, FpOnly);

  ASSERT_EQ(res, 40);

  loop().Run();
}

// CFI unwinding works well when intermixed with frame pointer unwinding, since the stack pointer
// register is also restored. That means when we unwind a frame with frame pointers, we can and
// should continue to try to unwind with CFI next.
TEST_F(AsyncUnwind, CfiFpHybridFromCfi) {
  int res = AsyncTestStart(4, CfiOnly, FpOnly, CfiOnly, CfiOnly);

  ASSERT_EQ(res, 13);

  // Now we run the message loop to consume the unwinder results.
  loop().Run();
}

TEST_F(AsyncUnwind, CfiFpHybridFromFp) {
  int res = AsyncTestStart(4, FpOnly, FpOnly, CfiOnly, FpOnly);

  // This matches the synchronous unwinder above.
  ASSERT_EQ(res, 31);

  // Now we run the message loop to consume the unwinder results.
  loop().Run();
}

}  // namespace

}  // namespace unwinder
