// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/captive-thread/captive-thread.h>
#include <lib/captive-thread/registers.h>
#include <lib/captive-thread/testing/matchers.h>
#include <lib/elfldltl/machine.h>
#include <zircon/syscalls.h>

#include <cstdint>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

namespace {

using ::testing::_;
using ::testing::AllOf;
using ::testing::Eq;
using ::testing::Field;
using ::testing::IsSupersetOf;
using ::testing::Ne;
using ::testing::Not;
using ::testing::Pair;

std::string_view TestName() {
  return ::testing::UnitTest::GetInstance()->current_test_info()->name();
}

// Do a bit cast and also hide the value from the compiler so it cannot be
// constant-folded in callers.
template <typename T>
  requires(sizeof(T) <= sizeof(uintptr_t))
T LaunderAs(auto value)
  requires(sizeof(value) <= sizeof(uintptr_t))
{
  T result;
  __asm__("" : "=r"(result) : "0"(value));
  return result;
}

// This always crashes and never returns, but the compiler doesn't know that.
// In fact, it can return if the PC is advanced past the trap instruction.
void Crash() {
  // This is the same instruction that __builtin_trap() emits.  But the
  // compiler knows that __builtin_trap() cannot return and so it may decide to
  // move its instruction out of the straight-ahead path even if it's hidden
  // that the trap path is always taken.
  __asm__ volatile(
#ifdef __aarch64__
      "brk #1"
#elifdef __riscv
      "unimp"
#elifdef __x86_64__
      "ud2"
#endif
  );
}

// This is called just to demonstrate actual call and return instructions,
// since it cannot be inlined.
[[gnu::noinline]] void CallAndReturn() {}

template <uintptr_t Address = 0>
[[clang::no_sanitize("all")]] void PageFault() {
  *LaunderAs<volatile int*>(Address) = 0;
}

TEST(CaptiveThreadTests, Destroy) {
  {
    // Destroyed immediately after construction.
    captive_thread::CaptiveThread thread([] {});
  }
}

TEST(CaptiveThreadTests, ExplicitJoin) {
  captive_thread::CaptiveThread thread([] {});
  EXPECT_FALSE(thread.Joined());
  thread.ForceJoin();
  EXPECT_TRUE(thread.Joined());
}

TEST(CaptiveThreadTests, BlockUntilSuccess) {
  bool ran = false;
  {
    captive_thread::CaptiveThread thread([&ran] { ran = true; });
    thread.BlockUntilSuccess();
    EXPECT_TRUE(thread.Joined());
  }
  EXPECT_TRUE(ran);
}

TEST(CaptiveThreadTests, CrashAndJoin) {
  captive_thread::CaptiveThread thread{Crash};
  thread.ForceJoin();
}

TEST(CaptiveThreadTests, WaitForException) {
  bool started = false, finished = false;
  {
    captive_thread::CaptiveThread thread{[&started, &finished] {
      started = true;
      Crash();
      finished = true;  // Should not be reached, but the compiler won't know.
    }};
    zx::result result = thread.WaitForException();
    EXPECT_TRUE(result.is_ok()) << result.status_value();
    EXPECT_FALSE(thread.Joined());
    EXPECT_TRUE(thread.InException());
    EXPECT_TRUE(thread.IsStopped());
  }
  EXPECT_TRUE(started);
  EXPECT_FALSE(finished);
}

TEST(CaptiveThreadTests, ExceptionReport) {
  captive_thread::CaptiveThread thread{Crash};
  zx::result result = thread.WaitForException();
  EXPECT_TRUE(result.is_ok()) << result.status_value();
  EXPECT_THAT(thread.ExceptionReport(),
              Optional(Field(&zx_exception_report_t::header,
                             Field(&zx_exception_header_t::type, captive_thread::kTrapException))));
}

TEST(CaptiveThreadTests, Suspend) {
  captive_thread::CaptiveThread thread{[] {
    while (true) {
      zx::nanosleep(zx::time::infinite());
    }
  }};
  zx::result suspend_result = thread.Suspend();
  ASSERT_TRUE(suspend_result.is_ok()) << suspend_result.status_value();
  EXPECT_TRUE(thread.InSuspend());
  EXPECT_TRUE(thread.IsStopped());
  EXPECT_FALSE(thread.InException());
  zx::result wait_result = thread.WaitForStop();
  EXPECT_TRUE(wait_result.is_ok()) << wait_result.status_value();
}

TEST(CaptiveThreadTests, SuspendVsException) {
  captive_thread::CaptiveThread thread{Crash};

  // It might already have an exception pending, but we haven't checked yet, so
  // Suspend() will always start a suspension.
  zx::result suspend_result = thread.Suspend();
  ASSERT_TRUE(suspend_result.is_ok()) << suspend_result.status_value();
  EXPECT_TRUE(thread.InSuspend());
  EXPECT_TRUE(thread.IsStopped());

  // This might get either the exception or the suspension first.
  zx::result wait_result = thread.WaitForStop();
  EXPECT_TRUE(wait_result.is_ok()) << wait_result.status_value();

  // Either way, the suspension is still in place.
  EXPECT_TRUE(thread.IsStopped());
  EXPECT_TRUE(thread.InSuspend());
}

TEST(CaptiveThreadTests, Matchers) {
  using captive_thread::testing::IsPageFault;
  {
    captive_thread::CaptiveThread thread{Crash};
    EXPECT_THAT(thread.WaitForException(), GotException(captive_thread::testing::IsTrap()));
    EXPECT_THAT(thread.WaitForException(), captive_thread::testing::IsTrap());
  }
  {
    captive_thread::CaptiveThread thread{PageFault};
    EXPECT_THAT(thread.WaitForException(), GotException(IsPageFault()));
    EXPECT_THAT(thread.WaitForException(), IsPageFault());
    EXPECT_THAT(thread.WaitForException(), GotException(IsPageFault(0)));
  }
  {
    captive_thread::CaptiveThread thread{PageFault<0x123>};
    EXPECT_THAT(thread.WaitForException(), IsPageFault(Ne(0)));
    EXPECT_THAT(thread.WaitForException(), IsPageFault(Eq(0x123)));
  }
  {
    // Basic death test usage.
    using captive_thread::CaptiveThread;
    using captive_thread::testing::GotException;
    EXPECT_THAT(CaptiveThread(Crash).WaitForException(), GotException());
    EXPECT_THAT(CaptiveThread([] {}).WaitForException(), Not(GotException()));
  }
}

TEST(CaptiveThreadTests, Registers) {
  captive_thread::CaptiveThread thread{Crash};
  EXPECT_THAT(thread.WaitForException(), captive_thread::testing::HasRegisters());
  EXPECT_THAT(thread.WaitForException(), HasRegisters(captive_thread::testing::WithPc(Ne(0))));
  EXPECT_THAT(thread.Registers(), HasRegisters(captive_thread::testing::WithTp(Ne(0))));

  auto regs = thread.Registers();
  EXPECT_TRUE(regs.is_ok()) << regs.status_string();
  if (regs.is_ok()) {
    EXPECT_THAT(*regs, captive_thread::testing::WithSp(Ne(0)));

    // The names differ, but there are pairs for each special register.
    auto special = captive_thread::SpecialRegisters(*regs);
    std::vector matchers{
        Pair(_, special.pc()),
        Pair(_, special.sp()),
        Pair(_, special.tp()),
    };
    if (std::optional<uint64_t> ra = special.ra()) {
      matchers.push_back(Pair(_, *ra));
    }
    if (std::optional<uint64_t> scsp = special.scsp()) {
      matchers.push_back(Pair(_, *scsp));
    }
    auto special_regs = captive_thread::testing::AsContainer(IsSupersetOf(matchers));
    EXPECT_THAT(thread.Registers(), HasRegisters(special_regs));
  }
}

TEST(CaptiveThreadTests, ResolveException) {
  bool started = true, finished = false;
  captive_thread::CaptiveThread thread{[&started, &finished] {
    started = true;
    Crash();
    finished = true;
  }};
  ASSERT_THAT(thread.WaitForException(),
              AllOf(captive_thread::testing::IsTrap(),  //
                    captive_thread::testing::HasRegisters()));
  EXPECT_TRUE(started);
  EXPECT_FALSE(finished);
  auto regs = *thread.Registers();
  captive_thread::SpecialRegisters(regs).pc() += captive_thread::kTrapInstructionSize;
  zx::result result = thread.SetRegisters(regs);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  thread.ResolveException();
  EXPECT_FALSE(thread.IsStopped());
  ASSERT_THAT(thread.WaitForException(), Not(captive_thread::testing::GotException()))
      << ::testing::PrintToString(captive_thread::testing::RegistersAsContainer(thread));
  ASSERT_FALSE(thread.IsStopped());
  thread.BlockUntilSuccess();
  EXPECT_TRUE(finished);
}

TEST(CaptiveThreadTests, SingleStep) {
  using captive_thread::testing::GotSingleStep;

  bool started = true, finished = false;
  captive_thread::CaptiveThread thread{[&started, &finished] {
    started = true;
    Crash();
    // Do some stuff that's interesting to single-step through.
    if (LaunderAs<bool>(true)) {  // Conditional branch, taken.
      CallAndReturn();
    }
    if (LaunderAs<bool>(false)) {  // Conditional branch, not taken.
      Crash();
    }
    finished = true;
  }};
  ASSERT_THAT(thread.WaitForException(),
              AllOf(captive_thread::testing::IsTrap(),  //
                    captive_thread::testing::HasRegisters()));
  EXPECT_TRUE(started);
  EXPECT_FALSE(finished);
  auto regs = *thread.Registers();
  captive_thread::SpecialRegisters(regs).pc() += captive_thread::kTrapInstructionSize;
  zx::result result = thread.SetRegisters(regs);
  ASSERT_TRUE(result.is_ok()) << result.status_string();

  zx::result step = thread.ResolveExceptionSingleStep();
  ASSERT_TRUE(step.is_ok()) << step.status_string();

  EXPECT_THAT(thread.WaitForException(), GotSingleStep());
  EXPECT_FALSE(finished);  // It needs more than one instruction to get there.

  // Single-step until it's done the store.
  int steps = 1;
  do {
    zx::result step = thread.ResolveExceptionSingleStep();
    ASSERT_TRUE(step.is_ok()) << step.status_value() << " at step " << steps;
    ++steps;
  } while (::testing::Value(thread.WaitForException(), GotSingleStep()) && !finished);

  // If the loop broke for another reason, the assert will report why.
  EXPECT_TRUE(finished) << "after step " << steps;
  ASSERT_THAT(thread.WaitForException(), GotSingleStep()) << "after step " << steps;

  // Now let it run to completion without getting another single-step trap.
  thread.ResolveException();
  thread.BlockUntilSuccess();
}

TEST(CaptiveThreadTests, CreateRaw) {
  using captive_thread::ArgumentRegisters;
  using captive_thread::testing::GotException;
  using captive_thread::testing::HasRegisters;
  using captive_thread::testing::IsPageFault;
  using captive_thread::testing::WithPc;
  using captive_thread::testing::WithSp;
  using ::testing::AllOf;
  using ::testing::ElementsAre;

  constexpr captive_thread::CaptiveThread::Raw kRegs{
      .pc = 0x1230,  // Canonical but guaranteed to fault.
      .sp = 0x4560,  // Canonical and ABI-aligned but guaranteed to fault.

      // Arbitrary values.
      .arg1 = 0xdeadbeef12345678,
      .arg2 = 0xdeadbeef87654321,
  };

  zx::result result = captive_thread::CaptiveThread::CreateRaw(TestName(), kRegs);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  std::unique_ptr thread = *std::move(result);
  ASSERT_TRUE(thread);

  ASSERT_THAT(thread->WaitForException(),
              AllOf(GotException(IsPageFault(0x1230)),
                    HasRegisters(AllOf(WithPc(kRegs.pc), WithSp(kRegs.sp)))));

  const std::vector<uint64_t> args{
      std::from_range,
      std::views::take(ArgumentRegisters(*thread->Registers()), 2),
  };
  EXPECT_THAT(args, ElementsAre(kRegs.arg1, kRegs.arg2));
}

constexpr size_t kRawStackSize = 1024;  // Plenty to call into the vDSO.

class alignas(elfldltl::AbiTraits<>::kStackAlignment<size_t>) RawStack
    : public std::array<std::byte, kRawStackSize> {
 public:
  constexpr RawStack() = default;

  uintptr_t sp() const {
    return elfldltl::AbiTraits<>::InitialStackPointer(reinterpret_cast<uintptr_t>(data()), size());
  }

  // Registers to start running on this stack in the vDSO.
  template <auto* Call = &zx_thread_exit>
  captive_thread::CaptiveThread::Raw call_regs(uint64_t arg1 = 0, uint64_t arg2 = 0) const {
    return {
        .pc = reinterpret_cast<uintptr_t>(Call),
        .sp = sp(),
        .arg1 = arg1,
        .arg2 = arg2,
    };
  }
};

TEST(CaptiveThreadTests, RawExit) {
  using captive_thread::testing::GotException;
  using ::testing::Not;

  // Start running directly in the vDSO with enough stack for zx_thread_exit().
  std::unique_ptr stack = std::make_unique<RawStack>();
  zx::result result = captive_thread::CaptiveThread::CreateRaw(TestName(), stack->call_regs());
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  std::unique_ptr thread = *std::move(result);
  ASSERT_TRUE(thread);

  ASSERT_THAT(thread->WaitForException(), Not(GotException()));

  thread->BlockUntilSuccess();
}

TEST(CaptiveThreadTests, CreateRawSuspended) {
  using captive_thread::ArgumentRegisters;
  using captive_thread::testing::GotException;
  using captive_thread::testing::HasRegisters;
  using captive_thread::testing::WithPc;
  using captive_thread::testing::WithSp;
  using ::testing::AllOf;
  using ::testing::ElementsAre;
  using ::testing::Not;

  // Start running directly in the vDSO with enough stack for zx_thread_exit().
  std::unique_ptr stack = std::make_unique<RawStack>();
  const auto regs = stack->call_regs(0xdeadbeef1234, 0xdeadbeef5678);
  zx::result result = captive_thread::CaptiveThread::CreateRaw(TestName(), regs, true);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  std::unique_ptr thread = *std::move(result);
  ASSERT_TRUE(thread);

  ASSERT_TRUE(thread->InSuspend());

  ASSERT_THAT(thread->WaitForStop(), Not(GotException()));

  ASSERT_TRUE(thread->IsStopped());
  ASSERT_FALSE(thread->InException());
  ASSERT_FALSE(thread->ExceptionReport());

  ASSERT_THAT(thread->Registers(), HasRegisters(AllOf(WithPc(regs.pc), WithSp(regs.sp))));

  const std::vector<uint64_t> args{
      std::from_range,
      std::views::take(ArgumentRegisters(*thread->Registers()), 2),
  };
  EXPECT_THAT(args, ElementsAre(regs.arg1, regs.arg2));

  thread->Resume();

  thread->BlockUntilSuccess();
}

TEST(CaptiveThreadTests, StartRaw) {
  using captive_thread::ArgumentRegisters;
  using captive_thread::testing::GotException;
  using captive_thread::testing::IsPageFault;
  using ::testing::AllOf;

  std::string_view name = TestName();
  zx::thread thread_handle;
  zx::result create = zx::make_result(zx::thread::create(
      *zx::process::self(), name.data(), static_cast<uint32_t>(name.size()), 0, &thread_handle));
  ASSERT_TRUE(create.is_ok()) << create.status_string();

  constexpr uint64_t kBadPc = 0x1230;
  zx::result result =
      captive_thread::CaptiveThread::StartRaw(std::move(thread_handle), {.pc = kBadPc});
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  std::unique_ptr thread = *std::move(result);
  ASSERT_TRUE(thread);

  EXPECT_THAT(thread->WaitForException(), GotException(IsPageFault(kBadPc)));
}

TEST(CaptiveThreadTests, StartRawSuspended) {
  using captive_thread::ArgumentRegisters;
  using captive_thread::testing::GotException;
  using captive_thread::testing::HasRegisters;
  using captive_thread::testing::IsPageFault;
  using captive_thread::testing::WithPc;
  using captive_thread::testing::WithSp;
  using ::testing::AllOf;

  std::string_view name = TestName();
  zx::thread thread_handle;
  zx::result create = zx::make_result(zx::thread::create(
      *zx::process::self(), name.data(), static_cast<uint32_t>(name.size()), 0, &thread_handle));
  ASSERT_TRUE(create.is_ok()) << create.status_string();

  zx::suspend_token token;
  zx::result suspend = zx::make_result(thread_handle.suspend(&token));
  ASSERT_TRUE(suspend.is_ok()) << suspend.status_string();

  constexpr uint64_t kBadPc = 0x1230;
  zx::result result = captive_thread::CaptiveThread::StartRaw(  //
      std::move(thread_handle), {.pc = kBadPc}, std::move(token));
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  std::unique_ptr thread = *std::move(result);
  ASSERT_TRUE(thread);

  ASSERT_TRUE(thread->InSuspend());

  EXPECT_THAT(thread->WaitForStop(), Not(GotException()));
  EXPECT_THAT(thread->Registers(), HasRegisters(AllOf(WithPc(kBadPc), WithSp(0))));

  thread->Resume();

  EXPECT_THAT(thread->WaitForException(), GotException(IsPageFault(kBadPc)));
}

}  // namespace
