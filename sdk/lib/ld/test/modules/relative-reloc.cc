// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "suffixed-test-start.h"

namespace {

const int i = 17;
const int* s = &i;

}  // namespace

extern "C" int64_t SUFFIXED_SYMBOL(TestStart)() {
  // We need to take the address of `s` so the compiler doesn't optimize this
  // as a pc-relative load to `i`.
  const int** p = &s;
  // Similarly, we need the compiler to lose all provenance information about
  // this pointer so it doesn't just optimize this to a pc-relative load or
  // more likely just load 17.
  __asm__("" : "=r"(p) : "0"(p));
  return **p;
}
