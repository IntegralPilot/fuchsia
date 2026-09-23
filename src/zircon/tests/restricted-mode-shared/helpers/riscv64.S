// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

.globl restricted_enter_wrapper
restricted_enter_wrapper:
  // A0 contains the options vector, and A1 contains the reason code pointer.

  // Make space for all of the normal mode context on the stack.
  addi sp, sp, -128

  // Save the reason code pointer.
  sd a1, (sp)

  // Save the shadow call stack pointer.
  sd gp, 8(sp)

  // Save all of the callee saved registers.
  sd s0, 16(sp)
  sd s1, 24(sp)
  sd s2, 32(sp)
  sd s3, 40(sp)
  sd s4, 48(sp)
  sd s5, 56(sp)
  sd s6, 64(sp)
  sd s7, 72(sp)
  sd s8, 80(sp)
  sd s9, 88(sp)
  sd s10, 96(sp)
  sd s11, 104(sp)

  // Save the return address.
  sd ra, 112(sp)

  // Save the thread pointer.
  sd tp, 120(sp)

  // Pass restricted exit as the return vector to the syscall.
  la a1, restricted_exit

  // Pass the stack pointer as the context argument to the syscall.
  mv a2, sp

  call zx_restricted_enter

  // If we got here it must have failed.
  // Restore the return address from prior to the syscall. We have to do this
  // because RA is caller-saved.
  ld ra, 112(sp)
  // Reset the stack.
  addi sp, sp, 128
  ret

.globl restricted_exit
restricted_exit:
  // Back from restricted mode
  // a0 holds the context, which is the stack pointer
  // a1 holds the reason code

  // Restore the stack pointer at the point of the restricted enter wrapper.
  mv sp, a0

  // Restore the shadow call stack pointer.
  ld gp, 8(sp)

  // Restore the callee saved registers.
  ld s0, 16(sp)
  ld s1, 24(sp)
  ld s2, 32(sp)
  ld s3, 40(sp)
  ld s4, 48(sp)
  ld s5, 56(sp)
  ld s6, 64(sp)
  ld s7, 72(sp)
  ld s8, 80(sp)
  ld s9, 88(sp)
  ld s10, 96(sp)
  ld s11, 104(sp)

  // Restore the return address.
  ld ra, 112(sp)

  // Restore the thread pointer.
  ld tp, 120(sp)

  // Move the reason code into the stored pointer.
  ld t3, (sp)
  sd a1, (t3)

  // Pop all the normal mode context off the stack.
  addi sp, sp, 128

  // Return to whatever address was in RA.
  // Make it appear as if the wrapper had returned ZX_OK.
  mv a0, zero
  ret
