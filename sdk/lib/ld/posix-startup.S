// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// This defines the entry point in assembly, such that it calls:
//   extern "C" uintptr_t StartLd(StartupStack& stack);
// And then jumps to the user entry point in the return value, with
// the original stack intact.
// (See posix-startup.cc for definition.)

.function _start, global

  // This can assume the sp is already aligned to 16 by the kernel.

#if defined(__aarch64__)

  mov x0, sp
  bl StartLd
  // The executable's entry point checks x0 for a pointer to pass to atexit.
  mov x16, x0
  mov x0, xzr
  br x16

#elif defined(__arm__)

  mov r0, sp
  bl StartLd
  mov r12, r0
  mov r0, #0
  bx r12

#elif defined(__riscv)

  mv a0, sp
  call StartLd
  // The executable's entry point checks a5 for a pointer to pass to atexit.
  mv a5, zero
  jr a0

#elif defined(__x86_64__)

  // The ld-startup-tests.cc code puts its return address here.
  // Save it in a call-saved register.
  mov -8(%rsp), %rbx

  mov %rsp, %rdi
  call StartLd
  // The executable's entry point checks %rdx for a pointer to pass to atexit.
  xor %rdx, %rdx
  // Restore the saved incoming return address so a special test executable
  // can know the protocol to recover it from below the stack.
  mov %rbx, -8(%rsp)
  jmp *%rax

#else
#error "unsupported machine"
#endif

.end_function
