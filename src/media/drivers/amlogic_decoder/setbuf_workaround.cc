// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdio.h>
#include <zircon/assert.h>

// TODO(b/333745994): This workaround prevents the driver from importing setbuf, but TBD why the
// driver is importing setbuf with the new clang.
extern "C" void setbuf(FILE* __restrict, char* __restrict) {
  // While setbuf symbol is imported, we don't expect it to actually get called. If it is called,
  // panic.
  ZX_PANIC("setbuf called");
}
