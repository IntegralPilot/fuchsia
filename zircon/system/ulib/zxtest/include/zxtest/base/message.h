// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef ZXTEST_BASE_MESSAGE_H_
#define ZXTEST_BASE_MESSAGE_H_

#include <zircon/status.h>

#include <array>
#include <iterator>
#include <string>
#include <string_view>
#include <tuple>

#include <fbl/string_printf.h>
#include <zxtest/base/types.h>

namespace zxtest {

namespace internal {

inline std::string_view ToStringView(std::string_view str) { return str; }

// We avoid calling std::string_view's constructor of a single `const char*`
// argument, as that will call std::char_traits<char>::length() which may not
// support nullptrs.
inline std::string_view ToStringView(const char* str) {
  if (!str) {
    return {};
  }
  return {str, strlen(str)};
}

inline std::string_view ToStringView(char* str) {
  return ToStringView(static_cast<const char*>(str));
}

// Else, default to assuming that std::data will yield a C string.
template <typename Stringlike>
inline std::enable_if_t<!std::is_convertible_v<Stringlike, nullptr_t>, std::string_view>
ToStringView(const Stringlike& str) {
  static_assert(
      std::is_convertible_v<decltype(std::data(std::declval<Stringlike&>())), const char*>);
  return ToStringView(std::data(str));
}

template <typename Stringlike>
inline std::enable_if_t<std::is_convertible_v<Stringlike, nullptr_t>, std::string_view>
ToStringView(const Stringlike& str) {
  return {};
}

}  // namespace internal

// Helper class for handling the error information, plus some logic for printing the correct error
// messages.
class Message {
 public:
  Message() = delete;
  Message(std::string_view desc, const SourceLocation& location);
  Message(const Message&) = delete;
  Message(Message&&) noexcept;
  ~Message();

  Message& operator=(const Message&) = delete;
  Message& operator=(Message&&) = delete;

  // Returns the position at which the message was sent.
  const SourceLocation& location() const { return location_; }

  // Returns the text of the message.
  const std::string& text() const { return text_; }

 private:
  std::string text_;

  SourceLocation location_;
};

// Helper functions used on assertion reporting contexts.
namespace internal {
// Returns a string with the Hex representation of the contents of the buffer pointed by ptr. If
// |ptr| is nullptr, returns "<nullptr>". If |size| is 0 returns <empty>.
std::string ToHex(const void* ptr, size_t size);

// It's not necessarily safe to do pointer arithmetic on volatiles because of alignment issues, so
// just print whether the pointer is nullptr/empty/normal.
std::string_view PrintVolatile(volatile const void* ptr, size_t size);

}  // namespace internal

// Specializations exist for primitive types, pointers and |std::string|.
template <typename T>
std::string PrintValue(const T& value) {
  // TODO(gevalentino): By default generate a hex representation of the memory contents of value.
  return internal::ToHex(&value, sizeof(value));
}

template <typename T>
std::string PrintValue(volatile const T& value) {
  return std::string{internal::PrintVolatile(&value, sizeof(value))};
}

// For pointers just print the address.
template <typename T>
std::string PrintValue(const T* value) {
  if (value == nullptr) {
    return "<nullptr>";
  }
  return std::string{fbl::StringPrintf("%p", static_cast<const void*>(value))};
}

// Explicit overloads for integers, floating point, char pointers, and strings.
std::string PrintValue(int32_t value);
std::string PrintValue(uint32_t value);
std::string PrintValue(int64_t value);
std::string PrintValue(uint64_t& value);
std::string PrintValue(float value);
std::string PrintValue(double value);
std::string PrintValue(const char* value);
inline std::string PrintValue(std::string str) { return str; }
inline std::string PrintValue(std::string_view str) { return std::string{str}; }

// Print a string form of the status, can't be a specialization of PrintValue because zx_status_t is
// a uint32_t.
std::string PrintStatus(zx_status_t status);

// For tuples, recursively print the individual components.
template <typename... Ts>
std::string PrintValue(const std::tuple<Ts...>& value) {
  const auto strings = std::apply(
      [&](auto&&... elems) {
        return std::array<std::string, sizeof...(Ts)>{
            PrintValue(std::forward<decltype(elems)>(elems))...};
      },
      value);

  // Total size of all string representations, plus:
  // * 1 for the opening "{".
  // * 1 for each " " between representations, plus a "," if it is not the last representation.
  // * 2 for the closing " }".
  size_t total_size = 0;
  for (const auto& s : strings) {
    total_size += s.size();
  }
  total_size += 1 + ((2 * strings.size()) - 1) + 2;

  char buffer[total_size];
  size_t current = 0;
  buffer[current++] = '{';
  for (size_t index = 0; index < strings.size(); ++index) {
    buffer[current++] = ' ';
    std::memcpy(buffer + current, strings[index].data(), strings[index].size());
    current += strings[index].size();
    if (index + 1 < strings.size()) {
      buffer[current++] = ',';
    }
  }
  buffer[current++] = ' ';
  buffer[current++] = '}';
  return std::string(buffer, current);
}

template <typename StringTypeA, typename StringTypeB>
inline bool StrCmp(StringTypeA&& actual, StringTypeB&& expected) {
  std::string_view actual_sv = internal::ToStringView(actual);
  std::string_view expected_sv = internal::ToStringView(expected);
  return actual_sv == expected_sv;
}

template <typename StringTypeA, typename StringTypeB>
inline bool StrContain(StringTypeA&& str, StringTypeB&& substr) {
  std::string_view str_view = internal::ToStringView(str);
  std::string_view substr_view = internal::ToStringView(substr);
  return str_view.contains(substr_view);
}

}  // namespace zxtest

#endif  // ZXTEST_BASE_MESSAGE_H_
