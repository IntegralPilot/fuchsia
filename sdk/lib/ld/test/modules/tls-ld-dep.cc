// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tls-ld-dep.h"

#include <zircon/compiler.h>

#if !defined(HAVE_TLSDESC) || !defined(WANT_TLSDESC)
#error "//build/config:{no-,}tlsdesc should define {HAVE,WANT}_TLSDESC"
#elif HAVE_TLSDESC == WANT_TLSDESC

// This file is used for tests that test local dynamic TLS access. It's almost
// identical to tls-dep.cc, except the TLS symbols (defined in
// tls-ld-dep-symbols.cc) aren't exported.

extern constinit thread_local char tls_ld_dep_bss[2];
extern constinit thread_local int tls_ld_dep_data;

// These functions perform a local dynamic access of these TLS symbols. The
// reason why these functions don't use a static `thread_local` is because in
// the x64 case, the compiler will add the relative offset of the symbol to the
// pointer returned by `tls_get_addr` (whereas on other architectures
// `__tls_get_addr` will return a pointer with the offset already accounted for).
// This complicates tests that want to use this module to test the pointer
// returned by `__tls_get_addr`. To solve for this, the actual definitions for
// the TLS symbols are in another TU with hidden visibility, so the compiler
// will think this is a global-dynamic TLS access and refrain from making an
// offset addition, but the linker will see this is a local dynamic case and
// handle accordingly.

__EXPORT int* get_tls_ld_dep_data() { return &tls_ld_dep_data; }

__EXPORT char* get_tls_ld_dep_bss0() { return tls_ld_dep_bss; }

__EXPORT char* get_tls_ld_dep_bss1() { return &tls_ld_dep_bss[1]; }

#endif
