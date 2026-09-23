// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <zircon/assert.h>

#include <__verbose_abort>

// TODO(https://fxbug.dev/42080760): These should print something useful instead of just crashing.

extern "C" void __assert_fail(const char*, const char*, int, const char*) { __builtin_trap(); }

extern "C" void __zx_panic(const char* format, ...) { __builtin_trap(); }

void std::__libcpp_verbose_abort(const char* format, ...) noexcept { __builtin_trap(); }

// GCC has an implicit declaration of abort with default visibility even under
// -fvisibility=hidden.  Even with an explicit attribute on the definition, it
// will just ignore it (with a warning) because of the "previous" declaration.
// So instead of `extern "C"`, declare a different function from the language
// perspective (one that would get a mangled name) but override its linkage
// name explicitly.
void abort() __asm__("abort");
void abort() { __builtin_trap(); }
