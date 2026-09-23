// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_TBI_TBI_H_
#define LIB_TBI_TBI_H_

#include <stdint.h>
#include <string.h>

// This is a small library for common TBI-related operations like stripping tags from pointers.

namespace tbi {

inline constexpr size_t kTagShift = 56;
inline constexpr uintptr_t kTagMask = uint64_t{0xff} << kTagShift;
inline constexpr uintptr_t kAddressMask = ~kTagMask;

constexpr uintptr_t RemoveTag(uintptr_t ptr) { return ptr & kAddressMask; }

template <typename T>
inline T* RemoveTag(T* ptr) {
  return reinterpret_cast<T*>(RemoveTag(reinterpret_cast<uintptr_t>(ptr)));
}

}  // namespace tbi

#endif  // LIB_TBI_TBI_H_
