// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/hw/debug/x86.h>
#include <zircon/syscalls/exception.h>

#include <optional>

#include <gtest/gtest.h>

#include "src/developer/debug/ipc/decode_exception.h"
#include "src/developer/debug/shared/arch_x86.h"

namespace debug_ipc {

TEST(DecodeException, Arm64) {
  uint32_t esr = 0;
  auto f = [&]() { return std::make_optional(esr); };

  // Exceptions that require no decoding.
  EXPECT_EQ(ExceptionType::kSoftwareBreakpoint, DecodeArm64Exception(ZX_EXCP_SW_BREAKPOINT, f));
  EXPECT_EQ(ExceptionType::kGeneral, DecodeArm64Exception(ZX_EXCP_GENERAL, f));
  EXPECT_EQ(ExceptionType::kPageFault, DecodeArm64Exception(ZX_EXCP_FATAL_PAGE_FAULT, f));
  EXPECT_EQ(ExceptionType::kUndefinedInstruction,
            DecodeArm64Exception(ZX_EXCP_UNDEFINED_INSTRUCTION, f));
  EXPECT_EQ(ExceptionType::kUnalignedAccess, DecodeArm64Exception(ZX_EXCP_UNALIGNED_ACCESS, f));
  EXPECT_EQ(ExceptionType::kThreadStarting, DecodeArm64Exception(ZX_EXCP_THREAD_STARTING, f));
  EXPECT_EQ(ExceptionType::kThreadExiting, DecodeArm64Exception(ZX_EXCP_THREAD_EXITING, f));
  EXPECT_EQ(ExceptionType::kPolicyError, DecodeArm64Exception(ZX_EXCP_POLICY_ERROR, f));
  EXPECT_EQ(ExceptionType::kProcessStarting, DecodeArm64Exception(ZX_EXCP_PROCESS_STARTING, f));

  // Hardware breakpoints. The meaty stuff.
  esr = 0b110000 << 26;
  EXPECT_EQ(ExceptionType::kHardwareBreakpoint, DecodeArm64Exception(ZX_EXCP_HW_BREAKPOINT, f));
  esr = 0b110001 << 26;
  EXPECT_EQ(ExceptionType::kHardwareBreakpoint, DecodeArm64Exception(ZX_EXCP_HW_BREAKPOINT, f));

  esr = 0b110010 << 26;
  EXPECT_EQ(ExceptionType::kSingleStep, DecodeArm64Exception(ZX_EXCP_HW_BREAKPOINT, f));
  esr = 0b110011 << 26;
  EXPECT_EQ(ExceptionType::kSingleStep, DecodeArm64Exception(ZX_EXCP_HW_BREAKPOINT, f));
}

TEST(DecodeException, X64) {
  X64DebugRegs regs;
  auto f = [&]() { return std::make_optional(regs); };

  // Exceptions that require no decoding.
  EXPECT_EQ(ExceptionType::kSoftwareBreakpoint, DecodeX64Exception(ZX_EXCP_SW_BREAKPOINT, f));
  EXPECT_EQ(ExceptionType::kGeneral, DecodeX64Exception(ZX_EXCP_GENERAL, f));
  EXPECT_EQ(ExceptionType::kPageFault, DecodeX64Exception(ZX_EXCP_FATAL_PAGE_FAULT, f));
  EXPECT_EQ(ExceptionType::kUndefinedInstruction,
            DecodeX64Exception(ZX_EXCP_UNDEFINED_INSTRUCTION, f));
  EXPECT_EQ(ExceptionType::kUnalignedAccess, DecodeX64Exception(ZX_EXCP_UNALIGNED_ACCESS, f));
  EXPECT_EQ(ExceptionType::kThreadStarting, DecodeX64Exception(ZX_EXCP_THREAD_STARTING, f));
  EXPECT_EQ(ExceptionType::kThreadExiting, DecodeX64Exception(ZX_EXCP_THREAD_EXITING, f));
  EXPECT_EQ(ExceptionType::kPolicyError, DecodeX64Exception(ZX_EXCP_POLICY_ERROR, f));
  EXPECT_EQ(ExceptionType::kProcessStarting, DecodeX64Exception(ZX_EXCP_PROCESS_STARTING, f));

  // Hardware breakpoints. The meaty stuff.
  regs.dr0 = 0x1111111111111111;
  regs.dr1 = 0x2222222222222222;
  regs.dr2 = 0x3333333333333333;
  regs.dr3 = 0x4444444444444444;

  regs.dr6 = 0;
  X86_DBG_STATUS_BS_SET(&regs.dr6, 1);
  ASSERT_EQ(ExceptionType::kSingleStep, DecodeX64Exception(ZX_EXCP_HW_BREAKPOINT, f));

  regs.dr6 = 0;
  X86_DBG_STATUS_B0_SET(&regs.dr6, 1);
  EXPECT_EQ(ExceptionType::kHardwareBreakpoint, DecodeX64Exception(ZX_EXCP_HW_BREAKPOINT, f));
  X86_DBG_CONTROL_RW0_SET(&regs.dr7, 1);
  EXPECT_EQ(ExceptionType::kWatchpoint, DecodeX64Exception(ZX_EXCP_HW_BREAKPOINT, f));

  regs.dr6 = 0;
  X86_DBG_STATUS_B1_SET(&regs.dr6, 1);
  EXPECT_EQ(ExceptionType::kHardwareBreakpoint, DecodeX64Exception(ZX_EXCP_HW_BREAKPOINT, f));
  X86_DBG_CONTROL_RW1_SET(&regs.dr7, 1);
  EXPECT_EQ(ExceptionType::kWatchpoint, DecodeX64Exception(ZX_EXCP_HW_BREAKPOINT, f));

  regs.dr6 = 0;
  X86_DBG_STATUS_B2_SET(&regs.dr6, 1);
  EXPECT_EQ(ExceptionType::kHardwareBreakpoint, DecodeX64Exception(ZX_EXCP_HW_BREAKPOINT, f));
  X86_DBG_CONTROL_RW2_SET(&regs.dr7, 1);
  EXPECT_EQ(ExceptionType::kWatchpoint, DecodeX64Exception(ZX_EXCP_HW_BREAKPOINT, f));

  regs.dr6 = 0;
  X86_DBG_STATUS_B3_SET(&regs.dr6, 1);
  EXPECT_EQ(ExceptionType::kHardwareBreakpoint, DecodeX64Exception(ZX_EXCP_HW_BREAKPOINT, f));
  X86_DBG_CONTROL_RW3_SET(&regs.dr7, 1);
  EXPECT_EQ(ExceptionType::kWatchpoint, DecodeX64Exception(ZX_EXCP_HW_BREAKPOINT, f));
}

}  // namespace debug_ipc
