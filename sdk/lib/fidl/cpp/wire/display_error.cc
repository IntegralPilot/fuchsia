// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fidl/coding.h>
#include <lib/fidl/cpp/wire/internal/display_error.h>
#include <zircon/assert.h>

#include <cstdio>

namespace fidl::internal {

size_t fidl::internal::DisplayError<int32_t>::Format(const int32_t& value, char* destination,
                                                     size_t capacity) {
  int num_would_write = snprintf(destination, capacity, "int32_t (value: %d)", value);
  return static_cast<size_t>(num_would_write) >= capacity ? capacity - 1 : num_would_write;
}

size_t fidl::internal::DisplayError<uint32_t>::Format(const uint32_t& value, char* destination,
                                                      size_t capacity) {
  int num_would_write = snprintf(destination, capacity, "uint32_t (value: %u)", value);
  return static_cast<size_t>(num_would_write) >= capacity ? capacity - 1 : num_would_write;
}

}  // namespace fidl::internal
