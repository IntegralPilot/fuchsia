// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include "jmp_buf.h"

namespace LIBC_NAMESPACE_DECL {

// The longjmp assembly code makes a tail call to this when it detects the
// jmp_buf is not self-consistent (bad checksum).  It has already clobbered
// many callee-saved registers from longjmp's caller, but the important ones
// for further backtraces won't be clobbered, so at least the plain backtrace
// of the call stack leading to longjmp should be intact in the crash state.
[[noreturn]] void longjmp_corrupted(jmp_buf env) {
  ZX_PANIC("longjmp() called with corrupted jmp_buf %p", static_cast<const void*>(env));
}

}  // namespace LIBC_NAMESPACE_DECL
