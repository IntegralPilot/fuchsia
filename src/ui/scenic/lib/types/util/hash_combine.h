// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_UI_SCENIC_LIB_TYPES_UTIL_HASH_COMBINE_H_
#define SRC_UI_SCENIC_LIB_TYPES_UTIL_HASH_COMBINE_H_

#include <cstddef>
#include <functional>

namespace types {

// Generic function to combine hashes, assuming that std::hash<T> specialization exists.
template <class T>
inline void hash_combine(std::size_t& seed, const T& v) {
  std::hash<T> hasher;
  seed ^= hasher(v) + 0x9e3779b9 + (seed << 6) + (seed >> 2);
}

}  // namespace types

#endif  // SRC_UI_SCENIC_LIB_TYPES_UTIL_HASH_COMBINE_H_
