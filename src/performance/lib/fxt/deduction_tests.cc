// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fxt/argument.h>
#include <lib/fxt/interned_string.h>
#include <lib/fxt/string_ref.h>
#include <lib/fxt/thread_ref.h>

#include <gtest/gtest.h>

namespace {

using fxt::operator""_intern;

struct ConvertibleToRefId {
  operator fxt::StringRef<fxt::RefType::kId>() const { return fxt::StringRef{1}; }
  operator fxt::ThreadRef<fxt::RefType::kId>() const { return fxt::ThreadRef{1}; }
};

struct ConvertibleToRefInline {
  operator fxt::StringRef<fxt::RefType::kInline>() const { return fxt::StringRef{"inline"}; }
  operator fxt::ThreadRef<fxt::RefType::kInline>() const { return fxt::ThreadRef{1, 2}; }
};

struct NotConvertible {};

TEST(Types, StringRef) {
  [[maybe_unused]] fxt::StringRef ref_id = ConvertibleToRefId{};
  [[maybe_unused]] fxt::StringRef ref_inline = ConvertibleToRefInline{};
  [[maybe_unused]] fxt::StringRef ref_intern = "test"_intern;
#if 0 || DOES_NOT_COMPILE
  [[maybe_unused]] fxt::StringRef ref_invalid = NotConvertible{};
#endif
}

TEST(Types, ThreadRef) {
  [[maybe_unused]] fxt::ThreadRef ref_id = ConvertibleToRefId{};
  [[maybe_unused]] fxt::ThreadRef ref_inline = ConvertibleToRefInline{};
#if 0 || DOES_NOT_COMPILE
  [[maybe_unused]] fxt::ThreadRef ref_invalid = NotConvertible{};
#endif
}

TEST(Types, Argument) {
  [[maybe_unused]] fxt::Argument arg_null_id = {ConvertibleToRefId{}};
  [[maybe_unused]] fxt::Argument arg_null_inline = {ConvertibleToRefInline{}};
  [[maybe_unused]] fxt::Argument arg_bool_id = {ConvertibleToRefId{}, false};
  [[maybe_unused]] fxt::Argument arg_bool_inline = {ConvertibleToRefInline{}, false};
  [[maybe_unused]] fxt::Argument arg_int32_id = {ConvertibleToRefId{}, int32_t{0}};
  [[maybe_unused]] fxt::Argument arg_int32_inline = {ConvertibleToRefInline{}, int32_t{0}};
  [[maybe_unused]] fxt::Argument arg_uint32_id = {ConvertibleToRefId{}, uint32_t{0}};
  [[maybe_unused]] fxt::Argument arg_uint32_inline = {ConvertibleToRefInline{}, uint32_t{0}};
  [[maybe_unused]] fxt::Argument arg_int64_id = {ConvertibleToRefId{}, int64_t{0}};
  [[maybe_unused]] fxt::Argument arg_int64_inline = {ConvertibleToRefInline{}, int64_t{0}};
  [[maybe_unused]] fxt::Argument arg_uint64_id = {ConvertibleToRefId{}, uint64_t{0}};
  [[maybe_unused]] fxt::Argument arg_uint64_inline = {ConvertibleToRefInline{}, uint64_t{0}};
  [[maybe_unused]] fxt::Argument arg_pointer_id = {ConvertibleToRefId{}, fxt::Pointer{0}};
  [[maybe_unused]] fxt::Argument arg_pointer_inline = {ConvertibleToRefInline{}, fxt::Pointer{0}};
  [[maybe_unused]] fxt::Argument arg_koid_id = {ConvertibleToRefId{}, fxt::Koid{0}};
  [[maybe_unused]] fxt::Argument arg_koid_inline = {ConvertibleToRefInline{}, fxt::Koid{0}};
  [[maybe_unused]] fxt::Argument arg_string_id_id = {ConvertibleToRefId{}, ConvertibleToRefId{}};
  [[maybe_unused]] fxt::Argument arg_string_inline_id = {ConvertibleToRefInline{},
                                                         ConvertibleToRefId{}};
  [[maybe_unused]] fxt::Argument arg_string_inline_inline = {ConvertibleToRefInline{},
                                                             ConvertibleToRefInline{}};
  [[maybe_unused]] fxt::Argument arg_string_id_inline = {ConvertibleToRefId{},
                                                         ConvertibleToRefInline{}};
  [[maybe_unused]] fxt::Argument arg_string_intern_intern = {"foo"_intern, "bar"_intern};
#if 0 || DOES_NOT_COMPILE
  [[maybe_unused]] fxt::Argument arg_null_invalid = {NotConvertible{}};
  [[maybe_unused]] fxt::Argument arg_bool_invalid = {NotConvertible{}, false};
  [[maybe_unused]] fxt::Argument arg_int32_invalid = {NotConvertible{}, int32_t{0}};
  [[maybe_unused]] fxt::Argument arg_uint32_invalid = {NotConvertible{}, uint32_t{0}};
  [[maybe_unused]] fxt::Argument arg_int64_invalid = {NotConvertible{}, int64_t{0}};
  [[maybe_unused]] fxt::Argument arg_uint64_invalid = {NotConvertible{}, uint64_t{0}};
  [[maybe_unused]] fxt::Argument arg_pointer_invalid = {NotConvertible{}, fxt::Pointer{0}};
  [[maybe_unused]] fxt::Argument arg_koid_invalid = {NotConvertible{}, fxt::Koid{0}};
  [[maybe_unused]] fxt::Argument arg_string_invalid_invalid = {NotConvertible{},
                                                     NotConvertible{}};
  [[maybe_unused]] fxt::Argument arg_string_id_invalid = {ConvertibleToRefId{},
                                                     NotConvertible{}};
  [[maybe_unused]] fxt::Argument arg_string_invalid_id = {NotConvertible{},
                                                     ConvertibleToRefId{}};
  [[maybe_unused]] fxt::Argument arg_string_inline_invalid = {ConvertibleToRefInline{},
                                                     NotConvertible{}};
  [[maybe_unused]] fxt::Argument arg_string_invalid_inline = {NotConvertible{},
                                                     ConvertibleToRefInline{}};
#endif
}

}  // namespace
