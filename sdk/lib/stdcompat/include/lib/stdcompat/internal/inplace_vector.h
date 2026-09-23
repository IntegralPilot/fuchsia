// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_STDCOMPAT_INTERNAL_INPLACE_VECTOR_H_
#define LIB_STDCOMPAT_INTERNAL_INPLACE_VECTOR_H_

#include <algorithm>
#include <cassert>
#include <initializer_list>
#include <iterator>
#include <memory>
#include <type_traits>
#include <utility>

#include "../ranges.h"
#include "span.h"

#if __cpp_exceptions
#include <new>
#include <stdexcept>
#endif  // __cpp_exceptions

namespace cpp26::internal {

template <typename R, typename T>
using container_compatible_range_t =
    std::enable_if_t<std::is_convertible_v<decltype(*std::begin(std::declval<R>())), T>>;

[[noreturn]] inline void inplace_vector_abort() {
#if __cpp_exceptions
  throw std::bad_alloc();
#else
  __builtin_abort();
#endif
}

[[noreturn]] inline void throw_abort_out_of_range() {
#if __cpp_exceptions
  throw std::out_of_range();
#else
  __builtin_abort();
#endif
}

template <typename T, std::size_t N>
class inplace_vector_storage_trivial {
 public:
  constexpr inplace_vector_storage_trivial() = default;
  ~inplace_vector_storage_trivial() = default;

  constexpr T* data() noexcept { return buffer_; }
  constexpr const T* data() const noexcept { return buffer_; }

  constexpr size_t size() const noexcept { return size_; }
  constexpr void set_size(size_t size) noexcept { size_ = size; }

  template <class... Args>
  constexpr void construct_at(size_t k, Args&&... args) {
    buffer_[k] = T(std::forward<Args>(args)...);
  }

  constexpr void destroy_at(size_t k) {}

 private:
  alignas(T) T buffer_[N] = {};
  size_t size_ = 0;
};

template <typename T, std::size_t N>
class inplace_vector_storage_non_trivial {
 public:
  constexpr inplace_vector_storage_non_trivial() = default;

#if __cplusplus >= 202002l
  constexpr
#endif  // __cplusplus >= 202002l
      ~inplace_vector_storage_non_trivial() {
    for (size_t i = 0; i < size_; ++i) {
      destroy_at(i);
    }
  }

  constexpr T* data() noexcept { return reinterpret_cast<T*>(buffer_); }
  constexpr const T* data() const noexcept { return reinterpret_cast<const T*>(buffer_); }

  constexpr size_t size() const noexcept { return size_; }
  constexpr void set_size(size_t size) noexcept { size_ = size; }

  template <class... Args>
  constexpr void construct_at(size_t k, Args&&... args) {
    new (data() + k) T(std::forward<Args>(args)...);
  }

  constexpr void destroy_at(size_t k) { data()[k].~T(); }

 private:
  alignas(T) std::byte buffer_[N * sizeof(T)];
  size_t size_ = 0;
};

template <typename T, std::size_t N>
using inplace_vector_storage =
    std::conditional_t<std::is_trivial_v<T>, inplace_vector_storage_trivial<T, N>,
                       inplace_vector_storage_non_trivial<T, N>>;

}  // namespace cpp26::internal

#endif  // LIB_STDCOMPAT_INTERNAL_INPLACE_VECTOR_H_
