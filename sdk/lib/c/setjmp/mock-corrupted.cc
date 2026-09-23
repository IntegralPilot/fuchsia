// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "mock-corrupted.h"

#include <zircon/assert.h>

#include "fuchsia/jmp_buf.h"

namespace LIBC_NAMESPACE_DECL {

// This is set only during a test and always cleared at the end of the test.
decltype(gMockLongjmpCorrupted) gMockLongjmpCorrupted;

// The test longjmp calls this instead of the real version that always panics.
[[noreturn]] void longjmp_corrupted(jmp_buf env) {
  if (gMockLongjmpCorrupted) {
    gMockLongjmpCorrupted(env);
    __builtin_trap();
  }
  ZX_PANIC("longjmp() called with corrupted jmp_buf %p", static_cast<const void*>(env));
}

}  // namespace LIBC_NAMESPACE_DECL
