// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/lib/unwinder/unwinder_base.h"

#include <gtest/gtest.h>

#include "src/lib/unwinder/elf_module_cache.h"
#include "src/lib/unwinder/testing/mock_memory.h"
#include "src/lib/unwinder/testing/mock_unwinder.h"

namespace unwinder {

class UnwinderBaseTest : public ::testing::Test {
 public:
  UnwinderBaseTest() : module_cache_({}), mock_unwinder_(module_cache_) {}

 protected:
  ElfModuleCache module_cache_;
  MockUnwinder mock_unwinder_;
  MockMemory mock_memory_;
};

TEST_F(UnwinderBaseTest, TryUnwinderSuccess) {
  Registers regs(Registers::Arch::kX64);
  regs.SetPC(0x1000);
  Frame current(regs, false, Frame::Trust::kContext);

  Frame next_expected(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
  next_expected.regs.SetPC(0x2000);
  mock_unwinder_.SetFrames({next_expected});

  Frame next(Registers(Registers::Arch::kX64), true, Frame::Trust::kCFI);
  auto err = TryUnwinder(&mock_unwinder_, &mock_memory_, current, next);

  EXPECT_TRUE(err.ok());
  uint64_t pc;
  EXPECT_TRUE(next.regs.GetPC(pc).ok());
  EXPECT_EQ(0x2000UL, pc);
  EXPECT_EQ(Frame::Trust::kCFI, next.trust);
}

TEST_F(UnwinderBaseTest, TryUnwinderError) {
  Registers regs(Registers::Arch::kX64);
  regs.SetPC(0x1000);
  Frame current(regs, false, Frame::Trust::kContext);

  Frame next(Registers(Registers::Arch::kX64), true, Frame::Trust::kCFI);

  mock_unwinder_.SetStepError(Error("Failed"));

  auto err = TryUnwinder(&mock_unwinder_, &mock_memory_, current, next);
  EXPECT_TRUE(err.has_err());
}

TEST_F(UnwinderBaseTest, UnwindLoop) {
  Registers regs(Registers::Arch::kX64);
  regs.SetPC(0x1000);
  regs.SetSP(0x10000);

  Frame frame1(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
  frame1.regs.SetPC(0x2000);
  mock_unwinder_.SetFrames({frame1});

  auto frames = mock_unwinder_.Unwind(nullptr, regs);

  ASSERT_EQ(2UL, frames.size());
  uint64_t pc;
  frames[0].regs.GetPC(pc);
  EXPECT_EQ(0x1000UL, pc);
  frames[1].regs.GetPC(pc);
  EXPECT_EQ(0x2000UL, pc);
}

TEST_F(UnwinderBaseTest, AsyncUnwindLoop) {
  Registers regs(Registers::Arch::kX64);
  regs.SetPC(0x1000);
  regs.SetSP(0x10000);

  MockAsyncMemoryDelegate delegate;
  AsyncMemory async_memory(&delegate);

  Frame frame1(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
  frame1.regs.SetPC(0x2000);
  mock_unwinder_.SetFrames({frame1});

  bool done = false;
  mock_unwinder_.AsyncUnwind(&async_memory, regs, 50, [&](std::vector<Frame> frames) {
    ASSERT_EQ(2UL, frames.size());
    uint64_t pc;
    frames[0].regs.GetPC(pc);
    EXPECT_EQ(0x1000UL, pc);
    frames[1].regs.GetPC(pc);
    EXPECT_EQ(0x2000UL, pc);
    done = true;
  });

  EXPECT_TRUE(done);
}

TEST_F(UnwinderBaseTest, TryUnwinderSignalFrame) {
  Registers regs(Registers::Arch::kX64);
  regs.SetPC(0x1000);
  Frame current(regs, false, Frame::Trust::kContext);

  Frame next_expected(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
  next_expected.regs.SetPC(0x2000);
  next_expected.is_signal_frame = true;
  mock_unwinder_.SetFrames({next_expected});

  Frame next(Registers(Registers::Arch::kX64), true, Frame::Trust::kCFI);
  auto err = TryUnwinder(&mock_unwinder_, &mock_memory_, current, next);
  EXPECT_TRUE(err.ok());
  EXPECT_TRUE(next.is_signal_frame);
  EXPECT_FALSE(next.pc_is_return_address);
}

TEST_F(UnwinderBaseTest, TryUnwinderPcIsReturnAddress) {
  Registers regs(Registers::Arch::kX64);
  regs.SetPC(0x1000);
  Frame current(regs, false, Frame::Trust::kContext);

  Frame next_expected(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
  next_expected.regs.SetPC(0x2000);
  // Do NOT set rax.
  mock_unwinder_.SetFrames({next_expected});

  Frame next(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
  auto err = TryUnwinder(&mock_unwinder_, &mock_memory_, current, next);
  EXPECT_TRUE(err.ok());
  EXPECT_TRUE(next.pc_is_return_address);
}

TEST_F(UnwinderBaseTest, TryUnwinderPcIsNOTReturnAddress) {
  Registers regs(Registers::Arch::kX64);
  regs.SetPC(0x1000);
  Frame current(regs, false, Frame::Trust::kContext);

  Frame next_expected(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
  next_expected.regs.SetPC(0x2000);
  next_expected.regs.Set(RegisterID::kX64_rax, 0);  // Set rax.
  mock_unwinder_.SetFrames({next_expected});

  Frame next(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
  auto err = TryUnwinder(&mock_unwinder_, &mock_memory_, current, next);
  EXPECT_TRUE(err.ok());
  EXPECT_FALSE(next.pc_is_return_address);
}

TEST_F(UnwinderBaseTest, TryAsyncUnwinderSuccess) {
  Registers regs(Registers::Arch::kX64);
  regs.SetPC(0x1000);
  Frame current(regs, false, Frame::Trust::kContext);

  MockAsyncMemoryDelegate delegate;
  AsyncMemory async_memory(&delegate);

  Frame next_expected(Registers(Registers::Arch::kX64), false, Frame::Trust::kCFI);
  next_expected.regs.SetPC(0x2000);
  mock_unwinder_.SetFrames({next_expected});

  bool done = false;
  TryAsyncUnwinder(&mock_unwinder_, &async_memory, current, [&](Error err, Frame next) {
    EXPECT_TRUE(err.ok());
    uint64_t pc;
    EXPECT_TRUE(next.regs.GetPC(pc).ok());
    EXPECT_EQ(0x2000UL, pc);
    EXPECT_EQ(Frame::Trust::kCFI, next.trust);
    done = true;
  });

  EXPECT_TRUE(done);
}

TEST_F(UnwinderBaseTest, SetTrust) {
  mock_unwinder_.SetTrust(Frame::Trust::kFP);
  EXPECT_EQ(Frame::Trust::kFP, mock_unwinder_.trust());
}

}  // namespace unwinder
