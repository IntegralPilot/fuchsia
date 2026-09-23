// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tls-ld-dep.h"

#if !defined(HAVE_TLSDESC) || !defined(WANT_TLSDESC)
#error "//build/config:{no-,}tlsdesc should define {HAVE,WANT}_TLSDESC"
#elif HAVE_TLSDESC == WANT_TLSDESC

alignas(kTlsDepAlign) constinit thread_local char tls_ld_dep_bss[2];

constinit thread_local int tls_ld_dep_data = kTlsLdDepDataValue;

#endif
