// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_INTERNED_CATEGORY_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_INTERNED_CATEGORY_H_

#include <lib/fit/function.h>
#include <lib/fxt/interned_string.h>
#include <lib/fxt/section_symbols.h>
#include <lib/stdcompat/span.h>
#include <zircon/types.h>

#include <atomic>

namespace fxt {

// Represents an internalized tracing category and its associated index in the enabled categories
// vector.
//
// This type uses the same linker section collection method as InternedString. See
// lib/fxt/interned_string.h for more details.
//
class InternedCategory {
 public:
  static constexpr uint32_t kInvalidIndex = -1;

  constexpr explicit InternedCategory(const InternedString& label) : label_{label} {}

  constexpr const InternedString& label() const { return label_; }
  constexpr const char* string() const { return label_.string(); }
  uint32_t index() const { return index_.load(std::memory_order_acquire); }

  // Sets the index for the category if it has the expected previous value. This provides a simple
  // way to prevent re-initialization if RegisterCategories() is called more than once, while also
  // providing a way to override the value. This can be removed once the index can be automatically
  // derived from the section offset when the kernel supports extensible categories.
  void SetIndex(uint32_t index, uint32_t expected = kInvalidIndex) const {
    index_.compare_exchange_strong(expected, index, std::memory_order_acq_rel);
  }

  // Returns the begin and end pointers of the interned category section.
  static constexpr const InternedCategory* section_begin() {
    return __start___fxt_interned_category_table;
  }
  static constexpr const InternedCategory* section_end() {
#ifdef __clang__
    return __stop___fxt_interned_category_table;
#else
    // GCC 14.1 fixes the problem where section attributes are dropped from members of templates
    // with static storage duration. However, it appears the GCC now emits the wrong sections
    // sometimes. Make the section appear empty to avoid crashes until that bug is resolved.
    // TODO(https://fxbug.dev/42101573): Enable section iteration when correct sections are emitted.
    return section_begin();
#endif
  }

  static constexpr cpp20::span<const InternedCategory> Iterate() {
    return {section_begin(), section_end()};
  }

  using RegisterCallbackSignature = uint32_t(const InternedCategory& category);
  using RegisterCallback = fit::inline_function<RegisterCallbackSignature, sizeof(void*) * 4>;

  // Sets the callback to register categories in the host trace environment.
  static void SetRegisterCallback(RegisterCallback callback) {
    register_callback_ = std::move(callback);
  }

  // Registers categories in the host trace environment. May be called more than once (e.g. when
  // loading shared libraries that provide interned categories). The host callback is expected to
  // handle categories that are already registered.
  static void RegisterCategories() {
    for (const InternedCategory& category : Iterate()) {
      if (register_callback_) {
        register_callback_(category);
      }
    }
  }

 private:
  const InternedString& label_;
  mutable std::atomic<uint32_t> index_{kInvalidIndex};
  inline static RegisterCallback register_callback_;
};

namespace internal {

// Indirection to allow the literal operator below to be constexpr. Addresses of variables with
// static storage duration are valid constant expressions, however, constexpr functions may not
// directly declare local variables with static storage duration.
template <char... chars>
struct InternedCategoryStorage {
  inline static InternedCategory interned_category FXT_INTERNED_CATEGORY_SECTION{
      InternedStringStorage<chars...>::interned_string};

  // Since InternedCategory has mutable members it must not be placed in a read-only data section.
  static_assert(!std::is_const_v<decltype(interned_category)>);
};

}  // namespace internal

// String literal template operator that generates a unique InternedCategory instance for the given
// string literal. Every invocation for a given string literal value returns the same
// InternedCategory instance, such that the set of InternedCategory instances behaves as an
// internalized category table.
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
//     using fxt::operator""_category;
//     const fxt::InternedCategory& category = "FooBar"_category;
//
#pragma GCC diagnostic push
#ifdef __clang__
#pragma GCC diagnostic ignored "-Wgnu-string-literal-operator-template"
#endif
template <typename T, T... chars>
constexpr const InternedCategory& operator""_category() {
#pragma GCC diagnostic pop
  static_assert(std::is_same_v<T, char>);
  return internal::InternedCategoryStorage<chars...>::interned_category;
}

}  // namespace fxt

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_INTERNED_CATEGORY_H_
