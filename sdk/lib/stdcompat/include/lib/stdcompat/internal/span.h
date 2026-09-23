// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_STDCOMPAT_INTERNAL_SPAN_H_
#define LIB_STDCOMPAT_INTERNAL_SPAN_H_

#include <array>
#include <cstddef>
#include <iterator>
#include <limits>
#include <type_traits>
#include <utility>

#if defined(__cpp_lib_span) && __cpp_lib_span >= 202002L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)
#include <span>
#endif  // __cpp_lib_span >= 202002L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

#include "../iterator.h"
#include "../type_traits.h"
#include "utility.h"

namespace cpp20 {
namespace internal {

static constexpr inline std::size_t dynamic_extent = std::numeric_limits<std::size_t>::max();

struct dynamic_extent_tag {};

// Specialization for different extent types, simplifies span implementation and duplication.
template <typename T, std::size_t Extent>
class extent {
 public:
  explicit constexpr extent(T* data, std::size_t) : data_(data) {}
  constexpr T* data() const { return data_; }
  constexpr std::size_t size() const { return Extent; }

 private:
  T* data_;
};

template <typename T>
class extent<T, dynamic_extent> {
 public:
  explicit constexpr extent(T* data, std::size_t size) : data_(data), size_(size) {}
  constexpr T* data() const { return data_; }
  constexpr std::size_t size() const { return size_; }

 private:
  T* data_;
  std::size_t size_;
};

// Iterator implementation that hides the pointer-based implementation behind an opaque type.
template <typename T>
class span_iterator {
 public:
  using difference_type = ptrdiff_t;
  using value_type = std::remove_cv_t<T>;
  using pointer = T*;
  using reference = T&;

#if _LIBCPP_STD_VER >= 20
  using iterator_category = std::contiguous_iterator_tag;
#else
  // C++17 doesn't support std::contiguous_iterator_tag, so we have to
  // instantiate the libc++ marker template type instead to indicate that the
  // iterator is contiguous.
  using iterator_category = std::random_access_iterator_tag;
#endif

  constexpr explicit span_iterator() : span_iterator(nullptr) {}
  constexpr explicit span_iterator(T* t) : ptr_(t) {}

  constexpr span_iterator(const span_iterator<T>& other) : ptr_(other.ptr_) {}
  constexpr span_iterator(span_iterator<T>&& other) noexcept : ptr_(other.ptr_) {}

  span_iterator& operator=(span_iterator<T> other) {
    if (this != &other) {
      ptr_ = other.ptr_;
    }
    return *this;
  }

  // Allow implicit conversion of span_iterator<V> to span_iterator<const V>.
  // NOLINTNEXTLINE(google-explicit-constructor)
  constexpr operator span_iterator<const T>() const {
    return span_iterator<const T>(static_cast<const T*>(ptr_));
  }

  constexpr reference operator*() const { return *ptr_; }
  constexpr pointer operator->() const { return ptr_; }
  constexpr reference operator[](difference_type offset) const { return *(ptr_ + offset); }

  constexpr bool operator==(span_iterator<T> other) const { return ptr_ == other.ptr_; }
  constexpr bool operator!=(span_iterator<T> other) const { return ptr_ != other.ptr_; }
  constexpr bool operator<(span_iterator<T> other) const { return ptr_ < other.ptr_; }
  constexpr bool operator<=(span_iterator<T> other) const { return ptr_ <= other.ptr_; }
  constexpr bool operator>(span_iterator<T> other) const { return ptr_ > other.ptr_; }
  constexpr bool operator>=(span_iterator<T> other) const { return ptr_ >= other.ptr_; }

  constexpr span_iterator<T>& operator++() {
    ptr_ += 1;
    return *this;
  }
  constexpr span_iterator<T> operator++(int) {
    span_iterator<T> before = *this;
    ++(*this);
    return before;
  }
  constexpr span_iterator<T>& operator--() {
    ptr_ -= 1;
    return *this;
  }
  constexpr span_iterator<T> operator--(int) {
    span_iterator<T> before = *this;
    --(*this);
    return before;
  }
  constexpr span_iterator<T>& operator+=(difference_type n) {
    ptr_ += n;
    return *this;
  }
  constexpr span_iterator<T>& operator-=(difference_type n) {
    ptr_ -= n;
    return *this;
  }
  constexpr span_iterator<T> operator-(difference_type n) const { return span_iterator(ptr_ - n); }
  constexpr span_iterator<T> operator+(difference_type n) const { return span_iterator(ptr_ + n); }

