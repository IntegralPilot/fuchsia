// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_ARRAY_H_
#define LIB_FIDL_CPP_WIRE_ARRAY_H_

#include <zircon/fidl.h>

#include <algorithm>

namespace fidl {

// Implementation of std::array guaranteed to have the same memory layout as a C array,
// hence the same layout as the FIDL wire-format.
// The standard does not guarantee that there are no trailing padding bytes in std::array.
// When adding new functionalities to this struct, the data layout should not be changed.
template <typename T, size_t N>
struct Array final {
  using value_type = T;

  static constexpr size_t size() { return N; }

  constexpr size_t max_size() const { return N; }
  constexpr size_t empty() const { return false; }

  constexpr const T* data() const { return data_; }
  constexpr T* data() { return data_; }

  constexpr const T& at(size_t offset) const { return data()[offset]; }
  constexpr T& at(size_t offset) { return data()[offset]; }

  constexpr const T& operator[](size_t offset) const { return at(offset); }
  constexpr T& operator[](size_t offset) { return at(offset); }

  constexpr T* begin() { return data(); }
  constexpr const T* begin() const { return data(); }
  constexpr const T* cbegin() const { return data(); }

  constexpr T* end() { return data() + size(); }
  constexpr const T* end() const { return data() + size(); }
  constexpr const T* cend() const { return data() + size(); }

  // Keeping data_ public such that an aggregate initializer can be used.
  T data_[N];

  static_assert(N > 0, "fidl::Array cannot have zero elements.");
};

template <typename T, size_t N>
bool operator==(const fidl::Array<T, N>& lhs, const fidl::Array<T, N>& rhs) {
  return std::equal(lhs.begin(), lhs.end(), rhs.begin(), rhs.end());
}

template <typename T, size_t N>
bool operator!=(const fidl::Array<T, N>& lhs, const fidl::Array<T, N>& rhs) {
  return !(lhs == rhs);
}

}  // namespace fidl

#endif  // LIB_FIDL_CPP_WIRE_ARRAY_H_
