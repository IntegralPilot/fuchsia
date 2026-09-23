// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <stdint.h>

#include "suffixed-test-start.h"

// Note, we use extern "C" to make debugging easier than seeing mangled names.
extern "C" {

int64_t SUFFIXED_SYMBOL(a)();
int64_t SUFFIXED_SYMBOL(b)();
int64_t SUFFIXED_SYMBOL(f)();

int64_t SUFFIXED_SYMBOL(TestStart)() {
  // a should return 13
  // b should return -8
  // f should return 3
  // 13 + -8 + 3 + 9 = 17
  return SUFFIXED_SYMBOL(a)() + SUFFIXED_SYMBOL(b)() + SUFFIXED_SYMBOL(f)() + 9;
}

}  // extern "C"
