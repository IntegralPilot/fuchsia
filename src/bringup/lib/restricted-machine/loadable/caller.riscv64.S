// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.section .text
.global caller
caller:
  // Preconditions:
  // - a0 points to memory containing the function address and arguments.
  // - sp must be a valid, accessible stack pointer.

  // Load function address and arguments from the structure pointed to by a0.
  // t0 temporarily holds the function address.
  ld t0, 0(a0)

  ld a3, 32(a0)
  ld a2, 24(a0)
  ld a1, 16(a0)
  ld a0, 8(a0)

  // Branch to function.
  jalr ra, t0, 0

  // The called function's return value will be in a0.
  // We don't need to move it, as a0 is already the first syscall argument register.

  // Call syscall 'ffe' (4094 in decimal) to return.
  li a7, 4094
  ecall

// ping() - environment validation helper
//
// Preconditions:
// - a0-a3 contain valid pointers to unsigned long integers
// Postconditions:
// - a0 contains the sum of [a0]+...+[a3]
// - f0 contains the original loaded from [a0].
// - Returns to the caller (ra)
.global ping
ping:
  mv t0, a0 // save the pointer for use below.
  ld a0, 0(a0)
  ld a1, 0(a1)
  ld a2, 0(a2)
  ld a3, 0(a3)

  // Sum the arguments into a0.
  add a0, a0, a1
  add a0, a0, a2
  add a0, a0, a3

  fld f0,0(t0)
  ret
