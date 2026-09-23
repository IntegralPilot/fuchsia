// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>
#include <lib/captive-thread/captive-thread.h>
#include <lib/captive-thread/registers.h>
#include <lib/captive-thread/testing/matchers.h>

#include <concepts>
#include <numeric>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "../asm-linkage.h"
#include "fuchsia/jmp_buf.h"
#include "mock-corrupted.h"
#include "src/lib/unwinder/fuchsia.h"
#include "src/lib/unwinder/unwind.h"
#include "src/setjmp/longjmp.h"
#include "src/setjmp/setjmp.h"

extern "C" void TrapWithAllRegs();

namespace LIBC_NAMESPACE_DECL {

// These resolve to the actual PC address of the start and end of longjmp.
// The start is used as the entry point as well as check the PC stays in range.
extern arch::AsmLabel kLongjmpStart LIBC_ASM_LINKAGE_DECLARE(longjmp_start);
extern arch::AsmLabel kLongjmpEnd LIBC_ASM_LINKAGE_DECLARE(longjmp_end);

// In sanitized builds of longjmp, it has a call into the sanitizer runtime.
[[gnu::weak]] extern arch::AsmLabel kLongjmpSanitizerCall
    LIBC_ASM_LINKAGE_DECLARE(longjmp_sanitizer_call);
[[gnu::weak]] extern arch::AsmLabel kLongjmpSanitizerReturn
    LIBC_ASM_LINKAGE_DECLARE(longjmp_sanitizer_return);

extern arch::AsmLabel kSetjmpStart LIBC_ASM_LINKAGE_DECLARE(setjmp_start);
extern arch::AsmLabel kSetjmpEnd LIBC_ASM_LINKAGE_DECLARE(setjmp_end);

extern "C" arch::AsmLabel __ehdr_start;  // NOLINT(bugprone-reserved-identifier)

namespace {

using captive_thread::ArgumentRegisters;
using captive_thread::CallSavedRegisters;
using captive_thread::CaptiveThread;
using captive_thread::SpecialRegisters;
using captive_thread::TemporaryRegisters;
using captive_thread::testing::AsContainer;
using captive_thread::testing::GotException;
using captive_thread::testing::GotSingleStep;
using captive_thread::testing::HasRegisters;
using captive_thread::testing::Hex;
using captive_thread::testing::IsPageFault;
using captive_thread::testing::IsTrap;
using captive_thread::testing::RegistersAsContainer;
using captive_thread::testing::RegistersContainer;
using captive_thread::testing::WithFp;
using captive_thread::testing::WithPc;
using captive_thread::testing::WithReturnAddress;
using captive_thread::testing::WithReturnValue;
using captive_thread::testing::WithScsp;
using captive_thread::testing::WithSp;
using ::testing::AllOf;
using ::testing::Contains;
using ::testing::ElementsAreArray;
using ::testing::IsSupersetOf;
using ::testing::Le;
using ::testing::Lt;
using ::testing::Ne;
using ::testing::Not;
using ::testing::Optional;
using ::testing::UnorderedElementsAreArray;
using ::testing::Value;

constexpr unwinder::RegisterID kUnwinderReturnValueRegister =
#ifdef __aarch64__
    unwinder::RegisterID::kArm64_x0
#elifdef __riscv
    unwinder::RegisterID::kRiscv64_a0
#elifdef __x86_64__
    unwinder::RegisterID::kX64_rax
#endif
    ;

const Hex kBaseAddress = arch::kAsmLabelAddress<__ehdr_start>;

const Hex kSetjmpEntry = arch::kAsmLabelAddress<kSetjmpStart>;
const Hex kLongjmpEntry = arch::kAsmLabelAddress<kLongjmpStart>;

constexpr int64_t AddressDistance(uint64_t a, uint64_t b) {
  return std::abs(std::bit_cast<int64_t>(a) - std::bit_cast<int64_t>(b));
}

auto AddressDistanceFrom(uint64_t addr, auto matcher) {
  return ::testing::DistanceFrom(addr, AddressDistance, matcher);
}

// This produces a matcher for a PC value in [Start, End).
template <arch::AsmLabel& Start, arch::AsmLabel& End>
auto InAsmLabel() {
  return AddressDistanceFrom(arch::kAsmLabelAddress<Start>, Lt(arch::kAsmLabelSize<Start, End>));
}

auto InSetjmp() { return InAsmLabel<kSetjmpStart, kSetjmpEnd>(); }
auto InLongjmp() { return InAsmLabel<kLongjmpStart, kLongjmpEnd>(); }

auto Called0() { return AllOf(GotException(IsPageFault(0)), HasRegisters(WithPc(0))); }

constexpr size_t kJmpBufSizeBytes = sizeof(jmp_buf);

// Views the jmp_buf as a span of bytes or words for examination.
template <typename T>
  requires(kJmpBufSizeBytes % sizeof(T) == 0)
auto JmpBufAs(jmp_buf buf) {
  constexpr size_t N = kJmpBufSizeBytes / sizeof(T);
  return std::span<T, N>{reinterpret_cast<T*>(buf), N};
}

// How an `int` argument or return value appears in a register.
constexpr uint64_t IntInRegister(int val) {
  return std::bit_cast<uint64_t>(static_cast<int64_t>(val));
}

// Get the registers as a container of pairs.
auto UnwinderRegs(const unwinder::Registers& regs) {
  return std::vector{std::from_range, regs.WithNames(captive_thread::testing::AsHex)};
}

std::optional<uint64_t> UnwinderPc(const unwinder::Registers& regs) {
  uint64_t pc;
  auto result = regs.GetPC(pc);
  EXPECT_TRUE(result.ok()) << result.msg();
  return result.ok() ? std::make_optional(pc) : std::nullopt;
}

// Return a function pointer like the argument, but actually nullptr without
// the compiler knowing that it is.
template <typename R, typename... Args>
auto NullOf(R (*)(Args...)) {
  using Unchecked = R(Args...) [[clang::cfi_unchecked_callee]];
  Unchecked* ptr = nullptr;
  __asm__("" : "+r"(ptr));
  return ptr;
}

// The test thread calls longjmp through this wrapper just to ensure it moves
// the stack pointer around from where it was when setjmp was called.
struct Waste {
  std::byte space[128];
};
[[gnu::noinline]] void DoTestLongjmp(jmp_buf buf, int val, Waste& waste) {
  __asm__ volatile("" : "=m"(waste));
  NullOf(LIBC_NAMESPACE::longjmp)(buf, val);
}

// The captive thread runs this function.  The test single-steps through it
// checking on things.  Once it sees longjmp return, the thread is never
// allowed to complete.
void LongjmpTestThread(jmp_buf buf, int val) {
  auto* volatile save_buf = buf;
  volatile int save_val = val;
  if (NullOf(LIBC_NAMESPACE::setjmp)(buf) == 0) {
    Waste waste;
    DoTestLongjmp(save_buf, save_val, waste);
    GTEST_FAIL() << "longjmp returned!";
  }
  GTEST_FAIL() << "should not be reached";
}

class MockCorrupted {
 public:
  MOCK_METHOD(void, Corrupted, (jmp_buf));
};

class LibcSetjmpTests : public ::testing::Test {
 public:
  static void SetUpTestSuite() {
    // This is the test LIBC_NAMESPACE, so no startup code has touched the
    // globals.  Get some nonzero values into the manglers so they actually do
    // something.  Use known values for convenient identification in debugging,
    // and to keep the test deterministic.  Use different values for each word
    // since setjmp should use each one for a different purpose.
    std::ranges::iota(gJmpBufManglers, uint64_t{0xdeadbeef} << 32);
  }

