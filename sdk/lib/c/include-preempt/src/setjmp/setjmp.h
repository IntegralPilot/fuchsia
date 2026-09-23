// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef PREEMPT_SRC_SETJMP_SETJMP_H_
#define PREEMPT_SRC_SETJMP_SETJMP_H_

// The llvm-libc header has a nonstandard name for reasons that don't apply in
// the Fuchsia build and might get fixed upstream.  This provides a redirect
// from the standard name to our local wrapper for the custom name.
#include "setjmp_impl.h"  // IWYU pragma: export

#endif  // PREEMPT_SRC_SETJMP_SETJMP_H_
