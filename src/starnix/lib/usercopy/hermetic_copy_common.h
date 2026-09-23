// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_STARNIX_LIB_USERCOPY_HERMETIC_COPY_COMMON_H_
#define SRC_STARNIX_LIB_USERCOPY_HERMETIC_COPY_COMMON_H_

#include "hermetic_common.h"

inline bool has_zeroes(word w) {
  constexpr word kOnes = repeat_byte(0x01);
  constexpr word kMask = repeat_byte(0x80);
  // See https://jameshfisher.com/2017/01/24/bitwise-check-for-zero-byte/ for
  // an explanation of whats going on here.
  return ((w - kOnes) & (~w) & kMask) != 0;
}

// The source and destination buffers are marked volatile so that the read/write
// of each bytes is considered a visible side-effect and prevents the compiler
// from attempting to read memory from multiple pages in an instruction (e.g.
// MOVUPS with XMM registers). We need to make sure each access happens within a
// page so that we read/write as much bytes as possible before faulting and
// ending the copy.
//
// See the change that introduced the volatile qualifier for the buffers for
// and https://g-issues.fuchsia.dev/issues/309108366 for more details.
template <bool until_null>
inline uintptr_t hermetic_copy_common(volatile uint8_t* dest, const volatile uint8_t* source,
                                      size_t count, bool ret_dest) {
  if ((reinterpret_cast<uintptr_t>(dest) | reinterpret_cast<uintptr_t>(source)) & kWordSizeMask) {
    size_t len;
    // src and/or dest do not align on word boundary
    if (((reinterpret_cast<uintptr_t>(dest) ^ reinterpret_cast<uintptr_t>(source)) &
         kWordSizeMask) ||
        (count < kWordSize)) {
      len = count;  // copy the rest of the buffer with the byte mover
    } else {
      len = kWordSize - (reinterpret_cast<uintptr_t>(dest) &
                         kWordSizeMask);  // move the ptrs up to a word boundary
    }
    count -= len;
    for (; len > 0; len--) {
      uint8_t byte = *source++;
      *dest++ = byte;
      if (until_null && (byte == 0)) {
        return ret_dest ? reinterpret_cast<uintptr_t>(dest) : reinterpret_cast<uintptr_t>(source);
      }
    }
  }
  for (; count >= kWordSize; count -= kWordSize) {
    word word_bytes = *reinterpret_cast<const volatile word*>(source);
    if (until_null && has_zeroes(word_bytes)) {
      uint8_t byte;
      do {
        byte = *source++;
        *dest++ = byte;
      } while (byte);
      return ret_dest ? reinterpret_cast<uintptr_t>(dest) : reinterpret_cast<uintptr_t>(source);
    }
    *reinterpret_cast<volatile word*>(dest) = word_bytes;
    dest += kWordSize;
    source += kWordSize;
  }
  for (; count > 0; count--) {
    uint8_t byte = *source++;
    *dest++ = byte;
    if (until_null && (byte == 0)) {
      break;
    }
  }
  return ret_dest ? reinterpret_cast<uintptr_t>(dest) : reinterpret_cast<uintptr_t>(source);
}

#endif  // SRC_STARNIX_LIB_USERCOPY_HERMETIC_COPY_COMMON_H_
