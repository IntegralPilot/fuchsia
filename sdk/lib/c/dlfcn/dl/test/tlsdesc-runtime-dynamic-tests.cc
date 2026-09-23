// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/layout.h>
#include <lib/ld/tls.h>

#include <algorithm>
#include <array>
#include <bit>
#include <format>
#include <iomanip>
#include <limits>
#include <ranges>
#include <string>

#include <fbl/array.h>
#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "../tlsdesc-runtime-dynamic.h"
#include "call-tlsdesc.h"

// The natural type of registers regardless of ILP32 or LP64.  There is no
// <stdint.h> name for this.
#if defined(__arm__)
using RegType = uint32_t;
#else
using RegType = uint64_t;
#endif

// The assembly code uses this layout.// This has members for all the registers
// a normal call would clobber but that TLSDESC must not (all call-used
// registers not used for the TLSDESC argument, return value, or return
// address).  The default-constructed values are the standard test data to
// compare against.  The additional members are a few essential values for
// fixed or call-saved registers that are filled in by the assembly code itself
// since they can't have constant values.
using Regs = std::array<RegType, REGS_COUNT>;

// This is defined in assembly (call-tlsdesc.S).  It calls into the runtime via
// the TLSDESC calling convention as a compiler-generated reference would via a
// GOT slot pair (or mockup thereof).  It installs the expected values in
// call-used registers before the call, and then collects their actual values
// after the call.  The SP, FP, and shadow-call SP slots in expected_regs are
// filled in before the TLSDESC call, but others are provided by the caller
// (from the default-constructed values).
extern "C" ptrdiff_t CallTlsdesc(const dl::TlsDescGot& got, Regs& expected_regs, Regs& actual_regs);

namespace {

constexpr std::vector<std::byte> CmpBytes(std::span<const std::byte> bytes) {
  return std::vector<std::byte>{bytes.begin(), bytes.end()};
}

// The test fixture ensures that the thread_local variable is always left
// containing nullptr between tests.  Each test can call SetBlocks() to set it
// to some local test data (once).
class DlTlsRuntimeTests : public ::testing::Test {
 public:
  void SetUp() override { ASSERT_EQ(dl::_dl_tlsdesc_runtime_dynamic_blocks, nullptr); }

  void SetBlocks(fbl::Array<dl::DynamicTlsPtr> blocks) {
    dl::RawDynamicTlsArray old_blocks =
        std::exchange(dl::_dl_tlsdesc_runtime_dynamic_blocks, nullptr);
    ASSERT_EQ(old_blocks, test_blocks_.data());
    test_blocks_ = std::move(blocks);
    dl::_dl_tlsdesc_runtime_dynamic_blocks = test_blocks_.data();
  }

  void TearDown() override {
    SetBlocks({});
    ASSERT_EQ(dl::_dl_tlsdesc_runtime_dynamic_blocks, nullptr);
  }

  static fbl::Array<dl::DynamicTlsPtr> MakeTestBlocks() {
    fbl::AllocChecker ac;
    fbl::Array<dl::DynamicTlsPtr> blocks = fbl::MakeArray<dl::DynamicTlsPtr>(&ac, 2);
    EXPECT_TRUE(ac.check());
    if (blocks) {
      blocks[0] = dl::DynamicTlsPtr::New(ac, kTlsModule0);
      EXPECT_TRUE(ac.check());
      blocks[1] = dl::DynamicTlsPtr::New(ac, kTlsModule1);
      EXPECT_TRUE(ac.check());
      if (blocks[0]) {
        std::span block = blocks[0].contents(kTlsModule0.tls_initial_data.size_bytes());
        EXPECT_EQ(CmpBytes(block), CmpBytes(kTlsModule0.tls_initial_data));
      }
      if (blocks[1]) {
        std::span block = blocks[1].contents(kTlsModule1.tls_initial_data.size_bytes());
        EXPECT_EQ(CmpBytes(block), CmpBytes(kTlsModule1.tls_initial_data));
      }
    }
    return blocks;
  }

 protected:
  static constexpr std::byte kData0[] = {std::byte{0xaa}, std::byte{0xbb}};
  static constexpr std::byte kData1[] = {std::byte{0xcc}, std::byte{0xdd}};
  static constexpr dl::TlsModule kTlsModule0 = {
      .tls_initial_data{kData0},
      .tls_bss_size = 3,
      .tls_alignment = 1,
  };

