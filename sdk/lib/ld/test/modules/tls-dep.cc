// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tls-dep.h"

#include <zircon/compiler.h>

__EXPORT constinit thread_local int tls_dep_data = kTlsDepDataValue;
__EXPORT alignas(kTlsDepAlign) constinit thread_local char tls_dep_bss[2];

#if !defined(HAVE_TLSDESC) || !defined(WANT_TLSDESC)
#error "//build/config:{no-,}tlsdesc should define {HAVE,WANT}_TLSDESC"
#elif HAVE_TLSDESC == WANT_TLSDESC

[[gnu::weak]] extern constinit thread_local int tls_dep_weak;

__EXPORT int* get_tls_dep_data() { return &tls_dep_data; }

__EXPORT char* get_tls_dep_bss0() { return &tls_dep_bss[0]; }

__EXPORT char* get_tls_dep_bss1() { return &tls_dep_bss[1]; }

__EXPORT int* get_tls_dep_weak() { return &tls_dep_weak; }

#endif
