// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_CONST_STRING_H_
#define SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_CONST_STRING_H_

#include <algorithm>
#include <array>
#include <cstddef>
#include <span>
#include <string>
#include <string_view>
#include <type_traits>

namespace elfldltl::internal {

using namespace std::literals;

// No constexpr std::to_string, and it doesn't handle non-10 bases anyway.
// C++23 has constexpr std::to_chars for integers, but that doesn't help with
// determining the length of buffer for it to fill.
template <typename T>
consteval std::string ToString(T n, unsigned int base = 10, bool alt = false) {
  std::string str;
  auto it = str.begin();
  if constexpr (std::is_signed_v<T>) {
    if (n < 0) {
      str = '-';
      it = ++str.begin();
      n = -n;
    }
  }
  if (alt) {
    switch (base) {
      case 8:
        str += '0';
        it = str.end();
        break;
      case 16:
        str += "0x";
        it = str.end();
        break;
      default:
        break;
    }
  }
  do {
    it = str.insert(it, "0123456789abcdef"[n % base]);
    n /= base;
  } while (n > 0);
  return str;
}

// ConstVector<T, N> is used only to define constexpr variables.  It can
// only be constructed one way, and cannot be default-constructed, copied, or
// moved.  The constructor argument is a captureless constexpr lambda of no
// arguments that returns some standard container with value_type = T and
// size() == N.  The deduction guide handles the template parameters as long as
// it returns something container-like.  Once constructed, it acts like a
// std::span<const T, N> that points to a constexpr std::array<T, N>.  The
// actual storage is only unique for the sequence of values, even if multiple
// different lambda arguments produce the same sequence.
template <typename T, size_t N>
class ConstVector : public std::span<const T, N> {
 public:
  ConstVector() = delete;
  ConstVector(const ConstVector&) = delete;
  ConstVector(ConstVector&&) = delete;

  // The argument is a captureless consteval lambda returning std::vector<T>.
  // The actual argument isn't used, since it's not a constant expression.
  // It's just used to deduce the type so constexpr instances can be called.
  template <class Maker>
  explicit consteval ConstVector(Maker) : std::span<const T, N>{kStorage<MakeArray<Maker>()>} {}

 private:
  using StorageArray = std::array<T, N>;

  // This ensures a single instantiation of the actual storage for each unique
  // sequence of values, regardless of how they're computed at compile time.
  template <StorageArray Values>
  static constexpr StorageArray kStorage = Values;

  template <class Maker>
  static consteval StorageArray MakeArray() {
    constexpr Maker maker;  // This can be called in constant expressions.
    static_assert(maker().size() == N);
    // Compute a constexpr object containing the right values.
    StorageArray values{};
    std::ranges::copy(maker(), values.begin());
    return values;
  }
};

// Deduction guide.
template <class Maker>
ConstVector(Maker maker) -> ConstVector<typename decltype(maker())::value_type, Maker{}().size()>;

// ConstString is to string as ConstVector is to vector.  Its constructor
// argument lambda should return some std::basic_string<...> type.  The
// constructed object is like a string_view rather than a span, but also has a
// c_str() method and guarantees NUL termination like string does.
template <typename CharT, class Traits>
class ConstString : public std::basic_string_view<CharT, Traits> {
 public:
  ConstString() = delete;
  ConstString(const ConstString&) = delete;
  ConstString(ConstString&&) = delete;

  template <class Maker>
  explicit consteval ConstString(Maker)
      : ConstString{ConstVector{[] {
          std::basic_string<CharT> str = Maker{}();
          str.push_back('\0');
          return str;
        }}} {}

  explicit consteval ConstString(const CharT* str) : std::basic_string_view<CharT, Traits>{str} {}

  consteval const char* c_str() const { return this->data(); }

 private:
  template <size_t N>
  explicit consteval ConstString(const ConstVector<CharT, N>& chars)
      : std::string_view{chars.data(), N - 1} {}
};

// Deduction guide.
template <class Maker>
ConstString(Maker maker)
    -> ConstString<typename decltype(maker())::value_type, typename decltype(maker())::traits_type>;

}  // namespace elfldltl::internal

#endif  // SRC_LIB_ELFLDLTL_INCLUDE_LIB_ELFLDLTL_INTERNAL_CONST_STRING_H_
