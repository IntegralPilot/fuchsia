// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#ifndef SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_LOCK_NAME_HELPER_H_
#define SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_LOCK_NAME_HELPER_H_

#include <lib/fxt/interned_string.h>
#include <stddef.h>

#include <string_view>

namespace lockdep::internal {

template <typename Class, int Line>
class LockNameHelper {
 public:
  static constexpr const fxt::InternedString& GetInternedString() {
    return GetInternedString(std::make_index_sequence<Size>{});
  }

  static constexpr const char* Name() { return GetInternedString().string(); }

 private:
  template <size_t... Is>
  static constexpr const fxt::InternedString& GetInternedString(std::index_sequence<Is...>) {
    return fxt::internal::InternedStringStorage<GetChar(Is)...>::interned_string;
  }

  struct Range {
    const size_t size;
    const size_t offset;
  };

  struct Info {
    const char* const buffer;
    const Range class_name;
    const Range line_num;
  };

  static constexpr const char* kClassPrefix = "Class = ";
#if defined(__clang__)
  static constexpr const char* kLinePrefix = ", Line = ";
#elif defined(__GNUC__)
  static constexpr const char* kLinePrefix = "; int Line = ";
#endif

  static constexpr Info Get() {
    const char* const function_name = __PRETTY_FUNCTION__;
    const size_t class_name_start =
        Find(function_name, kClassPrefix) + std::string_view(kClassPrefix).size();
    const size_t class_name_end =
        Find(function_name + class_name_start, kLinePrefix) + class_name_start;
    const size_t class_name_size = class_name_end - class_name_start;

    const size_t line_num_start = class_name_end + std::string_view(kLinePrefix).size();
    const size_t line_num_end = Find(function_name + line_num_start, "]") + line_num_start;
    const size_t line_num_size = line_num_end - line_num_start;

    return {function_name, {class_name_size, class_name_start}, {line_num_size, line_num_start}};
  }

  inline static constexpr Range ClassName = Get().class_name;
  inline static constexpr Range LineNum = Get().line_num;
  inline static constexpr size_t Size = ClassName.size + LineNum.size + 2;
  inline static constexpr const char* Data = Get().buffer;

  static constexpr size_t Find(const char* string, const char* substring) {
    size_t i = 0;

    while (string[i]) {
      for (size_t j = 0; true; ++j) {
        if (!substring[j]) {
          return i;
        }

        if (string[i + j] != substring[j]) {
          break;
        }
      }
      ++i;
    }

    return i;
  }

  // A helper method which makes the index_sequence constructor's initializers a
  // bit easier to read.  This method takes the index of our constant string and
  // returns the character which belongs at that position.
  inline static constexpr char GetChar(size_t index) {
    // If we are within the class name size, return a character from there.
    if (index < ClassName.size)
      return Data[index + ClassName.offset];

    // If we are one after the class name, add the ':' separator.
    if (index == ClassName.size)
      return ':';

    // If we are still within the buffer, then the character must be from the
    // line number section.
    if (index < Size - 1)
      return Data[index - ClassName.size - 1 + LineNum.offset];

    // Anything else means we are at, or beyond, the end of our string.  Return
    // the null terminator.
    return '\0';
  }
};

}  // namespace lockdep::internal

#endif  // SRC_LIB_LOCKDEP_INCLUDE_LOCKDEP_LOCK_NAME_HELPER_H_
