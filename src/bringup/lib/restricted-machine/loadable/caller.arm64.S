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
// 2. The wrapped function should return at most a register-width value
//    pointer.  More complex return values will be lost.
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
  ldr x4, [x0]
  ldr x3, [x0, 8+(8*3)]
  ldr x2, [x0, 8+(8*2)]
  ldr x1, [x0, 8+(8*1)]
  ldr x0, [x0, 8+(8*0)]
  // Branch to function
  blr x4
  // x0 will contain the return value so we leave it there to be the first
  // parameter to a bogus syscall to return.
  //
  // Call syscall 'ffe' to return
  mov x8, xzr
  add x8, x8, #4094
  svc #0
.end_function

// ping() - environment validation helper
//
// Preconditions:
// - x0-x3 contain valid pointers to unsigned long integers
// Postconditions:
// - x0 contains the sum of [x0]+...+[x3]
// - v0.d[0] contains the original loaded from [x0].
// - Returns to the caller (lr)
.function ping, export
  ldr x0, [x0]
  ldr x1, [x1]
  ldr x2, [x2]
  ldr x3, [x3]
  mov x4, x0

  // Sum the arguments to x0
  add x0, x0, x1
  add x0, x0, x2
  add x0, x0, x3

  // Clear the first 64-bits of the FPU registers:
  // First, save off the upper 64-bits of q[0]
  umov x1, v0.d[1]
  // Then, clear the lower 64-bits of q[0]
  fmov d0, x4
  // Finally, restore the upper bits after zero-extension
  ins v0.d[1], x1

  ret
.end_function
