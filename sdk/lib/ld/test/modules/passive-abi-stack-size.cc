// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/ld/abi.h>

#include "test-start.h"

#if TEST_STACK_SIZE == 0
#error "TEST_STACK_SIZE must be defined nonzero at compile time"
#endif

extern "C" int64_t TestStart() {
  const size_t stack_size = ld::abi::_ld_abi.stack_size;

  if (stack_size == 0) {
    return 1;
  }

  if (stack_size != TEST_STACK_SIZE) {
    return 2;
  }

  return 17;
}
