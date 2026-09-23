// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "hermetic_zero.h"

#include "hermetic_common.h"

// Build with the following cc flags:
//  `-O3 -fno-omit-frame-pointer -mno-omit-leaf-frame-pointer`

constexpr uint8_t kCh = 0;
constexpr word kRepeatedCh = repeat_byte(kCh);

[[gnu::section(".text.entry")]] uintptr_t hermetic_zero(volatile uint8_t* dest, size_t count) {
  if (reinterpret_cast<uintptr_t>(dest) & kWordSizeMask) {
    size_t len;
    if (count < kWordSize) {
      len = count;
    } else {
      len = kWordSize - (reinterpret_cast<uintptr_t>(dest) & kWordSizeMask);
    }
    count -= len;
    for (; len > 0; len--) {
      *dest++ = kCh;
    }
  }

  for (; count >= kWordSize; count -= kWordSize) {
    *reinterpret_cast<volatile word*>(dest) = kRepeatedCh;
    dest += kWordSize;
  }
  for (; count > 0; count--) {
    *dest++ = kCh;
  }
  return reinterpret_cast<uintptr_t>(dest);
}
