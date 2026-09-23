// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef LIB_STDCOMPAT_TYPE_TRAITS_H_
#define LIB_STDCOMPAT_TYPE_TRAITS_H_

#include <cstddef>
#include <type_traits>

#include "internal/type_traits.h"  // IWYU pragma: keep
#include "version.h"               // IWYU pragma: keep

namespace cpp20 {

#if defined(__cpp_lib_bounded_array_traits) && __cpp_lib_bounded_array_traits >= 201902L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

using std::is_bounded_array;
using std::is_bounded_array_v;

using std::is_unbounded_array;
using std::is_unbounded_array_v;

#else  // Provide polyfills for std::is_{,un}bounded_array{,_v}

template <typename T>
struct is_bounded_array : std::false_type {};
template <typename T, std::size_t N>
struct is_bounded_array<T[N]> : std::true_type {};

template <typename T>
static constexpr bool is_bounded_array_v = is_bounded_array<T>::value;

template <typename T>
struct is_unbounded_array : std::false_type {};
template <typename T>
struct is_unbounded_array<T[]> : std::true_type {};

template <typename T>
static constexpr bool is_unbounded_array_v = is_unbounded_array<T>::value;

#endif  // __cpp_lib_bounded_array_traits >= 201902L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

#if defined(__cpp_lib_remove_cvref) && __cpp_lib_remove_cvref >= 201711L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

using std::remove_cvref;
using std::remove_cvref_t;

#else  // Provide polyfill for std::remove_cvref{,_t}

template <typename T>
struct remove_cvref {
  using type = std::remove_cv_t<std::remove_reference_t<T>>;
};

template <typename T>
using remove_cvref_t = typename remove_cvref<T>::type;

#endif  // __cpp_lib_remove_cvref >= 201711L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

#if defined(__cpp_lib_type_identity) && __cpp_lib_type_identity >= 201806L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

using std::type_identity;
using std::type_identity_t;

#else  // Provide polyfill for std::type_identity{,_t}

template <typename T>
struct type_identity {
  using type = T;
};

template <typename T>
using type_identity_t = typename type_identity<T>::type;

#endif  // __cpp_lib_type_identity >= 201806L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

#if defined(__cpp_lib_is_constant_evaluated) && __cpp_lib_is_constant_evaluated >= 201811L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

#define LIB_STDCOMPAT_CONSTEVAL_SUPPORT 1
using std::is_constant_evaluated;

#else  // Provide polyfill for std::is_constant_evaluated

#ifdef __has_builtin
#if __has_builtin(__builtin_is_constant_evaluated)

#define LIB_STDCOMPAT_CONSTEVAL_SUPPORT 1
inline constexpr bool is_constant_evaluated() noexcept { return __builtin_is_constant_evaluated(); }

#endif  // __has_builtin(__builtin_is_constant_evaluated)
#endif  // __has_builtin

#ifndef LIB_STDCOMPAT_CONSTEVAL_SUPPORT

#define LIB_STDCOMPAT_CONSTEVAL_SUPPORT 0
inline constexpr bool is_constant_evaluated() noexcept { return false; }

#endif  // LIB_STDCOMPAT_CONSTEVAL_SUPPORT

#endif  // __cpp_lib_is_constant_evaluated >= 201811L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

}  // namespace cpp20

namespace cpp23 {

#if defined(__cpp_lib_is_scoped_enum) && __cpp_lib_is_scoped_enum >= 202011L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

using std::is_scoped_enum;
using std::is_scoped_enum_v;

#else  // Provide polyfill for std::is_scoped_enum{,_v}

template <typename T, typename = void>
struct is_scoped_enum : std::false_type {};

template <typename T>
struct is_scoped_enum<T, std::enable_if_t<std::is_enum_v<T>>>
    : std::bool_constant<!std::is_convertible_v<T, std::underlying_type_t<T>>> {};

template <typename T>
static constexpr bool is_scoped_enum_v = is_scoped_enum<T>::value;

#endif  // __cpp_lib_is_scoped_enum >= 202011L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

}  // namespace cpp23

#endif  // LIB_STDCOMPAT_TYPE_TRAITS_H_
