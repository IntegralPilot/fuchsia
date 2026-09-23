// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tls-initial-dep.h"

#include <zircon/compiler.h>

__EXPORT constinit thread_local int tls_initial_dep_data = kTlsInitialDepDataValue;

#if !defined(HAVE_TLSDESC) || !defined(WANT_TLSDESC)
#error "//build/config:{no-,}tlsdesc should define {HAVE,WANT}_TLSDESC"
#elif HAVE_TLSDESC == WANT_TLSDESC

__EXPORT int* get_tls_initial_dep_data() { return &tls_initial_dep_data; }

#endif
