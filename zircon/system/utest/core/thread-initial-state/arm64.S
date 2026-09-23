// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.macro check_simd reg1 reg2
  fmov x16, \reg1
  cbnz x16, .Lexit
  fmov x16, \reg2
  cbnz x16, .Lexit
.endm

// [[noreturn]] void InitialStateTestThread(uint64_t* ptr, uint64_t value);
// Arguments are in x0, x1.
.function InitialStateTestThread, global

  bmi .Lexit
  beq .Lexit
  bcs .Lexit
  bvs .Lexit

  cbnz x2, .Lexit
  cbnz x3, .Lexit
  cbnz x4, .Lexit
  cbnz x5, .Lexit
  cbnz x6, .Lexit
  cbnz x7, .Lexit
  cbnz x8, .Lexit
  cbnz x9, .Lexit
  cbnz x10, .Lexit
  cbnz x11, .Lexit
  cbnz x12, .Lexit
  cbnz x13, .Lexit
  cbnz x14, .Lexit
  cbnz x15, .Lexit
  cbnz x16, .Lexit
  cbnz x17, .Lexit
  cbnz x18, .Lexit
  cbnz x19, .Lexit
  cbnz x20, .Lexit
  cbnz x21, .Lexit
  cbnz x22, .Lexit
  cbnz x23, .Lexit
  cbnz x24, .Lexit
  cbnz x25, .Lexit
  cbnz x26, .Lexit
  cbnz x27, .Lexit
  cbnz x28, .Lexit
  cbnz x29, .Lexit
  cbnz x30, .Lexit

  check_simd d0, v0.D[1]
  check_simd d1, v1.D[1]
  check_simd d2, v2.D[1]
  check_simd d3, v3.D[1]
  check_simd d4, v4.D[1]
  check_simd d5, v5.D[1]
  check_simd d6, v6.D[1]
  check_simd d7, v7.D[1]
  check_simd d8, v8.D[1]
  check_simd d9, v9.D[1]
  check_simd d10, v10.D[1]
  check_simd d11, v11.D[1]
  check_simd d12, v12.D[1]
  check_simd d13, v13.D[1]
  check_simd d14, v14.D[1]
  check_simd d15, v15.D[1]
  check_simd d16, v16.D[1]
  check_simd d17, v17.D[1]
  check_simd d18, v18.D[1]
  check_simd d19, v19.D[1]
  check_simd d20, v20.D[1]
  check_simd d21, v21.D[1]
  check_simd d22, v22.D[1]
  check_simd d23, v23.D[1]
  check_simd d24, v24.D[1]
  check_simd d25, v25.D[1]
  check_simd d26, v26.D[1]
  check_simd d27, v27.D[1]
  check_simd d28, v28.D[1]
  check_simd d29, v29.D[1]
  check_simd d30, v30.D[1]
  check_simd d31, v31.D[1]

  // Store the expected value at the top of the stack.
  movlit x16, 0x1234567890abcdef
  // Use a double store to maintain the 16-byte SP alignment for the vDSO call.
  stp xzr, x16, [sp, #-16]!

  // Store the value from the second argument register to the pointer passed in
  // the first argument register, which tests that each had the expected value.
  str x1, [x0]

.Lexit:
  b zx_thread_exit

.end_function
