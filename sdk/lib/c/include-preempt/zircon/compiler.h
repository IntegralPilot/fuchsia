// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_ZIRCON_COMPILER_H_
#define PREEMPT_ZIRCON_COMPILER_H_

// TODO(https://fxbug.dev/42105189): <zircon/compiler.h> is used a little in
// libc itself, but a lot in other Fuchsia library headers that libc uses
// internally.  It's not namespace-clean yet, and it defines some macros that
// conflict with llvm-libc code's use of the same identifiers.
//
// This header preempts the real <zircon/compiler.h> and then includes it
// first, then does #undef for some problematic macros.  That means that any
// other headers that themselves do use these particular macros also need
// include-preempt wrapper headers.  The push_macro / pop_macro pragmas allow
// all the preempt headers to cope with all permutations of headers including
// each other.

#pragma push_macro("add_overflow")
#pragma push_macro("sub_overflow")
#pragma push_macro("mul_overflow")

#undef add_overflow
#undef sub_overflow
#undef mul_overflow

#include_next <zircon/compiler.h>

#pragma pop_macro("add_overflow")
#pragma pop_macro("sub_overflow")
#pragma pop_macro("mul_overflow")

#endif  // PREEMPT_ZIRCON_COMPILER_H_
