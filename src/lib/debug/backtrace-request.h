// Copyright 2016 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_DEBUG_BACKTRACE_REQUEST_H_
#define SRC_LIB_DEBUG_BACKTRACE_REQUEST_H_

#include <stdint.h>
#include <zircon/compiler.h>

// Special values we put in the first register to let the exception handler know
// that we are just requesting a backtrace and we should resume the thread.
#define BACKTRACE_REQUEST_MAGIC_ALL_THREADS ((uint64_t)0xee726573756d65ee)
#define BACKTRACE_REQUEST_MAGIC_CURRENT_THREAD ((uint64_t)0xee726573756d65ef)
#define BACKTRACE_REQUEST_MAGIC_SPECIFIC_THREAD ((uint64_t)0xee726573756d65f0)

__ALWAYS_INLINE static inline void software_break(uint64_t magic) {
  // We set a software breakpoint to trigger the exception handling in
  // crashsvc, which will print the debug info, including the backtrace.
  //
  // We write the "magic" value in the first register (rax on x64, x0 on arm64) so that the
  // exception handler can check for it and resume the thread rather than kill the process.
#ifdef __x86_64__
  __asm__ volatile("int3" : : "a"(magic));
#endif
#ifdef __aarch64__
  // This is what gdb uses.
  __asm__ volatile(
      "mov x0, %0\n\t"
      "brk 0"
      :
      : "r"(magic)
      : "x0");
#endif
#ifdef __riscv
  __asm__ volatile(
      ".option push\n"
      ".option arch, +c\n"
      "mv a0, %0\n"
      "c.ebreak\n"
      ".option pop"
      :
      : "r"(magic)
      : "a0");
#endif
}

// Prints backtrace for all threads and resumes without killing the process.
__ALWAYS_INLINE static inline void backtrace_request_all_threads(void) {
  software_break(BACKTRACE_REQUEST_MAGIC_ALL_THREADS);
}

// Requests a backtrace only for the current thread.
__ALWAYS_INLINE static inline void backtrace_request_current_thread(void) {
  software_break(BACKTRACE_REQUEST_MAGIC_CURRENT_THREAD);
}

__ALWAYS_INLINE static inline void software_break_koid(uint64_t magic, uint64_t koid) {
#ifdef __x86_64__
  __asm__ volatile("int3" : : "a"(magic), "b"(koid));
#endif
#ifdef __aarch64__
  __asm__ volatile(
      "mov x0, %0\n\t"
      "mov x1, %1\n\t"
      "brk 0"
      :
      : "r"(magic), "r"(koid)
      : "x0", "x1");
#endif
#ifdef __riscv
  __asm__ volatile(
      ".option push\n"
      ".option arch, +c\n"
      "mv a0, %0\n"
      "mv a1, %1\n"
      "c.ebreak\n"
      ".option pop"
      :
      : "r"(magic), "r"(koid)
      : "a0", "a1");
#endif
}

// Requests a backtrace for a specific thread by KOID.
__ALWAYS_INLINE static inline void backtrace_request_thread(uint64_t koid) {
  software_break_koid(BACKTRACE_REQUEST_MAGIC_SPECIFIC_THREAD, koid);
}

#endif  // SRC_LIB_DEBUG_BACKTRACE_REQUEST_H_
