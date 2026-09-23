// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include <__verbose_abort>  // libc++ internal
#include <cassert>
#include <cstdarg>

#include "log.h"
#include "stdio/printf_core/wrapper.h"

// These panic entry points are only for use within some hermetic_source_set().
// The normal libc entry points use the compiler ABI and stdio and so forth.

namespace {

using LIBC_NAMESPACE::gLog;

[[noreturn]] void VPanic(const char* format, va_list args) {
  using LIBC_NAMESPACE::printf_core::Printf;
  using LIBC_NAMESPACE::printf_core::PrintfNewline;
  Printf<ld::Log::kBufferSize, PrintfNewline::kYes>(gLog, format, args);
  __builtin_trap();
}

[[noreturn, gnu::format(printf, 1, 2)]] void Panic(const char* format, ...) {
  va_list args;
  va_start(args, format);
  VPanic(format, args);
  va_end(args);
}

}  // namespace

void __zx_panic(const char* format, ...) {
  va_list args;
  va_start(args, format);
  VPanic(format, args);
  va_end(args);
}

void std::__libcpp_verbose_abort(const char* format, ...) noexcept {
  va_list args;
  va_start(args, format);
  VPanic(format, args);
  va_end(args);
}

void __assert_fail(const char* expr, const char* file, int line, const char* func) {
  Panic("Assertion failed: %s (%s: %s: %d)\n", expr, file, func, line);
}
