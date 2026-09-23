// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "test-ubsan.h"

namespace ubsan {

// The test modules don't have any capabilities plumbed that could be used to
// log a message.  The message will be formatted into the stack buffer and then
// a known register will point to that buffer when the process crashes.  This
// will only make the initial header message visible, but that's something.
int LogError(std::string_view str) {
#if defined(__aarch64__)
  register const char* x0 __asm__("x0") = str.data();
  __asm__ volatile("udf #0" : : "r"(x0), "m"(str.front()));
#elif defined(__riscv)
  register const char* a0 __asm__("a0") = str.data();
  __asm__ volatile("unimp" : : "r"(a0), "m"(str.front()));
#elif defined(__x86_64__)
  __asm__ volatile("ud2" : : "a"(str.data()), "m"(str.front()));
#endif
  __builtin_trap();
}

}  // namespace ubsan
