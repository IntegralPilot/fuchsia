// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// [[noreturn]] void InitialStateTestThread(uint64_t* ptr, uint64_t value);
// Arguments are in a0, a1.
.function InitialStateTestThread, global

  // RISC-V has no condition codes or processor flags to test.

  // Most general registers should be zero.
  bnez zero, .Lexit     // This one especially!
  bnez ra, .Lexit
  bnez gp, .Lexit
  bnez tp, .Lexit
  bnez t0, .Lexit
  bnez t1, .Lexit
  bnez t2, .Lexit
  bnez s0, .Lexit
  bnez s1, .Lexit
  bnez a2, .Lexit
  bnez a3, .Lexit
  bnez a4, .Lexit
  bnez a5, .Lexit
  bnez a6, .Lexit
  bnez a7, .Lexit
  bnez s2, .Lexit
  bnez s3, .Lexit
  bnez s4, .Lexit
  bnez s5, .Lexit
  bnez s6, .Lexit
  bnez s7, .Lexit
  bnez s8, .Lexit
  bnez s9, .Lexit
  bnez s10, .Lexit
  bnez s11, .Lexit
  bnez t3, .Lexit
  bnez t4, .Lexit
  bnez t5, .Lexit
  bnez t6, .Lexit

#ifndef __riscv_float_abi_soft
  // All the F/D registers should be all zero bits.
  .irp n,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23,24,25,26,27,28,29,30,31
    fmv.x.d t0, f\n
    bnez t0, .Lexit
  .endr

  // TODO(mcgrathr): check Q and V registers when those are available
#endif

  // Store the expected value at the top of the stack.
  li t0, 0x1234567890abcdef
  sd t0, -8(sp)

  // Keep the stack aligned to 16 bytes for the call into the vDSO.
  add sp, sp, -16

  // Store the value from the second argument register to the pointer passed in
  // the first argument register, which tests that each had the expected value.
  sd a1, (a0)

.Lexit:
  tail zx_thread_exit

.end_function
