// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// This system call is not yet in the stable vdso.
.weak zx_restricted_enter

.function restricted_exit, global
  // back from restricted mode
  // rdi holds the context
  // rsi holds the reason code
  mov  %rdi,%rsp
  pop  %rsp
  pop  %r15
  pop  %r14
  pop  %r13
  pop  %r12
  pop  %rbp
  pop  %rbx

  // pop the FPU register pointer
  pop  %rax

  // pop the reason code return slot
  pop  %rdx

  // If an FPU register pointer was provided, store the FPU register state into it
  test %rax, %rax
  je .after_store_fpu_registers
  mov %rax, %rdi
  call store_fpu_registers
.after_store_fpu_registers:

  // return the reason code from this function
  mov  %rsi,(%rdx)

  // return back to whatever the address was on the stack
  // make it appear as if the wrapper had returned ZX_OK
  xor  %eax,%eax
  ret
.end_function

.function restricted_enter_wrapper, global
  // args 0 - 1 are already in place in rdi, rsi

  // save the return code pointer on the stack
  push  %rdx

  // save the FPU register pointer on the stack
  push  %rcx

  // save the callee saved regs since the return from restricted mode
  // will zero out all of the registers except rdi and rsi
  push  %rbx
  push  %rbp
  push  %r12
  push  %r13
  push  %r14
  push  %r15
  push  %rsp

  // If an FPU register pointer was provided, load the FPU register state from there
  test %rcx, %rcx
  je .after_load_fpu_registers
  // Save original %rdi
  push %rdi
  mov  %rcx, %rdi
  call load_fpu_registers
  // Restore original %rdi
  pop %rdi
.after_load_fpu_registers:

  // save the pointer the stack as the context pointer in the syscall
  mov   %rsp,%rdx

  // call the syscall
  call  zx_restricted_enter@PLT

  // if we got here it must have failed
  add   $(8*9),%rsp // pop the previous state on the stack
  ret
.end_function

// Load the contents of the array in *rdi to the FPU.
.function load_fpu_registers, global
  // Reset the FPU
  fninit
  // Load registers ST0-ST7 with data.
  fldt (%rdi)
  fldt 10(%rdi)
  fldt 20(%rdi)
  fldt 30(%rdi)
  fldt 40(%rdi)
  fldt 50(%rdi)
  fldt 60(%rdi)
  fldt 70(%rdi)

  // Load values into the SSE registers.
  movups 80(%rdi), %xmm0
  movups 96(%rdi), %xmm1
  movups 112(%rdi), %xmm2
  movups 128(%rdi), %xmm3
  movups 144(%rdi), %xmm4
  movups 160(%rdi), %xmm5
  movups 176(%rdi), %xmm6
  movups 192(%rdi), %xmm7
  movups 208(%rdi), %xmm8
  movups 224(%rdi), %xmm9
  movups 240(%rdi), %xmm10
  movups 256(%rdi), %xmm11
  movups 272(%rdi), %xmm12
  movups 288(%rdi), %xmm13
  movups 304(%rdi), %xmm14
  movups 320(%rdi), %xmm15
  ret
.end_function

// Store the contents of the FPU into the array in *rdi.
.function store_fpu_registers, global
  // Pop the float values out of ST0-ST7.
  // These registers are treated as a stack and thus must be popped LIFO.
  fstpt 70(%rdi)
  fstpt 60(%rdi)
  fstpt 50(%rdi)
  fstpt 40(%rdi)
  fstpt 30(%rdi)
  fstpt 20(%rdi)
  fstpt 10(%rdi)
  fstpt (%rdi)

  // Store the SSE registers.
  movups %xmm0, 80(%rdi)
  movups %xmm1, 96(%rdi)
  movups %xmm2, 112(%rdi)
  movups %xmm3, 128(%rdi)
  movups %xmm4, 144(%rdi)
  movups %xmm5, 160(%rdi)
  movups %xmm6, 176(%rdi)
  movups %xmm7, 192(%rdi)
  movups %xmm8, 208(%rdi)
  movups %xmm9, 224(%rdi)
  movups %xmm10, 240(%rdi)
  movups %xmm11, 256(%rdi)
  movups %xmm12, 272(%rdi)
  movups %xmm13, 288(%rdi)
  movups %xmm14, 304(%rdi)
  movups %xmm15, 320(%rdi)
  ret
.end_function
