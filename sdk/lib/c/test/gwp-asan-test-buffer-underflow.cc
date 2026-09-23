// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <stdint.h>

int main() {
  // GWP-ASan randomly decides whether to right or left-align
  // allocations. We just need one of those allocations to
  // be left-aligned so the buffer underflow is triggered.
  for (int i = 0; i < 1000; i++) {
    volatile uint64_t* p = new uint64_t;
    p[-1] = 0x31;
    delete (p);
  }
  return 0;
}
