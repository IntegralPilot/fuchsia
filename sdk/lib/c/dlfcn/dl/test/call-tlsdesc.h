// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_DL_TEST_CALL_TLSDESC_H_
#define LIB_DL_TEST_CALL_TLSDESC_H_

// This defines constants shared with the call-tlsdesc.S assembly code and the
// tlsdesc-runtime-dynamic-tests.cc C++ code.  Each machine defines REGS_COUNT
// and a set of REGS_* macros for indices into an array of that size.

#if defined(__aarch64__)

#define REGS_X(n) ((n) - 1)  // For x1..x18 only.
#define REGS_FP REGS_X(19)
#define REGS_SP REGS_X(20)
#define REGS_COUNT 20

#elif defined(__arm__)

#define REGS_R0 0
#define REGS_R1 1
#define REGS_R2 2
#define REGS_R3 3
#define REGS_R12 4
#define REGS_SP 5
#define REGS_FP 6
#define REGS_COUNT 7

#elif defined(__riscv)

#define REGS_RA 0
#define REGS_T(n) (n)        // For t1..t6 only.
#define REGS_A(n) (6 + (n))  // For a1..a7 only.
#define REGS_SP REGS_A(8)
#define REGS_FP REGS_A(9)
#define REGS_GP REGS_A(10)
#define REGS_COUNT REGS_A(11)

#elif defined(__x86_64__)

#define REGS_RCX 0
#define REGS_RDX 1
#define REGS_RDI 2
#define REGS_RSI 3
#define REGS_R8 4
#define REGS_R9 5
#define REGS_R10 6
#define REGS_R11 7
#define REGS_RSP 8
#define REGS_RBP 9
#define REGS_COUNT 10

#endif

#endif  // LIB_DL_TEST_CALL_TLSDESC_H_