  constexpr difference_type operator-(span_iterator<T> other) const { return ptr_ - other.ptr_; }

 private:
  T* ptr_;
};

template <typename T>
constexpr span_iterator<T> operator+(typename span_iterator<T>::difference_type n,
                                     span_iterator<T> it) {
  return it + n;
}

}  // namespace internal

#if defined(__cpp_lib_span) && __cpp_lib_span >= 202002L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

using std::span;

#else  // Provide forward declaration for traits below

template <typename T, std::size_t Extent>
class span;

#endif  // __cpp_lib_span >= 202002L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

namespace internal {

template <class T>
struct is_span_internal : std::false_type {};

template <class T, std::size_t Extent>
struct is_span_internal<span<T, Extent>> : std::true_type {};

template <class T>
struct is_span : is_span_internal<std::remove_cv_t<T>> {};

template <class T>
struct is_array_internal : std::is_array<T> {};

template <class T, std::size_t S>
struct is_array_internal<std::array<T, S>> : std::true_type {};

template <typename T, typename U>
using is_qualification_conversion = std::is_convertible<T (*)[], U (*)[]>;

template <class T>
struct is_array_type : is_array_internal<std::remove_cv_t<T>> {};

template <typename T, typename ElementType, typename = void>
struct is_well_formed_data_and_size : std::false_type {};

template <typename T, typename ElementType>
struct is_well_formed_data_and_size<
    T, ElementType,
    std::void_t<decltype(std::data(std::declval<T&>()), std::size(std::declval<T&>()))>>
    : is_qualification_conversion<std::remove_pointer_t<decltype(std::data(std::declval<T&>()))>,
                                  ElementType>::type {};

template <typename T, typename = void>
struct has_range_begin_and_end : std::false_type {};

template <typename T>
struct has_range_begin_and_end<T, std::void_t<decltype(std::begin(std::declval<T>())),
                                              decltype(std::begin(std::declval<T>()))>>
    : std::is_same<decltype(std::begin(std::declval<T>())), decltype(std::end(std::declval<T>()))> {
};

template <typename T, class ElementType>
static constexpr bool is_span_compatible_v =
    std::conjunction_v<std::negation<is_span<T>>, std::negation<is_array_type<T>>,
                       has_range_begin_and_end<T>, is_well_formed_data_and_size<T, ElementType>>;

template <typename SizeType, SizeType Extent, SizeType Offset, SizeType Count>
struct subspan_extent
    : std::conditional_t<Count != dynamic_extent, std::integral_constant<SizeType, Count>,
                         std::conditional_t<Extent != dynamic_extent,
                                            std::integral_constant<SizeType, Extent - Offset>,
                                            std::integral_constant<SizeType, dynamic_extent>>> {};

template <typename T, std::size_t Extent>
using byte_span_size =
    std::integral_constant<std::size_t,
                           Extent == dynamic_extent ? dynamic_extent : Extent * sizeof(T)>;

}  // namespace internal
}  // namespace cpp20

#ifdef _LIBCPP_STD_VER
#if _LIBCPP_STD_VER <= 17

// Indicate to libc++ that our `span_iterator` points to elements that are
// contiguous in memory, which enables copy and move optimizations. In C++20,
// this would be done in a standard way by implementing the
// `std::contiguous_iterator_tag` concept. Since that's not present in C++17,
// hook into the libc++ marker template type instead.

namespace std {
inline namespace _LIBCPP_ABI_NAMESPACE {

// Different libc++ versions use one of these class names to determine if an
// iterator is contiguous. We define both so that this works regardless of the
// libc++ version. We have to forward declare these types for the case that
// the other is not already defined.
template <class It>
struct __is_cpp17_contiguous_iterator;
template <typename T>
struct __is_cpp17_contiguous_iterator<cpp20::internal::span_iterator<T>> : true_type {};
template <class It>
struct __libcpp_is_contiguous_iterator;
template <typename T>
struct __libcpp_is_contiguous_iterator<cpp20::internal::span_iterator<T>> : true_type {};

}  // namespace _LIBCPP_ABI_NAMESPACE
}  // namespace std

#endif
#endif

#endif  // LIB_STDCOMPAT_INTERNAL_SPAN_H_
