// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "jmp_buf.h"
#include "startup/startup-random.h"

namespace LIBC_NAMESPACE_DECL {

// This is used from assembly via its LIBC_ASM_LINKAGE name to read the values.
// The only other use is implicitly in InitStartupRandom, which just gets there
// from the section bounds (untagged symbols) anyway.  The assembly code
// doesn't de-tag from an hwasan-adjusted symbol address.  Since there really
// aren't any other accesses that would check a tag, nothing is lost by not
// using a tagged pointer to access it in that one place.
[[clang::no_sanitize("hwaddress")]]
LIBC_STARTUP_RANDOM_VAR decltype(gJmpBufManglers) gJmpBufManglers;

}  // namespace LIBC_NAMESPACE_DECL