  static constexpr dl::TlsModule kTlsModule1 = {
      .tls_initial_data{kData1},
      .tls_bss_size = 5,
      .tls_alignment = 1,
  };

 private:
  fbl::Array<dl::DynamicTlsPtr> test_blocks_;
};

// Underlying integer type of the GOT value slot.
// On x86-64 ILP32 this is wider than size_t.
using GotValue = decltype(dl::TlsDescGot{}.value)::value_type;

// Bits used in each half of the split value word.
constexpr int kSplitShift = std::numeric_limits<GotValue>::digits / 2;

// Maximum value that can be stored in each half of the split value word.
constexpr GotValue kSplitMaxValue = (GotValue{1} << kSplitShift) - 1;

// The maximum offset value that might be used in theory (possibly actual
// offsets are much smaller).
constexpr size_t kOffsetMax = std::numeric_limits<size_t>::max();

// Convert a pointer into an offset from $tp.  The optional second argument is
// an offset from that pointer, which need not be a valid offset to add to the
// pointer (i.e. within the same object ptr points into).
ptrdiff_t TpOffset(const std::byte* ptr, size_t offset = 0) {
  return ld::TpRelativeToOffset(ptr) + std::bit_cast<ptrdiff_t>(offset);
}

#if defined(__aarch64__)

std::string RegName(size_t i) {
  switch (i) {
    case REGS_X(1)... REGS_X(18):
      return "x" + std::to_string(i - REGS_X(0));
    case REGS_SP:
      return "sp";
    case REGS_FP:
      return "fp";
    default:
      ADD_FAILURE() << "impossible register index " << i;
      return "";
  }
}

#elif defined(__arm__)

std::string RegName(size_t i) {
  switch (i) {
    case REGS_R1:
      return "r1";
    case REGS_R2:
      return "r2";
    case REGS_R3:
      return "r3";
    case REGS_R12:
      return "r12";
    case REGS_SP:
      return "sp";
    case REGS_FP:
      return "fp";
    default:
      ADD_FAILURE() << "impossible register index " << i;
      return "";
  }
}

#elif defined(__riscv)

std::string RegName(size_t i) {
  switch (i) {
    case REGS_RA:
      return "ra";
    case REGS_T(1)... REGS_T(6):
      return "t" + std::to_string(i - REGS_T(0));
    case REGS_A(1)... REGS_A(7):
      return "a" + std::to_string(i - REGS_A(0));
    case REGS_SP:
      return "sp";
    case REGS_FP:
      return "fp";
    case REGS_GP:
      return "gp";
    default:
      ADD_FAILURE() << "impossible register index " << i;
      return "";
  }
}

#elif defined(__x86_64__)

std::string RegName(size_t i) {
  switch (i) {
    case REGS_RCX:
      return "%rcx";
    case REGS_RDX:
      return "%rdx";
    case REGS_RDI:
      return "%rdi";
    case REGS_RSI:
      return "%rsi";
    case REGS_R8:
      return "%r8";
    case REGS_R9:
      return "%r9";
    case REGS_R10:
      return "%r10";
    case REGS_R11:
      return "%r11";
    case REGS_RSP:
      return "%rsp";
    case REGS_RBP:
      return "%rbp";
    default:
      ADD_FAILURE() << "impossible register index " << i;
      return "";
  }
}

#endif

constexpr size_t RegNameSize(size_t i) { return RegName(i).size(); }

const size_t kRegNameWidth = std::ranges::max(
    std::ranges::views::transform(std::ranges::views::iota(0, REGS_COUNT), RegNameSize));

// This just fills a Regs with distinctive values.
consteval Regs ExpectedRegs() {
  Regs regs;
  for (size_t i = 0; i < regs.size(); ++i) {
    regs[i] = i + 1;
    std::ranges::for_each(std::ranges::views::iota(0u, sizeof(regs[i])), [&](size_t) {
      regs[i] <<= 8;
      regs[i] |= i + 1;
    });
  }
  return regs;
}

// Fill a GOT slot pair for the using the split hook.
dl::TlsDescGot SplitGot(size_t index, size_t offset) {
  return {
      .function = reinterpret_cast<uintptr_t>(dl::_dl_tlsdesc_runtime_dynamic_split),
      .value = (index << kSplitShift) | offset,
  };
}

// Fill a GOT slot pair for the using the indirect hook.
//
// **NOTE:** This captures the argument reference, so that reference must live
// as long as the return value does.  In the value argument of EXPECT_THAT, a
// temporary rvalue will live to the end of the full expression that evaluates
// the match, so it's fine to pass a temporary here when used that way.
dl::TlsDescGot IndirectGot(const dl::TlsdescIndirect& indirect) {
  return {
      .function = reinterpret_cast<uintptr_t>(dl::_dl_tlsdesc_runtime_dynamic_indirect),
      .value = reinterpret_cast<uintptr_t>(&indirect),
  };
}

// The implicit `arg` in the matcher is the TlsDescGot.  It checks that calling
// the TLSDESC hook returns the expected_offset and doesn't clobber registers.
MATCHER_P(TlsdescYields, expected_offset,
          std::format("TLSDESC yields offset {:#x} from $tp {:p}", expected_offset,
                      __builtin_thread_pointer())) {
  Regs expected_regs = ExpectedRegs(), actual_regs;
  memset(&actual_regs, 0xf0, sizeof(actual_regs));
  ptrdiff_t actual_offset = CallTlsdesc(arg, expected_regs, actual_regs);
  bool result = true;
  if (actual_offset != expected_offset) {
    *result_listener << std::hex << std::showbase << "\n  Returned offset " << actual_offset
                     << " != expected " << expected_offset;
    result = false;
  }
  if (!::testing::ExplainMatchResult(::testing::Eq(actual_regs), expected_regs, result_listener)) {
    constexpr int kWidth = 2 + (std::numeric_limits<uintptr_t>::digits / 4);
    *result_listener << std::hex << std::showbase << std::internal << std::setfill('0');
    *result_listener << "\n  Registers clobbered by TLSDESC:"
                     << "\n    " << std::string(kRegNameWidth, ' ')
                     << "        Actual        vs       Expected";
    for (size_t i = 0; i < REGS_COUNT; ++i) {
      std::string name = RegName(i) + ':';
      name += std::string(kRegNameWidth + 2 - name.size(), ' ');
      *result_listener << "\n    " << name << std::setw(kWidth) << actual_regs[i]
                       << (actual_regs[i] == expected_regs[i] ? "  ==  " : "  !=  ")
                       << std::setw(kWidth) << expected_regs[i];
    }
    result = false;
  }
  return result;
}

TEST_F(DlTlsRuntimeTests, TlsdescRuntimeDynamicSplit) {
  fbl::Array blocks = MakeTestBlocks();
  ASSERT_EQ(blocks.size(), 2u);
  std::span block0 = blocks[0].contents(2);
  std::span block1 = blocks[1].contents(kTlsModule1);
  SetBlocks(std::move(blocks));

  EXPECT_THAT(SplitGot(0, 0), TlsdescYields(TpOffset(&block0[0])));
  EXPECT_THAT(SplitGot(0, 1), TlsdescYields(TpOffset(&block0[1])));
  EXPECT_THAT(SplitGot(1, 0), TlsdescYields(TpOffset(&block1[0])));
  EXPECT_THAT(SplitGot(1, 1), TlsdescYields(TpOffset(&block1[1])));
  EXPECT_THAT(SplitGot(0, kSplitMaxValue), TlsdescYields(TpOffset(block0.data(), kSplitMaxValue)));
  EXPECT_THAT(SplitGot(1, kSplitMaxValue), TlsdescYields(TpOffset(block1.data(), kSplitMaxValue)));
}

TEST_F(DlTlsRuntimeTests, TlsdescRuntimeDynamicIndirect) {
  fbl::Array blocks = MakeTestBlocks();
  ASSERT_EQ(blocks.size(), 2u);
  std::span block0 = blocks[0].contents(2);
  std::span block1 = blocks[1].contents(kTlsModule1);
  SetBlocks(std::move(blocks));

  EXPECT_THAT(IndirectGot({0, 0}), TlsdescYields(TpOffset(&block0[0])));
  EXPECT_THAT(IndirectGot({0, 1}), TlsdescYields(TpOffset(&block0[1])));
  EXPECT_THAT(IndirectGot({1, 0}), TlsdescYields(TpOffset(&block1[0])));
  EXPECT_THAT(IndirectGot({1, 1}), TlsdescYields(TpOffset(&block1[1])));
  EXPECT_THAT(IndirectGot({0, kOffsetMax}), TlsdescYields(TpOffset(block0.data(), kOffsetMax)));
  EXPECT_THAT(IndirectGot({1, kOffsetMax}), TlsdescYields(TpOffset(block1.data(), kOffsetMax)));
}

}  // namespace
