// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// Given a Writer implementing the Writer interface in writer-internal.h, provide an api
// over the writer to allow serializing fxt to the Writer.
//
// Based heavily on libTrace in zircon/system/ulib/trace to allow compatibility,
// but modified to enable passing in an arbitrary buffering system.

#ifndef SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_STRING_REF_H_
#define SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_STRING_REF_H_

#include <stdint.h>
#include <string.h>
#include <zircon/assert.h>

#include <type_traits>

#include "interned_string.h"
#include "record_types.h"

namespace fxt {

// Represents an FXT StringRecord which is either inline in the record body, or
// an index included in the record header.
//
// See also: https://fuchsia.dev/fuchsia-src/reference/tracing/trace-format#string-record
template <RefType>
class StringRef;

template <typename T, RefType ref_type>
using EnableIfConvertibleToStringRef =
    std::enable_if_t<(std::is_convertible_v<T, StringRef<ref_type>> &&
                      !std::is_same_v<T, StringRef<ref_type>>),
                     bool>;

template <>
class StringRef<RefType::kInline> {
  enum Convert { kConvert };

  static constexpr size_t constexpr_strnlen(char const* string, size_t count) {
    const char* cursor = string;
    while (count-- && *cursor != '\0') {
      ++cursor;
    }
    return cursor - string;
  }

 public:
  static constexpr size_t kMaxStringLength = InternedString::kMaxStringLength;

  template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
  constexpr StringRef(const T& value) : StringRef{kConvert, value} {}

  explicit constexpr StringRef(const char* string, size_t size = kMaxStringLength)
      : string_{string},
        size_{constexpr_strnlen(string, size < kMaxStringLength ? size : kMaxStringLength)} {}

  constexpr StringRef(const StringRef&) = default;
  constexpr StringRef& operator=(const StringRef&) = default;

  constexpr WordSize PayloadSize() const { return WordSize::FromBytes(size_); }

  constexpr uint64_t HeaderEntry() const { return 0x8000 | size_; }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {
    res.WriteBytes(string_, size_);
  }

  constexpr const char* string() const { return string_; }
  constexpr size_t size() const { return size_; }

  constexpr bool operator==(const StringRef& other) const {
    return string() == other.string() && size() == other.size();
  }
  constexpr bool operator!=(const StringRef& other) const { return !(*this == other); }

 private:
  constexpr StringRef(Convert, const StringRef& value)
      : string_{value.string_}, size_{value.size_} {}

  const char* string_;
  size_t size_;
};

StringRef(const char*) -> StringRef<RefType::kInline>;

StringRef(const char*, size_t) -> StringRef<RefType::kInline>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kInline> = true>
StringRef(const T&) -> StringRef<RefType::kInline>;

template <>
class StringRef<RefType::kId> {
  enum Convert { kConvert };

 public:
  template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
  constexpr StringRef(const T& value) : StringRef{kConvert, value} {}

  constexpr explicit StringRef(uint16_t id) : id_(id) {}

  constexpr StringRef(const InternedString& interned_string) : StringRef{interned_string.id()} {}

  constexpr StringRef(const StringRef&) = default;
  constexpr StringRef& operator=(const StringRef&) = default;

  constexpr static WordSize PayloadSize() { return WordSize(0); }

  constexpr uint64_t HeaderEntry() const { return id_; }

  template <typename Reservation>
  constexpr void Write(Reservation& res) const {}

  constexpr uint16_t id() const { return id_; }

  constexpr bool operator==(const StringRef& other) const { return id() == other.id(); }
  constexpr bool operator!=(const StringRef& other) const { return !(*this == other); }

 private:
  constexpr StringRef(Convert, const StringRef& value) : id_{value.id_} {}

  uint16_t id_;
};

StringRef(uint16_t) -> StringRef<RefType::kId>;

StringRef(const InternedString&) -> StringRef<RefType::kId>;

template <typename T, EnableIfConvertibleToStringRef<T, RefType::kId> = true>
StringRef(const T&) -> StringRef<RefType::kId>;

}  // namespace fxt

#endif  // SRC_PERFORMANCE_LIB_FXT_INCLUDE_LIB_FXT_STRING_REF_H_
