// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_FIDL_CPP_WIRE_INTERNAL_DISPLAY_ERROR_H_
#define LIB_FIDL_CPP_WIRE_INTERNAL_DISPLAY_ERROR_H_

#include <zircon/fidl.h>

#include <cstddef>
#include <cstdint>

namespace fidl::internal {

// |DisplayError| should be explicitly specialized for error-like types which
// could be printed: transport errors, application errors, etc.
template <typename T>
struct DisplayError {
  // Formats the description into a buffer |destination| that is of size
  // |capacity|. The description will cut off at `capacity - 1`. It inserts a
  // trailing NUL.
  //
  // Returns how many bytes were written, not counting the NUL.
  //
  // Explicit specializations should define a method with this signature.
  // This particular method does not have an implementation, such that missed
  // specializations would be detected by a link-time error.
  static size_t Format(const T& value, char* destination, size_t capacity);
};

// Convenience wrapper for |DisplayError<T>::Format|.
template <typename T>
size_t FormatDisplayError(const T& value, char* destination, size_t capacity) {
  return DisplayError<T>::Format(value, destination, capacity);
}

// Built-in support for printing raw numerical errors.
// TODO(https://fxbug.dev/42177190): |zx_status_t| dispatches to this path today.
// Ideally we would like to print the human readable status name.
template <>
struct DisplayError<int32_t> {
  static size_t Format(const int32_t& value, char* destination, size_t capacity);
};
template <>
struct DisplayError<uint32_t> {
  static size_t Format(const uint32_t& value, char* destination, size_t capacity);
};

}  // namespace fidl::internal

#endif  // LIB_FIDL_CPP_WIRE_INTERNAL_DISPLAY_ERROR_H_
