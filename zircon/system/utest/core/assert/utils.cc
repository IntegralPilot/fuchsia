// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

// A simple function used during ASSERT testing, annotated so that it will not
// be inlined and hopefully keep the ASSERT predicates used in the tests
// complicated enough that they don't simply get optimized away by the compiler
// in a strange way which might otherwise would have triggered a warning.
extern "C" {

[[gnu::noinline]] uint32_t EchoValue(uint32_t val) {
  __asm__("" : "=r"(val) : "0"(val));
  return val;
}
}
