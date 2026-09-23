// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <string_view>

#include <efi/string/string.h>
#include <fbl/vector.h>
#include <gtest/gtest.h>

namespace fbl {

template <typename T>
inline bool operator==(const Vector<T>& a, const Vector<T>& b) {
  return std::equal(a.begin(), a.end(), b.begin(), b.end());
}

template <typename T>
inline bool operator!=(const Vector<T>& a, const Vector<T>& b) {
  return !(a == b);
}

}  // namespace fbl

namespace {

using namespace std::string_view_literals;

TEST(String, ConstructDefault) {
  efi::String string;

  ASSERT_TRUE(string.IsValid());
  ASSERT_EQ(string, "");
  ASSERT_EQ(string, u"");
}

TEST(String, ConstructUtf8) {
  efi::String string("abc 123");

  ASSERT_TRUE(string.IsValid());
  ASSERT_EQ(string, "abc 123");
  ASSERT_EQ(string, u"abc 123");
}

TEST(String, ConstructUtf16) {
  efi::String string(u"abc 123");

  ASSERT_TRUE(string.IsValid());
  ASSERT_EQ(string, "abc 123");
  ASSERT_EQ(string, u"abc 123");
}

TEST(String, ConstructUtf8Vector) {
  fbl::Vector<char> vector{'a', 'b', 'c', ' ', '1', '2', '3', '\0'};
  efi::String string(std::move(vector));

  ASSERT_TRUE(string.IsValid());
  ASSERT_EQ(string, "abc 123");
  ASSERT_EQ(string, u"abc 123");
}

TEST(String, ConstructUtf16Vector) {
  fbl::Vector<char16_t> vector{u'a', u'b', u'c', u' ', u'1', u'2', u'3', u'\0'};
  efi::String string(std::move(vector));

  ASSERT_TRUE(string.IsValid());
  ASSERT_EQ(string, "abc 123");
  ASSERT_EQ(string, u"abc 123");
}

TEST(String, Copy) {
  efi::String original("abc 123");
  efi::String copy(original);

  for (const auto& string : {original, copy}) {
    ASSERT_TRUE(string.IsValid());
    ASSERT_EQ(string, "abc 123");
    ASSERT_EQ(string, u"abc 123");
  }
}

TEST(String, Move) {
  efi::String original("abc 123");
  efi::String final(std::move(original));

  ASSERT_FALSE(original.IsValid());
  ASSERT_TRUE(final.IsValid());
  ASSERT_EQ(final, "abc 123");
  ASSERT_EQ(final, u"abc 123");
}

// Make sure [in]equality operators are defined for String and {String, c-string, string_view}.
TEST(String, Equality) {
  efi::String string("abc 123");
  efi::String same(string);
  efi::String different("foo bar");

  ASSERT_EQ(string, same);
  ASSERT_EQ(string, "abc 123");
  ASSERT_EQ(string, u"abc 123");
  ASSERT_EQ(string, "abc 123"sv);
  ASSERT_EQ(string, u"abc 123"sv);

  ASSERT_NE(string, different);
  ASSERT_NE(string, "foo bar");
  ASSERT_NE(string, u"foo bar");
  ASSERT_NE(string, "foo bar"sv);
  ASSERT_NE(string, u"foo bar"sv);
}

TEST(String, EmbeddedNulls) {
  efi::String string("\0abc\0123\0"sv);

  ASSERT_TRUE(string.IsValid());
  ASSERT_EQ(string, "\0abc\0123\0"sv);
  ASSERT_EQ(string, u"\0abc\0123\0"sv);
}

TEST(String, ToUtf8) {
  ASSERT_EQ(efi::String::ToUtf8(u"abc 123"),
            (fbl::Vector<char>{'a', 'b', 'c', ' ', '1', '2', '3', '\0'}));
}

TEST(String, ToUtf16) {
  ASSERT_EQ(efi::String::ToUtf16("abc 123"),
            (fbl::Vector<char16_t>{u'a', u'b', u'c', u' ', u'1', u'2', u'3', u'\0'}));
}

}  // namespace
