// Copyright 2024 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_USERCOPY_HERMETIC_COMMON_H_
#define SRC_STARNIX_LIB_USERCOPY_HERMETIC_COMMON_H_

#include <stddef.h>
#include <stdint.h>

using word = uintptr_t;
constexpr size_t kWordSize = sizeof(word);
constexpr size_t kWordSizeMask = kWordSize - 1;

constexpr word repeat_byte(uint8_t b) {
  constexpr size_t kBitsInByte = 8;
  word w = 0;
  for (size_t i = 0; i < kWordSize; i++) {
    w = (w << kBitsInByte) | b;
  }
  return w;
}

#endif  // SRC_STARNIX_LIB_USERCOPY_HERMETIC_COMMON_H_
