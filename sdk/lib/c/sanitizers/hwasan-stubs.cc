// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/zircon-internal/unique-backtrace.h>
#include <zircon/compiler.h>

#include <__verbose_abort>  // libc++ internal
#include <cstdarg>
#include <cstdint>
#include <cstdio>
#include <cstdlib>

// In the HWASan build, this file provides weak definitions for all the
// same entry points that are defined by the HWASan runtime library.
// The definitions here are stubs that are used only during the
// dynamic linker's startup phase before the HWASan runtime shared
// library has been loaded.  These are required to satisfy the
// references in libc's own code.
//
// LLVM provides no documentation on the ABI between the compiler and
// the runtime.  The set of function signatures here was culled from
// the LLVM sources for the compiler instrumentation and the runtime
// (see llvm/lib/Transforms/Instrumentation/HWAddressSanitizer.cpp and
// compiler-rt/lib/hwasan/*).

#if __has_feature(hwaddress_sanitizer)

// This is normally defined in the hermetic libc++.a, but the instrumented one
// uses default inline instrumentation.  Because of the same legacy constraints
// that necessitate these stubs exist at all, libc is compiled with non-default
// (suboptimal) instrumentation that avoids generating symbol references that
// are difficult in the bootstrap context.  Hence the prebuilt libc++.a must be
// avoided entirely, and anything that libc++ headers used in building libc
// itself (hermetically) might generate references to must be defined here.
void std::__libcpp_verbose_abort(const char* format, ...) noexcept {
  va_list args;
  va_start(args, format);
  fprintf(stderr, format, args);
  va_end(args);
  abort();
}

// These should never actually be called until the hwasan runtime is loaded.
#define HWASAN_TRAP_STUB(name)                                           \
  extern "C" __EXPORT __WEAK __NO_SAFESTACK void __hwasan_##name(void) { \
    CRASH_WITH_UNIQUE_BACKTRACE();                                       \
  }

// These can be called before the runtime is loaded, but they should effectively
// do nothing until then.
#define HWASAN_STUB(name) \
  extern "C" __EXPORT __WEAK __NO_SAFESTACK void __hwasan_##name(void) {}

HWASAN_TRAP_STUB(init)
HWASAN_STUB(storeN)
HWASAN_STUB(store1)
HWASAN_STUB(store2)
HWASAN_STUB(store4)
HWASAN_STUB(store8)
HWASAN_STUB(store16)
HWASAN_STUB(loadN)
HWASAN_STUB(load1)
HWASAN_STUB(load2)
HWASAN_STUB(load4)
HWASAN_STUB(load8)
HWASAN_STUB(load16)

// TODO(https://fxbug.dev/42060386): The llvm flag for replacing memory checking with
// libcalls also replaces memory tagging. Memory tagging can continue to be
// inlined though since it's perfectly fine to write tags to shadow memory after
// it's set up.
HWASAN_STUB(tag_memory)

// This is instrumented by hwasan into the prologue of every function. Its
// purpose is to add stack information to a thread-local ring buffer in the
// hwasan runtime. This information is used during error reporting to check if
// an access is being made to the stack within a particular function frame.
// This particular stub needs to be empty since it will be called many times
// before the runtime has been loaded. Additionally, if hwasan were to find an
// actual bug, we should end up crashing in one of the trap stubs above.
HWASAN_STUB(add_frame_record)

// This is referenced by HWASan-instrumented code for getting the start
// of a dynamically placed shadow. Currently shadow is still placed zero and the
// HWASan runtime default initializes this to zero so this is just here since
// the early libc startup path needs some definition of this.
extern "C" __EXPORT __WEAK const uintptr_t __hwasan_shadow_memory_dynamic_address = 0;

#endif  // __has_feature(hwaddress_sanitizer)
