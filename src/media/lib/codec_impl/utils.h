// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_MEDIA_LIB_CODEC_IMPL_UTILS_H_
#define SRC_MEDIA_LIB_CODEC_IMPL_UTILS_H_

#include <optional>

// Leaves the source optional with has_value() false instead of leaving source.has_value() true with
// a moved-out contained value. This is a nice way of dealing with an optional that's getting moved
// into a lambda's captures.
template <typename T>
std::optional<T> TakeOptional(std::optional<T>& source) {
  auto tmp = std::move(source);
  source.reset();
  return tmp;
}

template <typename T>
T TakeOptionalValue(std::optional<T>& source) {
  ZX_DEBUG_ASSERT(source.has_value());
  auto tmp = std::move(*source);
  source.reset();
  return tmp;
}

#endif  // SRC_MEDIA_LIB_CODEC_IMPL_UTILS_H_
