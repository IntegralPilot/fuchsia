// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "tls-ie-dep.h"

#include <zircon/compiler.h>

// These IE accesses get libtls-ie-dep.so marked with DF_STATIC_TLS.

__EXPORT int* tls_ie_data() {
  [[gnu::tls_model("initial-exec")]] static constinit thread_local int ie_data = 1;
  return &ie_data;
}

__EXPORT int* tls_ie_bss() {
  [[gnu::tls_model("initial-exec")]] static constinit thread_local int ie_bss;
  return &ie_bss;
}
