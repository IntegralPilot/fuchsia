// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/arch/asm.h>

// This defines the entry point in assembly, such that it calls:
//
//   extern "C" int64_t TestStart(...);
//
// In the in-process version, that's a tail call so it returns directly to the
// in-process caller.  In the separate-process version, the return value is
// passed to _exit.

.function _start, global

  // This can assume the sp is already aligned to 16 by the kernel.

#if defined(__aarch64__)

#ifdef IN_PROCESS_TEST
  b TestStart
#else
  .prologue.fp
  bl TestStart
  // Return value in x0 is argument.
  bl _exit
  assert.fail
#endif

#elif defined(__arm__)

#ifdef IN_PROCESS_TEST
  b TestStart
#else
  // .prologue.fp
  bl TestStart
  // Return value in r0 is argument.
  bl _exit
  assert.fail
#endif

#elif defined(__riscv)

#ifdef IN_PROCESS_TEST
  b TestStart
#else
  .prologue.fp
  call TestStart
  // Return value in a0 is argument.
  call _exit
  assert.fail
#endif

#elif defined(__x86_64__)

#ifdef IN_PROCESS_TEST
  // If called by ld-startup-tests.cc code, the return address is already in
  // the word just below %rsp.  Simulate what calling TestStart directly would
  // do so its value is returned to the caller.
  sub $8, %rsp
  jmp TestStart
#else
  // There's no caller so push a placeholder return address to appear in
  // the frame pointer record.
  pushq $0
  .prologue.fp
  call TestStart
  // Move the return value into the argument register.
  mov %rax, %rdi
  call _exit
  assert.fail
#endif

#else
#error "unsupported machine"
#endif

// The backtrace.cc test code needs this.
.label _start_end, global
.end_function
