// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_C_STARTUP_STARTUP_RANDOM_H_
#define LIB_C_STARTUP_STARTUP_RANDOM_H_

#include "../asm-linkage.h"
#include "src/__support/macros/config.h"

namespace LIBC_NAMESPACE_DECL {

// Use this as an attribute to define an uninitialized variable of trivial
// type.  It will be filled with random bits by InitStartupRandom().
#define LIBC_STARTUP_RANDOM_VAR [[gnu::section(LIBC_ASM_LINKAGE_STRING(StartupRandom))]]

// This fills all LIBC_STARTUP_RANDOM_VAR objects with random bits.  This is
// called very early, before the compiler ABI is set up.  In fact, it's also
// what initializes the main thread's ZX_TLS_STACK_GUARD_OFFSET slot with
// random bits.  Therefore it's built with an hermetic partial link, and so
// needs LIBC_ASM_LINKAGE.
void InitStartupRandom() LIBC_ASM_LINKAGE_DECLARE(InitStartupRandom);

}  // namespace LIBC_NAMESPACE_DECL

#endif  // LIB_C_STARTUP_STARTUP_RANDOM_H_
