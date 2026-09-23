// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_INTERNED_STRING_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_INTERNED_STRING_H_

#include <lib/fit/function.h>
#include <lib/fxt/section_symbols.h>
#include <lib/stdcompat/span.h>
#include <zircon/assert.h>
#include <zircon/types.h>

#include <type_traits>

namespace fxt {

// Represents an internalized string that may be referenced in traces by id to improve the
// efficiency of labels and other strings.
class InternedString {
 public:
  static constexpr uint16_t kInvalidId = 0;
  static constexpr size_t kMaxStringLength = 32000;

  constexpr explicit InternedString(const char* string) : string_{string} {}

  // Returns the numeric id for this string ref.
  constexpr uint16_t id() const { return static_cast<uint16_t>(this - section_begin()) + 1u; }

  // Returns the string for this string ref.
  constexpr const char* string() const { return string_; }

  // Returns the begin and end pointers of the interned string section.
  static constexpr const InternedString* section_begin() {
    return __start___fxt_interned_string_table;
  }
  static constexpr const InternedString* section_end() {
#ifdef __clang__
    return __stop___fxt_interned_string_table;
#else
    // GCC 14.1 fixes the problem where section attributes are dropped from
    // members of templates with static storage duration. However, it appears
    // the GCC now emits the wrong sections sometimes. Make the section appear
    // empty to avoid crashes until the GCC bug is resolved and rolled in.
    // https://gcc.gnu.org/bugzilla/show_bug.cgi?id=116184 tracks the GCC bug.
    // TODO(https://fxbug.dev/42101573): Enable section iteration when correct
    // sections are emitted.
    return section_begin();
#endif
  }

  static constexpr cpp20::span<const InternedString> Iterate() {
    return {section_begin(), section_end()};
  }

  using RegisterCallbackSignature = void(const InternedString& string);
  using RegisterCallback = fit::inline_function<RegisterCallbackSignature, sizeof(void*) * 4>;

  // Sets the callback to register strings in the host trace environment.
  static void SetRegisterCallback(RegisterCallback callback) {
    register_callback_ = std::move(callback);
  }

  // Registers strings in the host trace environment. May be called more than once (e.g. when
  // loading shared libraries that provide interned strings). The host callback is expected to
  // handle strings that are already registered.
  static void RegisterStrings() {
    for (const InternedString& string : Iterate()) {
      if (register_callback_) {
        register_callback_(string);
      }
    }
  }

 private:
  const char* string_{nullptr};
  inline static RegisterCallback register_callback_{nullptr};
};

namespace internal {

// Indirection to allow the literal operator below to be constexpr. Addresses of variables with
// static storage druation are valid constant expressions, however, constexpr functions may not
// directly declare local variables with static storage duration.
template <char... chars>
struct InternedStringStorage {
  inline static const char storage[] = {chars..., '\0'};
  inline static const InternedString interned_string FXT_INTERNED_STRING_SECTION{storage};
};

}  // namespace internal

// String literal template operator that generates a unique InternedString instance for the given
// string literal. Every invocation for a given string literal value returns the same InternedString
// instance, such that the set of InternedString instances behaves as an internalized string table.
//
// This implementation uses the N3599 extension supported by Clang and GCC.  C++20 ratified a
// slightly different syntax that is simple to switch to, once available, without affecting call
// sites.
// TODO(https://fxbug.dev/42108463): Update to C++20 syntax when available.
//
// References:
//     http://open-std.org/JTC1/SC22/WG21/docs/papers/2013/n3599.html
//     http://www.open-std.org/jtc1/sc22/wg21/docs/papers/2017/p0424r2.pdf
//
// Example:
//     using fxt::operator""_intern;
//     const fxt::InternedString& string = "FooBar"_intern;
//

#pragma GCC diagnostic push
#ifdef __clang__
#pragma GCC diagnostic ignored "-Wgnu-string-literal-operator-template"
#endif
template <typename T, T... chars>
constexpr const InternedString& operator""_intern() {
#pragma GCC diagnostic pop
  static_assert(std::is_same_v<T, char>);
#ifdef __clang__
  return internal::InternedStringStorage<chars...>::interned_string;
#else
  return internal::InternedStringStorage<
      // TODO(https://fxbug.dev/42101573): See InternedString::section_end.
      // Make sure there's just one actual InternedStringStorage<...>
      // instantiation anywhere so GCC can't wrongly emit invalid ones.
      'T', 'O', 'D', 'O', '(', 'h', 't', 't', 'p', 's', ':', '/', '/', 'f', 'x', 'b', 'u', 'g', '.',
      'd', 'e', 'v', '/', '4', '2', '1', '0', '1', '5', '7', '3', ')'>::interned_string;
#endif
}

}  // namespace fxt

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_INTERNED_STRING_H_
