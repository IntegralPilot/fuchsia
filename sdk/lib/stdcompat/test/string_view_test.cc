// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdcompat/string_view.h>
#include <lib/stdcompat/version.h>

#include <cstring>
#include <limits>
#include <sstream>
#include <stdexcept>
#include <string>

#include "gtest.h"
#include "test_helper.h"

namespace {

TEST(StringViewTest, StartsWith) {
  constexpr std::string_view kString = "ABCdef";

  // By convention, a string view always "starts" with an empty or NUL string.
  EXPECT_TRUE(cpp20::starts_with(kString, std::string_view{}));
  EXPECT_TRUE(cpp20::starts_with(kString, ""));
  EXPECT_TRUE(cpp20::starts_with(kString, std::string_view{""}));
  EXPECT_TRUE(cpp20::starts_with(kString, std::string{""}));

  EXPECT_TRUE(cpp20::starts_with(kString, 'A'));
  EXPECT_FALSE(cpp20::starts_with(kString, 'B'));
  EXPECT_FALSE(cpp20::starts_with(kString, 'f'));

  EXPECT_TRUE(cpp20::starts_with(kString, "A"));
  EXPECT_TRUE(cpp20::starts_with(kString, std::string_view{"A"}));
  EXPECT_TRUE(cpp20::starts_with(kString, std::string{"A"}));

  EXPECT_TRUE(cpp20::starts_with(kString, "AB"));
  EXPECT_TRUE(cpp20::starts_with(kString, std::string_view{"AB"}));
  EXPECT_TRUE(cpp20::starts_with(kString, "ABC"));
  EXPECT_TRUE(cpp20::starts_with(kString, std::string_view{"ABC"}));
  EXPECT_TRUE(cpp20::starts_with(kString, "ABCd"));
  EXPECT_TRUE(cpp20::starts_with(kString, std::string_view{"ABCd"}));
  EXPECT_TRUE(cpp20::starts_with(kString, "ABCde"));
  EXPECT_TRUE(cpp20::starts_with(kString, std::string_view{"ABCde"}));

  // A string view should start with itself.
  EXPECT_TRUE(cpp20::starts_with(kString, "ABCdef"));
  EXPECT_TRUE(cpp20::starts_with(kString, kString));
  EXPECT_TRUE(cpp20::starts_with(kString, "ABCdef\0"));
  EXPECT_TRUE(cpp20::starts_with(kString, std::string_view{"ABCdef\0"}));

  EXPECT_FALSE(cpp20::starts_with(kString, "rAnDoM"));
  EXPECT_FALSE(cpp20::starts_with(kString, std::string_view{"rAnDoM"}));
  EXPECT_FALSE(cpp20::starts_with(kString, "longer than kString"));
  EXPECT_FALSE(cpp20::starts_with(kString, std::string_view{"longer than kString"}));
}

TEST(StringViewTest, EndsWith) {
  constexpr std::string_view kString = "ABCdef";

  // By convention, a string view always "ends" with an empty or NUL string.
  EXPECT_TRUE(cpp20::ends_with(kString, std::string_view{}));
  EXPECT_TRUE(cpp20::ends_with(kString, ""));
  EXPECT_TRUE(cpp20::ends_with(kString, std::string_view{""}));
  EXPECT_TRUE(cpp20::ends_with(kString, std::string{""}));

  EXPECT_TRUE(cpp20::ends_with(kString, 'f'));
  EXPECT_FALSE(cpp20::ends_with(kString, 'e'));
  EXPECT_FALSE(cpp20::ends_with(kString, 'A'));

  EXPECT_TRUE(cpp20::ends_with(kString, "f"));
  EXPECT_TRUE(cpp20::ends_with(kString, std::string_view{"f"}));
  EXPECT_TRUE(cpp20::ends_with(kString, std::string{"f"}));
  EXPECT_TRUE(cpp20::ends_with(kString, "ef"));
  EXPECT_TRUE(cpp20::ends_with(kString, std::string_view{"ef"}));
  EXPECT_TRUE(cpp20::ends_with(kString, "def"));
  EXPECT_TRUE(cpp20::ends_with(kString, std::string_view{"def"}));
  EXPECT_TRUE(cpp20::ends_with(kString, "Cdef"));
  EXPECT_TRUE(cpp20::ends_with(kString, std::string_view{"Cdef"}));
  EXPECT_TRUE(cpp20::ends_with(kString, "BCdef"));
  EXPECT_TRUE(cpp20::ends_with(kString, std::string_view{"BCdef"}));

  // A string view should end with itself.
  EXPECT_TRUE(cpp20::ends_with(kString, "ABCdef"));
  EXPECT_TRUE(cpp20::ends_with(kString, kString));
  EXPECT_TRUE(cpp20::ends_with(kString, "ABCdef\0"));
  EXPECT_TRUE(cpp20::ends_with(kString, std::string_view{"ABCdef\0"}));

  EXPECT_FALSE(cpp20::ends_with(kString, "rAnDoM"));
  EXPECT_FALSE(cpp20::ends_with(kString, std::string_view{"rAnDoM"}));
  EXPECT_FALSE(cpp20::ends_with(kString, "longer than kString"));
  EXPECT_FALSE(cpp20::ends_with(kString, std::string_view{"longer than kString"}));
}

TEST(StringViewTest, Contains) {
  constexpr std::string_view kString = "Foo is Bar and Baz is Foo Bar";

  // string_view query.
  constexpr std::string_view kSvQueryPresent = "Foo";
  constexpr std::string_view kSvQueryMissing = "foobar";

  EXPECT_TRUE(cpp23::contains(kString, kSvQueryPresent));
  EXPECT_FALSE(cpp23::contains(kString, kSvQueryMissing));

  // character
  constexpr char kCharQueryPresent = 'F';
  constexpr char kCharQueryMissing = 'Q';
  EXPECT_TRUE(cpp23::contains(kString, kCharQueryPresent));
  EXPECT_FALSE(cpp23::contains(kString, kCharQueryMissing));

  // C-string
  constexpr const char* kCstringQueryPresent = "Foo";
  constexpr const char* kCstringQueryMissing = "foobar";
  EXPECT_TRUE(cpp23::contains(kString, kCstringQueryPresent));
  EXPECT_FALSE(cpp23::contains(kString, kCstringQueryMissing));
}

TEST(StringViewTest, CalledWithString) {
  constexpr std::string_view kString = "abcdef";
  const std::string str{kString};
  EXPECT_TRUE(cpp20::starts_with(str, kString));
  EXPECT_TRUE(cpp20::starts_with(str, str.c_str()));
  EXPECT_TRUE(cpp20::starts_with(str, kString.front()));
  EXPECT_TRUE(cpp20::ends_with(str, kString));
  EXPECT_TRUE(cpp20::ends_with(str, str.c_str()));
  EXPECT_TRUE(cpp20::ends_with(str, kString.back()));
  EXPECT_TRUE(cpp23::contains(str, kString));
  EXPECT_TRUE(cpp23::contains(str, str.c_str()));
  EXPECT_TRUE(cpp23::contains(str, kString.front()));

  constexpr std::wstring_view kWString = L"abcdef";
  const std::wstring wstr{kWString};
  EXPECT_TRUE(cpp20::starts_with(wstr, kWString));
  EXPECT_TRUE(cpp20::starts_with(wstr, wstr.c_str()));
  EXPECT_TRUE(cpp20::starts_with(wstr, kWString.front()));
  EXPECT_TRUE(cpp20::ends_with(wstr, kWString));
  EXPECT_TRUE(cpp20::ends_with(wstr, wstr.c_str()));
  EXPECT_TRUE(cpp20::ends_with(wstr, kWString.back()));
  EXPECT_TRUE(cpp23::contains(wstr, kWString));
  EXPECT_TRUE(cpp23::contains(wstr, wstr.c_str()));
  EXPECT_TRUE(cpp23::contains(wstr, kWString.front()));

#if __cpp_char8_t >= 201811L
  constexpr std::u8string_view kU8String = u8"abcdef";
  const std::u8string u8str{kU8String};
  EXPECT_TRUE(cpp20::starts_with(u8str, kU8String));
  EXPECT_TRUE(cpp20::starts_with(u8str, u8str.c_str()));
  EXPECT_TRUE(cpp20::starts_with(u8str, kU8String.front()));
  EXPECT_TRUE(cpp20::ends_with(u8str, kU8String));
  EXPECT_TRUE(cpp20::ends_with(u8str, u8str.c_str()));
  EXPECT_TRUE(cpp20::ends_with(u8str, kU8String.back()));
  EXPECT_TRUE(cpp23::contains(u8str, kU8String));
  EXPECT_TRUE(cpp23::contains(u8str, u8str.c_str()));
  EXPECT_TRUE(cpp23::contains(u8str, kU8String.front()));
#endif

  constexpr std::u16string_view kU16String = u"abcdef";
  const std::u16string u16str{kU16String};
  EXPECT_TRUE(cpp20::starts_with(u16str, kU16String));
  EXPECT_TRUE(cpp20::starts_with(u16str, u16str.c_str()));
  EXPECT_TRUE(cpp20::starts_with(u16str, kU16String.front()));
  EXPECT_TRUE(cpp20::ends_with(u16str, kU16String));
  EXPECT_TRUE(cpp20::ends_with(u16str, u16str.c_str()));
  EXPECT_TRUE(cpp20::ends_with(u16str, kU16String.back()));
  EXPECT_TRUE(cpp23::contains(u16str, kU16String));
  EXPECT_TRUE(cpp23::contains(u16str, u16str.c_str()));
  EXPECT_TRUE(cpp23::contains(u16str, kU16String.front()));

  constexpr std::u32string_view kU32String = U"abcdef";
  const std::u32string u32str{kU32String};
  EXPECT_TRUE(cpp20::starts_with(u32str, kU32String));
  EXPECT_TRUE(cpp20::starts_with(u32str, u32str.c_str()));
  EXPECT_TRUE(cpp20::starts_with(u32str, kU32String.front()));
  EXPECT_TRUE(cpp20::ends_with(u32str, kU32String));
  EXPECT_TRUE(cpp20::ends_with(u32str, u32str.c_str()));
  EXPECT_TRUE(cpp20::ends_with(u32str, kU32String.back()));
  EXPECT_TRUE(cpp23::contains(u32str, kU32String));
  EXPECT_TRUE(cpp23::contains(u32str, u32str.c_str()));
  EXPECT_TRUE(cpp23::contains(u32str, kU32String.front()));
}

}  // namespace
