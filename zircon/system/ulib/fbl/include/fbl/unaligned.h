// Copyright 2023 The Fuchsia Authors
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef FBL_UNALIGNED_H_
#define FBL_UNALIGNED_H_

#include <lib/stdcompat/bit.h>

#include <array>
#include <cstddef>
#include <cstring>

namespace fbl {

// Loads a value of type T at a potentially unaligned address.
// T must be bit-castable (see std::bit_cast documentation).
template <typename T>
inline T UnalignedLoad(const void* source) {
  // This first allocates storage for the type and then performs a bit_cast to
  // "cast" (or "copy") the storage into an instance of T. This is equivalent to
  // this logic:
  //   T tmp;
  //   std::memcpy(&tmp, ...);
  //   return tmp;
  // but also works for types that are not default constructible.
  std::array<std::byte, sizeof(T)> tmp;
  std::memcpy(&tmp, reinterpret_cast<const std::byte*>(source), sizeof(T));
  return cpp20::bit_cast<T>(tmp);
}

// Stores a value at a potentially unaligned destination address.
// T must be bit-castable (see std::bit_cast documentation).
template <typename T>
inline void UnalignedStore(void* dest, T value) {
  std::memcpy(reinterpret_cast<std::byte*>(dest), &value, sizeof(T));
}

}  // namespace fbl

#endif  // FBL_UNALIGNED_H_