  void SetUp() override {
    EXPECT_EQ(gMockLongjmpCorrupted, nullptr);
    gMockLongjmpCorrupted = [this](jmp_buf env) { mock_.Corrupted(env); };
  }

  void TearDown() override {
    gMockLongjmpCorrupted = nullptr;
    ::testing::Mock::VerifyAndClear(&mock_);
  }

  auto& mock_corrupted() { return mock_; }

 private:
  ::testing::StrictMock<MockCorrupted> mock_;
};

class Unwinder {
 public:
  unwinder::Registers operator()(const zx_thread_state_general_regs_t& regs) {
    auto unwinder_regs = unwinder::FromFuchsiaRegisters(regs);
    std::vector frames = unwinder_.Unwind(&memory_, unwinder_regs, 2);
    EXPECT_EQ(frames.size(), 2u) << " unwinding from " << UnwinderRegs(unwinder_regs);
    if (frames.size() != 2) {
      return unwinder::Registers{unwinder_regs.arch()};
    }

    // The first frame is just the input registers, inside setjmp or longjmp.
    EXPECT_FALSE(frames.front().pc_is_return_address)
        << " unwinding from " << UnwinderRegs(unwinder_regs);
    EXPECT_THAT(UnwinderRegs(frames.front().regs),
                UnorderedElementsAreArray(UnwinderRegs(unwinder_regs)))
        << " unwinding from " << UnwinderRegs(unwinder_regs);

    // The second frame is the setjmp or longjmp caller.
    EXPECT_EQ(frames.back().trust, unwinder::Frame::Trust::kCFI)
        << " unwinding from " << UnwinderRegs(unwinder_regs);
    EXPECT_FALSE(frames.back().is_signal_frame) << UnwinderRegs(frames.back().regs);
    return std::move(frames.back().regs);
  }

