// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

.globl restricted_enter_wrapper
restricted_enter_wrapper:
  // save the return code pointer on the stack
  push  %rsi

  // Save callee saved registers.
  push  %rbx
  push  %rbp
  push  %r12
  push  %r13
  push  %r14
  push  %r15

  // Pass the pointer to restricted_exit as the second argument.
  lea restricted_exit(%rip), %rsi

  // Pass the stack pointer as the third argument.
  mov %rsp, %rdx

  call zx_restricted_enter

.globl restricted_exit
restricted_exit:
  mov %rdi, %rsp

  // Restore the callee saved registers.
  pop  %r15
  pop  %r14
  pop  %r13
  pop  %r12
  pop  %rbp
  pop  %rbx

  // Pop the reason code pointer from the stack.
  pop  %rdx

  // Store the exit reason in the pointer.
  mov  %rsi,(%rdx)

  // Set the return status to ZX_OK.
  xor %eax, %eax

  // Return to whatever address was on the stack.
  ret
