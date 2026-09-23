// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// This binary exports an architecture-appropriate thunk which is used
// by the RestrictedMachine::Call() function.  'caller' below provides
// a means for a function to be called in restricted mode, from outside
// of restricted mode, and have its return value be passed back to the
// caller (via a bogus system call argument).
//
// There are restrictions:
// 1. Only up to four arguments may be passed in.
//    More may be added based on architectural support, but the minimum was
//    added for now.
// 2. The wrapped function should return at most a 64-bit value.
//    More complex return values should be returned via shared memory.
// 3. No system calls may be called by the wrapped function.
//    Callers may be prepared to handle the system calls and continue
//    until completion.

// caller is exported for use by restricted-machine.cc.
//
// Expected registers at call:
//    x0 points to memory laid out as follows:
//       *(x0+0) = function address to call
//       *(x0+8+(8*n)) = arg_n, where n={0...3}
//    sp must point to valid, accessible memory
//
// Return to the caller will be done using system call 'ffe'
// which should be invalid on most known operating systems.
//   x0 will contain the called functions return value.
//
// There is no support at present for return values that span
// registers.
.function caller, export
  // Setup the arguments and/or other function preamble
  // For this, the arguments are always pointers.
  ldr r4, [r0]
  ldr r3, [r0, 8+(8*3)]
  ldr r2, [r0, 8+(8*2)]
  ldr r1, [r0, 8+(8*1)]
  ldr r0, [r0, 8+(8*0)]
  blx r4
  // x0 will contain the return value so we leave it there to be the first
  // parameter to a bogus syscall to return.
  //
  // Call syscall 'ffe' to return
  mov r7, #4094
  svc #0
.end_function

// ping() - environment validation helper
//
// Preconditions:
// - r0-r3 contain valid pointers to unsigned long integers
// Postconditions:
// - r0 contains the sum of [r0]+...+[r3]
// - d[0] contains the original loaded from [r0].
// - Returns to the caller (lr)
.function ping, export
  ldr r0, [r0]
  ldr r1, [r1]
  ldr r2, [r2]
  ldr r3, [r3]

  // Clear the first 64-bits of the FPU registers:
  // Write arg0 to the first 32-bits.
  veor d0, d0, d0
  vmov d0[0], r0

  // Sum the arguments to r0
  add r0, r0, r1
  add r0, r0, r2
  add r0, r0, r3

  // Ensure r1 is clear since the caller expects a 64-bit
  eor r1, r1, r1

  bx lr
.end_function
