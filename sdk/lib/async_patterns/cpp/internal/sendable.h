// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_ASYNC_PATTERNS_CPP_INTERNAL_SENDABLE_H_
#define LIB_ASYNC_PATTERNS_CPP_INTERNAL_SENDABLE_H_

#include <lib/stdcompat/type_traits.h>

#include <tuple>
#include <type_traits>

namespace async_patterns::internal {

template <typename T>
struct Smuggled {
  T value;

  // Pretend this is a |T| to support |std::is_invocable| tests.
  // It is never called in practice.
  // NOLINTNEXTLINE(google-explicit-constructor)
  operator T() const {
    __builtin_abort();
    return {};
  }
};

// |BindForSending| disallows sending pointers. In some cases we know the
// pointee will outlive the consumer. This function wraps a value (likely
// some pointer) to be sent and disables any restrictions in |BindForSending|.
template <typename T>
auto Smuggle(T&& value) {
  return Smuggled<std::remove_reference_t<T>>{std::forward<T>(value)};
}

template <typename T>
auto UnSmuggle(T&& value) {
  return std::forward<T>(value);
}

template <typename T>
auto UnSmuggle(Smuggled<T>&& value) {
  return std::forward<T>(value.value);
}

}  // namespace async_patterns::internal

#endif  // LIB_ASYNC_PATTERNS_CPP_INTERNAL_SENDABLE_H_