 private:
  unwinder::LocalMemory memory_;

  // The unwinder should only see frames in this test executable itself, so it
  // only needs the one module.
  const std::array<unwinder::Module, 1> modules_{{{
      arch::kAsmLabelAddress<__ehdr_start>,
      &memory_,
      unwinder::Module::AddressMode::kProcess,
  }}};
  unwinder::Unwinder unwinder_{modules_};
};

// When the unwinder is told that all registers are .cfi_same_value, it will
// still only report a subset of registers mentioned in the CFI.  Use a
// reference function that does just that, to collect the set of registers the
// unwinders actually claims to know.
std::set<std::string> UnwinderSupportedRegs() {
  CaptiveThread thread{TrapWithAllRegs};
  EXPECT_THAT(thread.WaitForException(), GotException());
  zx::result regs = thread.Registers();
  EXPECT_THAT(regs, HasRegisters());
  if (regs.is_error()) {
    return {};
  }
  Unwinder unwind_from;
  return std::set<std::string>{
      std::from_range,
      std::views::keys(UnwinderRegs(unwind_from(*regs))),
  };
}

template <uint32_t Tag>
constexpr auto Filler() {
  return [next = uint64_t{Tag} << 32] mutable { return next++; };
}

// Fill the registers that don't affect longjmp with unusual values so e.g.
// unwinding a register as zero doesn't happen to look correct, and no
// call-saved register happens to contain a "special" value.  This only
// preserves the fixed registers and the first two argument registers.
zx_thread_state_general_regs_t FillRegs(zx_thread_state_general_regs_t regs) {
  auto gen = Filler<0xfeedface>();
  std::ranges::generate(CallSavedRegisters(regs), gen);
  std::ranges::generate(TemporaryRegisters(regs), gen);
  std::ranges::generate(std::views::drop(ArgumentRegisters(regs), 2), gen);
  return regs;
}

auto FixedRegsMatcher(const zx_thread_state_general_regs_t& regs) {
  auto r = SpecialRegisters(regs);
  return AllOf(WithReturnAddress(r.ra()), WithScsp(r.scsp()), WithFp(r.fp()),
               // The stack can move around a little around the sanitizer call.
               WithSp(AllOf(Le(r.sp()), AddressDistanceFrom(r.sp(), Lt(128)))));
}

auto FixedRegisters(const zx_thread_state_general_regs_t& regs [[clang::lifetimebound]]) {
  // Ignore the PC, which is first.
  return std::views::drop(SpecialRegisters(regs), 1);
}

// The PC can be warped to the real entry point before resuming the thread.
zx_thread_state_general_regs_t WarpTo(CaptiveThread& thread, uint64_t pc) {
  zx_thread_state_general_regs_t regs = *thread.Registers();
  SpecialRegisters(regs).pc() = pc;
  return regs;
}

TEST_F(LibcSetjmpTests, UnwindLongjmp) {
  const auto in_setjmp = HasRegisters(WithPc(InSetjmp()));
  const auto in_longjmp = HasRegisters(WithPc(InLongjmp()));

  static const std::set<std::string> kUnwinderRegs = UnwinderSupportedRegs();
  constexpr auto keep_only_unwinder_regs = [](auto& regs) {
    std::erase_if(regs, [](const auto& reg) { return !kUnwinderRegs.contains(reg.first); });
  };

  // Start the test thread.
  jmp_buf buf;
  constexpr int kReturnValue = 0xd00d;
  CaptiveThread thread{LongjmpTestThread, auto(buf), kReturnValue};

  auto current_regs = [&thread] {
    return captive_thread::testing::RegistersAsContainer(*thread.Registers());
  };

  // It will first attempt to call setjmp but actually call nullptr (0).
  ASSERT_THAT(thread.WaitForException(), Called0());

  // Fill the jmp_buf with known garbage before setjmp is called.
  constexpr uint8_t kFillByte = 0xbb;
  memset(buf, kFillByte, sizeof(buf));

  // Warp the thread as if it had actually called setjmp.
  auto setjmp_entry_regs = WarpTo(thread, kSetjmpEntry);

  // Fill the call-saved registers with distinct but boring values.  These will
  // be saved in the jmp_buf, so make sure their values can't collide with any
  // special values that should not leak into the jmp_buf.  But after setjmp
  // returns, its caller will want its original values back; save them first.
  const std::vector<uint64_t> setjmp_call_saved{
      std::from_range,
      CallSavedRegisters(setjmp_entry_regs),
  };
  std::ranges::generate(CallSavedRegisters(setjmp_entry_regs), Filler<0xbadf00d>());

  zx::result result = thread.SetRegisters(setjmp_entry_regs);
  ASSERT_TRUE(result.is_ok()) << result.status_string();

  // Make a matcher to verify that the fixed registers are never changed.
  const auto setjmp_fixed_regs = FixedRegsMatcher(setjmp_entry_regs);

  // Unwind from the setjmp entry point and note the return address.
  Unwinder unwind_from;
  const auto setjmp_caller_regs = unwind_from(setjmp_entry_regs);
  const auto setjmp_caller_pc = UnwinderPc(setjmp_caller_regs);
  ASSERT_THAT(setjmp_caller_pc, Optional(Not(InSetjmp())));
  const uint64_t setjmp_retaddr = *setjmp_caller_pc;
  const auto setjmp_unwind_regs = UnwinderRegs(setjmp_caller_regs);

  // Single-step into setjmp.  Unwinding should recover the same caller
  // registers at every step.
  {
    decltype(auto(setjmp_unwind_regs)) caller_regs;
    int step = 0;
    do {
      EXPECT_THAT(*thread.Registers(), setjmp_fixed_regs)
          << "at setjmp instruction #" << step << " " << current_regs() << " vs setjmp caller "
          << setjmp_unwind_regs;

      ++step;
      ASSERT_THAT(thread.StepToException(), AllOf(GotSingleStep(), HasRegisters()))
          << "at setjmp instruction #" << step << " " << current_regs() << " vs setjmp caller "
          << setjmp_unwind_regs;
      caller_regs = UnwinderRegs(unwind_from(*thread.Registers()));
    } while (Value(caller_regs, UnorderedElementsAreArray(setjmp_unwind_regs)));

    if (Value(thread.Registers(), in_setjmp)) {
      // If it's still in setjmp but the registers aren't the same, the only
      // valid situation is that setjmp's incoming value of the return value
      // register was still recoverable up to this point, but no longer is.  So
      // it should be reporting exactly the same registers as before except for
      // that one.
      auto expected_regs = setjmp_caller_regs;
      expected_regs.Unset(kUnwinderReturnValueRegister);
      const auto expected_unwind_regs = UnwinderRegs(expected_regs);
      do {
        EXPECT_THAT(*thread.Registers(), setjmp_fixed_regs)
            << "at setjmp instruction #" << step << " " << current_regs() << " vs setjmp caller "
            << setjmp_unwind_regs;
        EXPECT_THAT(caller_regs, UnorderedElementsAreArray(expected_unwind_regs))
            << "at setjmp instruction #" << step << " " << current_regs() << " vs setjmp caller "
            << setjmp_unwind_regs;

        ++step;
        ASSERT_THAT(thread.StepToException(), AllOf(GotSingleStep(), HasRegisters()))
            << "at setjmp instruction #" << step << " " << current_regs();
        caller_regs = UnwinderRegs(unwind_from(*thread.Registers()));
      } while (Value(thread.Registers(), in_setjmp));
    }
  }

  // It's no longer inside setjmp, so setjmp must have just returned.
  ASSERT_THAT(thread.Registers(), HasRegisters(WithPc(setjmp_retaddr))) << current_regs();

  // Once setjmp has returned, the jmp_buf is filled in.

  // At least some bytes must have been changed from the initial garbage.
  EXPECT_THAT(JmpBufAs<uint8_t>(buf), Contains(Ne(kFillByte)));

  // Various important values should not be visible unmangled in the jmp_buf.
  const auto buf_regs = JmpBufAs<Hex>(buf);
  EXPECT_THAT(buf_regs, Not(Contains(SpecialRegisters(setjmp_entry_regs).sp())))
      << "vs setjmp entry " << RegistersAsContainer(setjmp_entry_regs);
  EXPECT_THAT(buf_regs, Not(Contains(SpecialRegisters(setjmp_entry_regs).fp())))
      << "vs setjmp entry " << RegistersAsContainer(setjmp_entry_regs);
  EXPECT_THAT(buf_regs, Not(Contains(setjmp_retaddr)))
      << "vs setjmp entry " << RegistersAsContainer(setjmp_entry_regs);

  // Put the setjmp caller's call-saved registers back in place.
  {
    zx_thread_state_general_regs_t regs = *thread.Registers();
    std::ranges::copy(setjmp_call_saved, CallSavedRegisters(regs).begin());
    zx::result result = thread.SetRegisters(regs);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }

  // Let it continue from the setjmp return address.
  thread.ResolveException();

  // It will next attempt to call longjmp but again actually call PC 0.
  ASSERT_THAT(thread.WaitForException(), Called0());

  // Warp the thread as if it had actually called longjmp.
  const auto longjmp_entry_regs = FillRegs(WarpTo(thread, kLongjmpEntry));
  result = thread.SetRegisters(longjmp_entry_regs);
  ASSERT_TRUE(result.is_ok()) << result.status_string();
  {
    // Its argument registers should already be set.
    const std::vector arg_regs{
        std::from_range,
        std::views::take(ArgumentRegisters(longjmp_entry_regs), 2),
    };
    EXPECT_EQ(arg_regs[0], reinterpret_cast<uintptr_t>(auto(buf)));
    EXPECT_EQ(arg_regs[1], IntInRegister(kReturnValue));
  }

  // Unwind from the longjmp entry point and note the return address.
  const auto longjmp_caller_regs = unwind_from(longjmp_entry_regs);
  const auto longjmp_caller_pc = UnwinderPc(longjmp_caller_regs);
  ASSERT_THAT(longjmp_caller_pc, Optional(Not(InLongjmp())));
  const uint64_t longjmp_retaddr = *longjmp_caller_pc;

  // Make a matcher to check for when the fixed registers change.
  const auto longjmp_fixed_regs = FixedRegsMatcher(longjmp_entry_regs);

  // When there is a call into the sanitizer runtime, we'll step through it
  // until we get back to the return address in longjmp.
  uint64_t step_over_from = arch::kAsmLabelAddress<kLongjmpSanitizerCall>;
  uint64_t step_over_until = arch::kAsmLabelAddress<kLongjmpSanitizerReturn>;
  ASSERT_EQ(step_over_from == 0, step_over_until == 0);
  if (step_over_from != 0) {
    ASSERT_GT(step_over_until, step_over_from);
  }

  // Unwinding should recover the same longjmp caller registers at every step,
  // up until it starts recovering the setjmp caller registers instead.
  {
    const auto longjmp_unwind_regs = UnwinderRegs(longjmp_caller_regs);
    int step = 0;
    while (true) {
      ++step;
      ASSERT_THAT(thread.StepToException(), AllOf(GotSingleStep(), HasRegisters()))
          << "at longjmp instruction #" << step << " " << current_regs();
      uint64_t pc = SpecialRegisters(*thread.Registers()).pc();
      if (step_over_from != 0 && pc == step_over_from) {
        // Step into the call into the sanitizer runtime.  The call might
        // actually be more than one instruction, so just step until the PC
        // leaves longjmp.
        step_over_from = 0;  // Can't happen twice!
        int step_over = 0;
        do {
          ASSERT_THAT(thread.StepToException(), GotSingleStep())
              << "at longjmp instruction #" << step << " => sanitizer call step #" << step_over
              << " " << current_regs();
          ++step, ++step_over;
        } while (Value(thread.Registers(), in_longjmp));

        // We could just single-step all the way through the sanitizer runtime
        // until the return address at step_over_until.  But on branch-and-link
        // machines, it's easy to use the jump-to-0 trick to effectively set a
        // breakpoint on the return instead.  When using CaptiveThread's fake
        // single-step, it's not really safe to "step" through arbitrary code
        // since it does actual breakpoint insertion and so causes breakpoint
        // traps in any thread reaching that spot.  The setjmp / longjmp code
        // under test is not used outside the test, so no other thread is going
        // to hit those breakpoint spots.
        auto regs = *thread.Registers();
        auto skip_call = [&]<typename T>(T&& ra) {
          if constexpr (std::same_as<uint64_t&, T>) {
            ASSERT_EQ(ra, step_over_until);
            ra = 0;
            zx::result result = thread.SetRegisters(regs);
            ASSERT_TRUE(result.is_ok()) << result.status_string();
            thread.ResolveException();
            ASSERT_THAT(thread.WaitForException(), Called0());
            regs = *thread.Registers();  // Fetch the new registers.
            ASSERT_EQ(ra, 0u);           // Still references the regs field.
            SpecialRegisters(regs).pc() = pc = ra = step_over_until;
            result = thread.SetRegisters(regs);
            ASSERT_TRUE(result.is_ok()) << result.status_string();
          }
        };
        skip_call(SpecialRegisters(regs).ra());

        // Step all the way through the sanitizer runtime until the return.
        while (pc != step_over_until) {
          ASSERT_THAT(thread.StepToException(), AllOf(GotSingleStep(), HasRegisters()))
              << "at longjmp instruction #" << step << " => sanitizer call step #" << step_over
              << " " << current_regs();
          ++step, ++step_over;
          pc = SpecialRegisters(*thread.Registers()).pc();
        }

        // It may take another instruction after the return before all the
        // normal invariants hold again, so step again before checking things.
        ASSERT_THAT(thread.StepToException(), AllOf(GotSingleStep(), in_longjmp))
            << "at longjmp instruction #" << step << " => sanitizer call step #" << step_over << " "
            << current_regs();
      }
      ASSERT_THAT(pc, InLongjmp()) << "at longjmp instruction #" << step << " " << current_regs();
      auto caller = unwind_from(*thread.Registers());
      if (UnwinderPc(caller) != longjmp_retaddr) {
        // It's no longer reporting longjmp's caller.
        break;
      }
      auto caller_regs = UnwinderRegs(caller);
      EXPECT_THAT(caller_regs, UnorderedElementsAreArray(longjmp_unwind_regs))
          << "at longjmp instruction #" << step << " " << current_regs();
      EXPECT_THAT(*thread.Registers(), longjmp_fixed_regs)
          << "at longjmp instruction #" << step << " " << current_regs() << " vs longjmp entry "
          << RegistersAsContainer(longjmp_entry_regs);
    }

    // We should still be in longjmp when the unwound return address changes.
    ASSERT_THAT(thread.Registers(), in_longjmp) << current_regs();

    // If the caller is no longer longjmp's caller, then it should be setjmp's.
    auto longjmp_setjmp_regs = unwind_from(*thread.Registers());
    auto longjmp_setjmp_unwind_regs = UnwinderRegs(longjmp_setjmp_regs);
    keep_only_unwinder_regs(longjmp_setjmp_unwind_regs);
    EXPECT_THAT(longjmp_setjmp_unwind_regs, IsSupersetOf(setjmp_unwind_regs))
        << "at longjmp instruction #" << step << " " << current_regs() << " vs setjmp entry "
        << RegistersAsContainer(setjmp_entry_regs);

    // At this point, each fixed register has the longjmp entry value until a
    // step that switches it to the setjmp entry value.  Each can validly (and
    // independently) still be the longjmp value at each step, but once it
    // changes then it must be into the setjmp value and then not change again.
    const std::vector<Hex> setjmp_entry_fixed{
        std::from_range,
        FixedRegisters(setjmp_entry_regs),
    };
    std::vector<std::optional<Hex>> longjmp_entry_fixed{
        std::from_range,
        FixedRegisters(longjmp_entry_regs),
    };

    // Unwinding should recover the same caller registers at every step.
    decltype(longjmp_setjmp_unwind_regs) caller_regs;
    do {
      // Each fixed register whose longjmp value is no longer seen is instead
      // checked against the setjmp value.
      const std::vector<Hex> current_fixed_regs{
          std::from_range,
          FixedRegisters(*thread.Registers()),
      };
      std::vector<Hex> expected_fixed_regs;
      for (auto&& [longjmp_entry, setjmp_entry, current] :
           std::views::zip(longjmp_entry_fixed, setjmp_entry_fixed, current_fixed_regs)) {
        if (current != longjmp_entry) {
          longjmp_entry.reset();
          expected_fixed_regs.push_back(setjmp_entry);
        } else {
          expected_fixed_regs.push_back(current);
        }
      }
      EXPECT_THAT(current_fixed_regs, ElementsAreArray(expected_fixed_regs))
          << "at longjmp instruction #" << step << " " << current_regs() << " vs longjmp entry "
          << RegistersAsContainer(longjmp_entry_regs) << " vs setjmp entry "
          << RegistersAsContainer(setjmp_entry_regs);

      ++step;
      ASSERT_THAT(thread.StepToException(), AllOf(GotSingleStep(), HasRegisters()))
          << "at longjmp instruction #" << step << " " << current_regs() << " vs setjmp entry "
          << RegistersAsContainer(setjmp_entry_regs);
      caller_regs = UnwinderRegs(unwind_from(*thread.Registers()));
      keep_only_unwinder_regs(caller_regs);
    } while (Value(caller_regs, IsSupersetOf(longjmp_setjmp_unwind_regs)));

    // If it's still in longjmp but the registers aren't the same, the only
    // valid situation is that setjmp's incoming value of the return value
    // register was still recoverable up to this point, but no longer is.  So
    // it should be reporting exactly the same registers as before except for
    // that one.
    longjmp_setjmp_regs.Unset(kUnwinderReturnValueRegister);
    longjmp_setjmp_unwind_regs = UnwinderRegs(longjmp_setjmp_regs);
    keep_only_unwinder_regs(longjmp_setjmp_unwind_regs);
    while (Value(thread.Registers(), in_longjmp)) {
      EXPECT_THAT(caller_regs, IsSupersetOf(longjmp_setjmp_unwind_regs))
          << "at longjmp instruction #" << step << " " << current_regs() << " vs setjmp entry "
          << setjmp_unwind_regs << " vs longjmp entry " << RegistersAsContainer(longjmp_entry_regs);

      ++step;
      ASSERT_THAT(thread.StepToException(), AllOf(GotSingleStep(), HasRegisters()))
          << "at longjmp instruction #" << step << " " << current_regs() << " vs setjmp entry "
          << RegistersAsContainer(setjmp_entry_regs) << " vs longjmp entry "
          << RegistersAsContainer(longjmp_entry_regs);
      caller_regs = UnwinderRegs(unwind_from(*thread.Registers()));
      keep_only_unwinder_regs(caller_regs);
    }
  }

  // The original setjmp caller's registers have been restored as they could be
  // unwound on entry to setjmp, except for the return value register.
  auto setjmp_return_regs = setjmp_caller_regs;
  setjmp_return_regs.Unset(kUnwinderReturnValueRegister);
  const auto setjmp_return_unwind_regs = UnwinderRegs(setjmp_return_regs);

  EXPECT_THAT(thread.Registers(),
              HasRegisters(AllOf(          //
                  WithPc(setjmp_retaddr),  //
                  WithReturnValue(IntInRegister(kReturnValue)),
                  AsContainer(IsSupersetOf(setjmp_return_unwind_regs)))))
      << current_regs();
}

TEST_F(LibcSetjmpTests, LongjmpCorrupted) {
  constexpr uint64_t kCallSavedValue = 0xd00d'feed'face'f00d;
  constexpr uint64_t kBogusValue = 0x2'bad'd00d'0'f00d;

  // Start the test thread.
  jmp_buf buf;
  CaptiveThread thread{LongjmpTestThread, auto(buf), 0};

  auto current_regs = [&thread] -> RegistersContainer {
    if (auto regs = thread.Registers(); regs.is_ok()) {
      return RegistersAsContainer(*regs);
    }
    return {};
  };

  // It will first attempt to call setjmp but actually call nullptr (0).
  ASSERT_THAT(thread.WaitForException(), Called0());

  // Warp the thread as if it had actually called setjmp.
  auto setjmp_entry_regs = WarpTo(thread, kSetjmpEntry);

  // Also Set one normal call-saved register to a known value so that will be
  // captured in the jmp_buf.
  const uint64_t saved =
      std::exchange(CallSavedRegisters(setjmp_entry_regs).front(), kCallSavedValue);

  // Step through setjmp starting with the modified registers.
  {
    zx::result result = thread.SetRegisters(setjmp_entry_regs);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }
  do {
    zx::result result = thread.ResolveExceptionSingleStep();
    ASSERT_TRUE(result.is_ok()) << result.status_string();
    ASSERT_THAT(thread.WaitForException(), GotSingleStep())
        << current_regs() << " from base " << kBaseAddress << " vs setjmp @ " << kSetjmpEntry;
  } while (Value(thread.Registers(), HasRegisters(WithPc(InSetjmp()))));

  // Now that setjmp has returned, its caller might care about that register.
  // Put back the original value; setjmp used the the other value forced in.
  {
    auto regs = thread.Registers();
    ASSERT_THAT(regs, HasRegisters());
    EXPECT_EQ(std::exchange(CallSavedRegisters(*regs).front(), saved), kCallSavedValue);
    zx::result result = thread.SetRegisters(*regs);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }

  // Now let it continue from after the setjmp call.  The thread doesn't notice
  // that anything happened, but now we know the jmp_buf it captured contains
  // kCallSavedValue in some slot.
  thread.ResolveException();

  // It will next attempt to call longjmp but again actually call PC 0.
  ASSERT_THAT(thread.WaitForException(), Called0())
      << current_regs() << " from base " << kBaseAddress << " vs setjmp @ " << kSetjmpEntry
      << " longjmp @" << kLongjmpEntry;

  // Warp the thread as if it had actually called longjmp.
  {
    const auto longjmp_entry_regs = WarpTo(thread, kLongjmpEntry);
    zx::result result = thread.SetRegisters(longjmp_entry_regs);
    ASSERT_TRUE(result.is_ok()) << result.status_string();
  }

  // That normal call-saved register should be found verbatim in the jmp_buf.
  std::span buf_regs = JmpBufAs<uint64_t>(buf);
  ASSERT_THAT(buf_regs, Contains(kCallSavedValue));

  // Change it.  That should corrupt the buffer, but would not make any
  // non-checking version of longjmp fail to return properly since the
  // essential registers in the jmp_buf are intact.
  auto it = std::ranges::find(buf_regs, kCallSavedValue);
  *it = kBogusValue;

  // The panic path should call into the mock.  The expectation will fail at
  // the end of the test if it was never called after the thread is destroyed.
  EXPECT_CALL(mock_corrupted(), Corrupted(buf));

  // Let it continue to enter longjmp with the corrupted jmp_buf.
  thread.ResolveException();
  ASSERT_THAT(thread.WaitForException(), GotException(IsTrap()))
      << current_regs() << " from base " << kBaseAddress << " vs setjmp @ " << kSetjmpEntry
      << " longjmp @" << kLongjmpEntry;
}

}  // namespace
}  // namespace LIBC_NAMESPACE_DECL
