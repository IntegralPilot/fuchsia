// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

.section .text
.global caller
caller:
  // Preconditions:
  // - rdi points to memory containing the function address and arguments.
  // - rsp must be a valid, accessible stack pointer.

  // Load the function address and the four arguments.
  // We use rax as a temporary register to hold the function address.
  movq (%rdi), %rax

  // r9 holds the fourth argument.
  movq 32(%rdi), %rcx

  // r8 holds the third argument.
  movq 24(%rdi), %rdx

  // rsi holds the second argument.
  movq 16(%rdi), %rsi

  // The first argument is loaded into rdi.
  movq 8(%rdi), %rdi

  // Call the function whose address is in rax.
  callq *%rax

  // The function's return value is in rax, but it needs to be the first
  // argument for the syscall.
  movq %rax, %rdi

  // Call syscall 'ffe' (4094 in decimal) to return.
  // The syscall number is placed in rax.
  movq $4094, %rax

  syscall


.global ping
ping:
  // Preconditions:
  // - rdi, rsi, rdx, rcx contain valid pointers to unsigned long integers.
  // Postconditions:
  // - rax contains the sum of [rdi]+...+[rcx].
  // - xmm0 contains the original loaded from [rdi].
  // - Returns to the caller (using ret).

  // Load the 64-bit unsigned integers from memory.
  movq (%rdi), %rax
  movq (%rsi), %rsi
  movq (%rdx), %rdx
  movq (%rcx), %rcx

  // Sum the arguments into rax.
  addq %rsi, %rax
  addq %rdx, %rax
  addq %rcx, %rax

  // Ping must also place the first argument value in the initial 8 bytes
  // of the FPU registers (see $arch.S).  On x64, we must place the value
  // in ST(7). ST(7) is 10 bytes and FPU operates as a stack. So
  // we first, swap the 0th and 7th elements, then save the 7th element
  // to our stack. This allows us to clobber 8 bytes of that value, reload
  // it onto the FPU stack, and then swap it back to the 7th element.

  // Prep the stack so we can use some scratch.
  pushq   %rbp
  movq    %rsp, %rbp

  subq $16, %rsp // Scratch for 10 bytes, aligned.
  fxch %st(7)  // Swap ST(7) for ST(0).
  fstpt (%rsp) // Save ST(0) on the stack.
  movq (%rdi), %rcx // Load the first argument into rcx.
  pushq %rcx // Overwrite 8 bytes on the stack with rcx.
  fldt (%rsp) // Load the updated 10 bytes onto the FPU stack.
  fxch %st(7) // Swap the loaded value into ST(7).

  // Clean up our scratch stack on the way out.
  movq %rbp, %rsp
  popq %rbp
  ret
