// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZIRCON_SYSTEM_UTEST_CORE_NEEDS_NEXT_H_
#define ZIRCON_SYSTEM_UTEST_CORE_NEEDS_NEXT_H_

// This provides macros to facilitate tests that use @next syscalls.  The
// hosted versions of the tests should always run with the vDSO their syscalls
// are available.  The core-tests-standalone versions of the tests are always
// linked in, but sometimes run with one vDSO and sometimes the other.

#ifdef __ASSEMBLER__

#define NEEDS_NEXT_SYSCALL(name) .weak name

#else  // __ASSEMBLER__

#include <zxtest/zxtest.h>

// `NEEDS_NEXT_SYSCALL(zx_foo);` should appear at top level for each @next
// system call referenced.
#define NEEDS_NEXT_SYSCALL(name) [[gnu::weak]] decltype(name) name

// `NEEDS_NEXT_SKIP(zx_foo);` should appear at the top of the TEST(...) body.
#define NEEDS_NEXT_SKIP(name)                                                 \
  do {                                                                        \
    if (!name) {                                                              \
      if (NeedsNextShouldSkip()) {                                            \
        ZXTEST_SKIP("test not run with @next vDSO, " #name " not available"); \
      } else {                                                                \
        FAIL(#name " not resolved in vDSO but test should be using @next");   \
      }                                                                       \
    }                                                                         \
  } while (0)

bool NeedsNextShouldSkip();

#endif  // !__ASSEMBLER__

#endif  // ZIRCON_SYSTEM_UTEST_CORE_NEEDS_NEXT_H_
