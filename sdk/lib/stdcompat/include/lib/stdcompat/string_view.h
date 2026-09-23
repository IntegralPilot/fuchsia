// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SDK_LIB_STDCOMPAT_INCLUDE_LIB_STDCOMPAT_STRING_VIEW_H_
#define SDK_LIB_STDCOMPAT_INCLUDE_LIB_STDCOMPAT_STRING_VIEW_H_

#include <string_view>

#include "version.h"

namespace cpp20 {

// Per the README, we define standalone cpp20::starts_with() and
// cpp20::ends_with() functions. These correspond to std::basic_string_view
// methods introduced in C++20. For parity's sake, in the C++20 context we also
// define the same functions, though as thin wrappers around these methods.

#if defined(__cpp_lib_string_view) && __cpp_lib_string_view >= 202002L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool starts_with(std::basic_string_view<CharT, Traits> s, decltype(s) prefix) {
  return s.starts_with(prefix);
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool starts_with(std::basic_string_view<CharT, Traits> s, const CharT* prefix) {
  return s.starts_with(prefix);
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool starts_with(std::basic_string_view<CharT, Traits> s, CharT prefix) {
  return s.starts_with(prefix);
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool ends_with(std::basic_string_view<CharT, Traits> s, decltype(s) prefix) {
  return s.ends_with(prefix);
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool ends_with(std::basic_string_view<CharT, Traits> s, const CharT* prefix) {
  return s.ends_with(prefix);
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool ends_with(std::basic_string_view<CharT, Traits> s, CharT prefix) {
  return s.ends_with(prefix);
}

#else  // Polyfills for C++20 std::basic_string_view methods.

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool starts_with(std::basic_string_view<CharT, Traits> s, decltype(s) prefix) {
  return s.substr(0, prefix.size()) == prefix;
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool starts_with(std::basic_string_view<CharT, Traits> s, const CharT* prefix) {
  return ::cpp20::starts_with(s, decltype(s){prefix});
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool starts_with(std::basic_string_view<CharT, Traits> s, CharT c) {
  return !s.empty() && Traits::eq(s.front(), c);
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool ends_with(std::basic_string_view<CharT, Traits> s, decltype(s) suffix) {
  return s.size() >= suffix.size() && s.substr(s.size() - suffix.size(), suffix.size()) == suffix;
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool ends_with(std::basic_string_view<CharT, Traits> s, const CharT* suffix) {
  return ::cpp20::ends_with(s, decltype(s){suffix});
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool ends_with(std::basic_string_view<CharT, Traits> s, CharT c) {
  return !s.empty() && Traits::eq(s.back(), c);
}

#endif  // if __cpp_lib_string_view >= 202002L && !defined(LIB_STDCOMPAT_USE_POLYFILLS)

// The templated functions above do the real work, and they're sufficient on
// their own for calls that use a std::basic_string_view instantiation directly
// in the first argument.  However, argument type deduction alone won't be able
// to choose a template instantiation to use for some _other_ type that is
// implicitly convertible to a std::basic_string_view instantiation, such as
// the corresponding std::basic_string instantiation.  In C++20, std::string
// also has these methods.  But there is no <lib/stdcompat/string.h> polyfill
// to provide them.  Instead, these replacements for the std::basic_string_view
// methods are made to accept std::basic_string arguments for the common
// instantiations that have standard aliases (std::string_view, etc.).  Since
// these overloads are not templated, they will be considered first and they
// don't require any type deduction to determine that e.g. the std::string_view
// argument can be implicitly converted from a std::string value in the call.

constexpr bool starts_with(std::string_view s, std::string_view prefix) {
  return ::cpp20::starts_with<char>(s, prefix);
}

constexpr bool starts_with(std::string_view s, const char* prefix) {
  return ::cpp20::starts_with<char>(s, prefix);
}

constexpr bool starts_with(std::string_view s, char prefix) {
  return ::cpp20::starts_with<char>(s, prefix);
}

constexpr bool ends_with(std::string_view s, std::string_view suffix) {
  return ::cpp20::ends_with<char>(s, suffix);
}

constexpr bool ends_with(std::string_view s, const char* prefix) {
  return ::cpp20::ends_with<char>(s, prefix);
}

constexpr bool ends_with(std::string_view s, char prefix) {
  return ::cpp20::ends_with<char>(s, prefix);
}

constexpr bool starts_with(std::wstring_view s, std::wstring_view prefix) {
  return ::cpp20::starts_with<wchar_t>(s, prefix);
}

constexpr bool starts_with(std::wstring_view s, const wchar_t* prefix) {
  return ::cpp20::starts_with<wchar_t>(s, prefix);
}

constexpr bool starts_with(std::wstring_view s, wchar_t prefix) {
  return ::cpp20::starts_with<wchar_t>(s, prefix);
}

constexpr bool ends_with(std::wstring_view s, std::wstring_view suffix) {
  return ::cpp20::ends_with<wchar_t>(s, suffix);
}

constexpr bool ends_with(std::wstring_view s, const wchar_t* prefix) {
  return ::cpp20::ends_with<wchar_t>(s, prefix);
}

constexpr bool ends_with(std::wstring_view s, wchar_t prefix) {
  return ::cpp20::ends_with<wchar_t>(s, prefix);
}

#if defined(__cpp_char8_t) && __cpp_char8_t >= 201811L
constexpr bool starts_with(std::u8string_view s, std::u8string_view prefix) {
  return ::cpp20::starts_with<char8_t>(s, prefix);
}

constexpr bool starts_with(std::u8string_view s, const char8_t* prefix) {
  return ::cpp20::starts_with<char8_t>(s, prefix);
}

constexpr bool starts_with(std::u8string_view s, char8_t prefix) {
  return ::cpp20::starts_with<char8_t>(s, prefix);
}

constexpr bool ends_with(std::u8string_view s, std::u8string_view suffix) {
  return ::cpp20::ends_with<char8_t>(s, suffix);
}

constexpr bool ends_with(std::u8string_view s, const char8_t* prefix) {
  return ::cpp20::ends_with<char8_t>(s, prefix);
}

constexpr bool ends_with(std::u8string_view s, char8_t prefix) {
  return ::cpp20::ends_with<char8_t>(s, prefix);
}
#endif

constexpr bool starts_with(std::u16string_view s, std::u16string_view prefix) {
  return ::cpp20::starts_with<char16_t>(s, prefix);
}

constexpr bool starts_with(std::u16string_view s, const char16_t* prefix) {
  return ::cpp20::starts_with<char16_t>(s, prefix);
}

constexpr bool starts_with(std::u16string_view s, char16_t prefix) {
  return ::cpp20::starts_with<char16_t>(s, prefix);
}

constexpr bool ends_with(std::u16string_view s, std::u16string_view suffix) {
  return ::cpp20::ends_with<char16_t>(s, suffix);
}

constexpr bool ends_with(std::u16string_view s, const char16_t* prefix) {
  return ::cpp20::ends_with<char16_t>(s, prefix);
}

constexpr bool ends_with(std::u16string_view s, char16_t prefix) {
  return ::cpp20::ends_with<char16_t>(s, prefix);
}

constexpr bool starts_with(std::u32string_view s, std::u32string_view prefix) {
  return ::cpp20::starts_with<char32_t>(s, prefix);
}

constexpr bool starts_with(std::u32string_view s, const char32_t* prefix) {
  return ::cpp20::starts_with<char32_t>(s, prefix);
}

constexpr bool starts_with(std::u32string_view s, char32_t prefix) {
  return ::cpp20::starts_with<char32_t>(s, prefix);
}

constexpr bool ends_with(std::u32string_view s, std::u32string_view suffix) {
  return ::cpp20::ends_with<char32_t>(s, suffix);
}

constexpr bool ends_with(std::u32string_view s, const char32_t* prefix) {
  return ::cpp20::ends_with<char32_t>(s, prefix);
}

constexpr bool ends_with(std::u32string_view s, char32_t prefix) {
  return ::cpp20::ends_with<char32_t>(s, prefix);
}

}  // namespace cpp20

namespace cpp23 {

#if defined(__cpp_lib_string_contains) && __cpp_lib_string_contains >= 202011L && \
    !defined(LIB_STDCOMPAT_USE_POLYFILLS)

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool contains(std::basic_string_view<CharT, Traits> haystack, decltype(haystack) needle) {
  return haystack.contains(needle);
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool contains(std::basic_string_view<CharT, Traits> haystack, const CharT* needle) {
  return haystack.contains(needle);
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool contains(std::basic_string_view<CharT, Traits> haystack, CharT needle) {
  return haystack.contains(needle);
}

#else

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool contains(std::basic_string_view<CharT, Traits> haystack, decltype(haystack) needle) {
  return haystack.find(needle) != std::basic_string_view<CharT, Traits>::npos;
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool contains(std::basic_string_view<CharT, Traits> haystack, CharT needle) {
  return haystack.find(needle) != std::basic_string_view<CharT, Traits>::npos;
}

template <class CharT, class Traits = std::char_traits<CharT>>
constexpr bool contains(std::basic_string_view<CharT, Traits> haystack, const CharT* needle) {
  return haystack.find(needle) != std::basic_string_view<CharT, Traits>::npos;
}

#endif

// See comments above regarding separate overloads for common instantiations.

constexpr bool contains(std::string_view haystack, std::string_view needle) {
  return ::cpp23::contains<char>(haystack, needle);
}

constexpr bool contains(std::string_view haystack, const char* needle) {
  return ::cpp23::contains<char>(haystack, needle);
}

constexpr bool contains(std::string_view haystack, char needle) {
  return ::cpp23::contains<char>(haystack, needle);
}

constexpr bool contains(std::wstring_view haystack, std::wstring_view needle) {
  return ::cpp23::contains<wchar_t>(haystack, needle);
}

constexpr bool contains(std::wstring_view haystack, const wchar_t* needle) {
  return ::cpp23::contains<wchar_t>(haystack, needle);
}

constexpr bool contains(std::wstring_view haystack, wchar_t needle) {
  return ::cpp23::contains<wchar_t>(haystack, needle);
}

#if defined(__cpp_char8_t) && __cpp_char8_t >= 201811L
constexpr bool contains(std::u8string_view haystack, std::u8string_view needle) {
  return ::cpp23::contains<char8_t>(haystack, needle);
}

constexpr bool contains(std::u8string_view haystack, const char8_t* needle) {
  return ::cpp23::contains<char8_t>(haystack, needle);
}

constexpr bool contains(std::u8string_view haystack, char8_t needle) {
  return ::cpp23::contains<char8_t>(haystack, needle);
}
#endif

constexpr bool contains(std::u16string_view haystack, std::u16string_view needle) {
  return ::cpp23::contains<char16_t>(haystack, needle);
}

constexpr bool contains(std::u16string_view haystack, const char16_t* needle) {
  return ::cpp23::contains<char16_t>(haystack, needle);
}

constexpr bool contains(std::u16string_view haystack, char16_t needle) {
  return ::cpp23::contains<char16_t>(haystack, needle);
}

constexpr bool contains(std::u32string_view haystack, std::u32string_view needle) {
  return ::cpp23::contains<char32_t>(haystack, needle);
}

constexpr bool contains(std::u32string_view haystack, const char32_t* needle) {
  return ::cpp23::contains<char32_t>(haystack, needle);
}

constexpr bool contains(std::u32string_view haystack, char32_t needle) {
  return ::cpp23::contains<char32_t>(haystack, needle);
}

}  // namespace cpp23

#endif  // SDK_LIB_STDCOMPAT_INCLUDE_LIB_STDCOMPAT_STRING_VIEW_H_
