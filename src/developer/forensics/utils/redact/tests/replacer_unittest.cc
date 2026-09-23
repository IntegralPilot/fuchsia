// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/forensics/utils/redact/replacer.h"

#include <lib/inspect/cpp/vmo/types.h>

#include <gmock/gmock.h>
#include <gtest/gtest.h>

#include "src/developer/forensics/utils/redact/cache.h"

namespace forensics {
namespace {

template <typename T>
std::string GetTestName(const testing::TestParamInfo<T>& info) {
  return info.param.test_name;
}

struct RegexpTestParam {
  std::string test_name;
  std::string pattern;
  std::string replacement;

  // nullopt means |pattern| is bad.
  std::optional<std::string> text{std::nullopt};
  std::optional<std::string> expected_output{std::nullopt};
};

class TextReplacerTest : public ::testing::Test,
                         public testing::WithParamInterface<RegexpTestParam> {};
INSTANTIATE_TEST_SUITE_P(TextReplacement, TextReplacerTest,
                         ::testing::ValuesIn(std::vector<RegexpTestParam>({
                             {
                                 "BadRegexp",
                                 "[",
                                 "unused",
                             },
                             {
                                 "Numbers",
                                 "\\d+",
                                 "<NUMBER>",
                                 "9 8 7 abc65",
                                 "<NUMBER> <NUMBER> <NUMBER> abc<NUMBER>",
                             },
                         })),
                         GetTestName<RegexpTestParam>);

TEST_P(TextReplacerTest, ReplaceWithText) {
  RegexpTestParam param = GetParam();

  RedactionIdCache cache(inspect::UintProperty{});
  Replacer replacer = ReplaceWithText(param.pattern, param.replacement);
  if (param.text == std::nullopt || param.expected_output == std::nullopt) {
    EXPECT_EQ(replacer, nullptr);
  } else {
    ASSERT_NE(replacer, nullptr);
    EXPECT_EQ(replacer(cache, *param.text), *param.expected_output);
  }
}

class IdReplacerTest : public ::testing::Test,
                       public testing::WithParamInterface<RegexpTestParam> {};
INSTANTIATE_TEST_SUITE_P(IdReplacement, IdReplacerTest,
                         ::testing::ValuesIn(std::vector<RegexpTestParam>({
                             {
                                 "BadRegexp",
                                 "[",
                                 "unused",
                             },
                             {
                                 "MissingCapture",
                                 "\\d+",
                                 "unused",
                             },
                             {
                                 "TooManyCaptures",
                                 "(\\d+) (\\d+)",
                                 "unused",
                             },
                             {
                                 "MissingFormatSpecifier",
                                 "(\\d+)",
                                 "unused",
                             },
                             {
                                 "TooManyFormatSpecifiers",
                                 "(\\d+)",
                                 "%d %d",
                             },
                             {
                                 "Numbers",
                                 "(\\d+)",
                                 "<NUMBER: %d>",
                                 "9 8 7 abc65",
                                 "<NUMBER: 1> <NUMBER: 2> <NUMBER: 3> abc<NUMBER: 4>",
                             },
                             {
                                 "OverlappingMatches",
                                 "(b?c)",
                                 "<bc_or_c: %d>",
                                 "9 8 7 abc65",
                                 "9 8 7 a<bc_or_c: 1>65",
                             },
                         })),
                         GetTestName<RegexpTestParam>);

TEST_P(IdReplacerTest, ReplaceWithIdFormatString) {
  RegexpTestParam param = GetParam();

  RedactionIdCache cache(inspect::UintProperty{});
  Replacer replacer =
      ReplaceWithIdFormatString(param.pattern, param.replacement, /*ignore_prefixes=*/{});
  if (param.text == std::nullopt || param.expected_output == std::nullopt) {
    EXPECT_EQ(replacer, nullptr);
  } else {
    ASSERT_NE(replacer, nullptr);
    EXPECT_EQ(replacer(cache, *param.text), *param.expected_output);
  }
}

TEST(IdReplacerTests, ReplacementIsShorter) {
  RedactionIdCache cache(inspect::UintProperty{});

  std::string content =
      R"([00050.219][forensics, feedback] INFO: [file_name.cc:80] ID:1234567890abcdefABCDEF0123456789
[00050.220][forensics, feedback] INFO: [file_name.cc:80] ID:1234567890abcdefABCDEF0123456789
[00050.221][forensics, feedback] INFO: [file_name.cc:80] ID:1234567890abcdefABCDEF0123456789)";

  static constexpr std::string_view kExpectedContent =
      R"([00050.219][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 1>
[00050.220][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 1>
[00050.221][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 1>)";

  Replacer replacer = ReplaceWithIdFormatString(R"((\b[0-9a-fA-F]{32}\b))", "<REDACTED-HEX: %d>",
                                                /*ignore_prefixes=*/{});

  ASSERT_NE(replacer, nullptr);
  EXPECT_EQ(replacer(cache, content), kExpectedContent);
}

TEST(IdReplacerTests, ReplacementIsLonger) {
  RedactionIdCache cache(inspect::UintProperty{});

  std::string content =
      R"([00050.219][forensics, feedback] INFO: [file_name.cc:80] ID:12345678
[00050.220][forensics, feedback] INFO: [file_name.cc:80] ID:12345678
[00050.221][forensics, feedback] INFO: [file_name.cc:80] ID:12345678)";

  static constexpr std::string_view kExpectedContent =
      R"([00050.219][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 1>
[00050.220][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 1>
[00050.221][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 1>)";

  Replacer replacer = ReplaceWithIdFormatString(R"((\b[0-9a-fA-F]{8}\b))", "<REDACTED-HEX: %d>",
                                                /*ignore_prefixes=*/{});

  ASSERT_NE(replacer, nullptr);
  EXPECT_EQ(replacer(cache, content), kExpectedContent);
}

TEST(IdReplacerTests, VariableOffset) {
  RedactionIdCache cache(inspect::UintProperty{});

  std::string content =
      R"([00050.219][forensics, feedback] INFO: [file_name.cc:80] ID:1234567890abcdefABCDEF0123456789
[00050.220][forensics, feedback] INFO: [file_name.cc:80] ID:abcdef1234567890ABCDEF012345678
[00050.221][forensics, feedback] INFO: [file_name.cc:80] ID:1234567890abcdefABCDEF0123456789)";

  static constexpr std::string_view kExpectedContent =
      R"([00050.219][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 1>
[00050.220][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 2>
[00050.221][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 1>)";

  Replacer replacer = ReplaceWithIdFormatString(R"((\b[0-9a-fA-F]{31,32}\b))", "<REDACTED-HEX: %d>",
                                                /*ignore_prefixes=*/{});

  ASSERT_NE(replacer, nullptr);
  EXPECT_EQ(replacer(cache, content), kExpectedContent);
}

TEST(IdReplacerTests, IgnoresPrefixes) {
  RedactionIdCache cache(inspect::UintProperty{});

  std::string content =
      R"([00050.219][forensics, feedback] INFO: [file_name.cc:80] ID:1234567890abcdefABCDEF0123456789
[00050.220][forensics, feedback] INFO: [file_name.cc:80] elf:1234567890abcdefABCDEF0123456789
[00050.221][forensics, feedback] INFO: [file_name.cc:80] build_id: 1234567890abcdefABCDEF0123456789)";

  static constexpr std::string_view kExpectedContent =
      R"([00050.219][forensics, feedback] INFO: [file_name.cc:80] ID:<REDACTED-HEX: 1>
[00050.220][forensics, feedback] INFO: [file_name.cc:80] elf:1234567890abcdefABCDEF0123456789
[00050.221][forensics, feedback] INFO: [file_name.cc:80] build_id: 1234567890abcdefABCDEF0123456789)";

  const std::vector<std::string> kHexIgnorePrefixes({
      "elf:",
      "build_id: ",
  });

  Replacer replacer = ReplaceWithIdFormatString(R"((\b[0-9a-fA-F]{32}\b))", "<REDACTED-HEX: %d>",
                                                /*ignore_prefixes=*/kHexIgnorePrefixes);

  ASSERT_NE(replacer, nullptr);
  EXPECT_EQ(replacer(cache, content), kExpectedContent);
}

TEST(IdReplacerTests, RedactsIfPrefixWouldBeBeforeBuffer) {
  RedactionIdCache cache(inspect::UintProperty{});

  std::string content = R"(lf:1234567890abcdefABCDEF0123456789)";
  static constexpr std::string_view kExpectedContent = R"(lf:<REDACTED-HEX: 1>)";
  const std::vector<std::string> kHexIgnorePrefixes({"elf:"});

  Replacer replacer = ReplaceWithIdFormatString(R"((\b[0-9a-fA-F]{32}\b))", "<REDACTED-HEX: %d>",
                                                /*ignore_prefixes=*/kHexIgnorePrefixes);

  ASSERT_NE(replacer, nullptr);
  EXPECT_EQ(replacer(cache, content), kExpectedContent);
}

struct IpTestParam {
  std::string test_name;
  std::string text;
  std::string expected_output;
};

class IPv4ReplacerTest : public ::testing::Test, public testing::WithParamInterface<IpTestParam> {};
INSTANTIATE_TEST_SUITE_P(IPv4Replacement, IPv4ReplacerTest,
                         ::testing::ValuesIn(std::vector<IpTestParam>({
                             {
                                 "IPv4",
                                 "IPv4: 8.8.8.8",
                                 "IPv4: <REDACTED-IPV4: 1>",
                             },
                             {
                                 "IPv46",
                                 "IPv46: ::ffff:12.34.56.78",
                                 "IPv46: ::ffff:<REDACTED-IPV4: 1>",
                             },
                             {
                                 "Cleartext",
                                 "current: 0.8.8.8",
                                 "current: 0.8.8.8",
                             },
                             {
                                 "Loopback",
                                 "loopback: 127.8.8.8",
                                 "loopback: 127.8.8.8",
                             },
                             {
                                 "LinkLocal",
                                 "link_local: 169.254.8.8",
                                 "link_local: 169.254.8.8",
                             },
                             {
                                 "LinkLocalMulticast",
                                 "link_local_multicast: 224.0.0.8",
                                 "link_local_multicast: 224.0.0.8",
                             },
                             {
                                 "Broadcast",
                                 "broadcast: 255.255.255.255",
                                 "broadcast: 255.255.255.255",
                             },
                             {
                                 "NotBroadcast",
                                 "not_broadcast: 255.255.255.254",
                                 "not_broadcast: <REDACTED-IPV4: 1>",
                             },
                             {
                                 "NotLinkLocalMulticast",
                                 "not_link_local_multicast: 224.0.1.8",
                                 "not_link_local_multicast: <REDACTED-IPV4: 1>",
                             },
                             {
                                 "Partial",
                                 "partial: 192.168.42.x",
                                 "partial: <REDACTED-IPV4: 1>",
                             },
                             {
                                 "NotPartial",
                                 "not-partial: 192.168.42 x",
                                 "not-partial: 192.168.42 x",
                             },
                             {
                                 "WrongDigits",
                                 "wrong-digits: 192.168.420",
                                 "wrong-digits: 192.168.420",
                             },
                         })),
                         GetTestName<IpTestParam>);

TEST_P(IPv4ReplacerTest, ReplaceIPv4) {
  IpTestParam param = GetParam();

  RedactionIdCache cache(inspect::UintProperty{});
  Replacer replacer = ReplaceIPv4();
  ASSERT_NE(replacer, nullptr);
  EXPECT_EQ(replacer(cache, param.text), param.expected_output);
}

class IPv6ReplacerTest : public ::testing::Test, public testing::WithParamInterface<IpTestParam> {};
INSTANTIATE_TEST_SUITE_P(IPv6Replacement, IPv6ReplacerTest,
                         ::testing::ValuesIn(std::vector<IpTestParam>({
                             {
                                 "IPv46H",
                                 "IPv46h: ::ffff:ab12:34cd",
                                 "IPv46h: ::ffff:<REDACTED-IPV4: 1>",
                             },
                             {
                                 "NotIPv46h",
                                 "not_IPv46h: ::ffff:ab12:34cd:5",
                                 "not_IPv46h: <REDACTED-IPV6: 1>",
                             },
                             {
                                 "IPv6",
                                 "IPv6: 2001:503:eEa3:0:0:0:0:30",
                                 "IPv6: <REDACTED-IPV6: 1>",
                             },
                             {
                                 "IPv6Colon",
                                 "IPv6C: [::/0 via 2082::7d84:c1dc:ab34:656a nic 4]",
                                 "IPv6C: [::/0 via <REDACTED-IPV6: 1> nic 4]",
                             },
                             {
                                 "IPv6LL",
                                 "IPv6LL: fe80::7d84:c1dc:ab34:656a",
                                 "IPv6LL: fe80:<REDACTED-IPV6-LL: 1>",
                             },
                             {
                                 "IPv6LocalMulticast1",
                                 "local_multicast_1: fF41::1234:5678:9aBc",
                                 "local_multicast_1: fF41::1234:5678:9aBc",
                             },
                             {
                                 "IPv6LocalMulticast2",
                                 "local_multicast_2: Ffe2:1:2:33:abcd:ef0:6789:456",
                                 "local_multicast_2: Ffe2:1:2:33:abcd:ef0:6789:456",
                             },
                             {
                                 "IPv6Multicast3",
                                 "multicast: fF43:abcd::ef0:6789:456",
                                 "multicast: fF43:<REDACTED-IPV6-MULTI: 1>",
                             },
                             {
                                 "IPv6fe89",
                                 "link_local_8: fe89:123::4567:8:90",
                                 "link_local_8: fe89:<REDACTED-IPV6-LL: 1>",
                             },
                             {
                                 "IPv6feb2",
                                 "link_local_b: FEB2:123::4567:8:90",
                                 "link_local_b: FEB2:<REDACTED-IPV6-LL: 1>",
                             },
                             {
                                 "IPv6fec1",
                                 "not_link_local: fec1:123::4567:8:90",
                                 "not_link_local: <REDACTED-IPV6: 1>",
                             },
                             {
                                 "IPv6fe71",
                                 "not_link_local_2: fe71:123::4567:8:90",
                                 "not_link_local_2: <REDACTED-IPV6: 1>",
                             },
                             {
                                 "ShortColons",
                                 "not_address_1: 12:34::",
                                 "not_address_1: 12:34::",
                             },
                             {
                                 "ColonsShort",
                                 "not_address_2: ::12:34",
                                 "not_address_2: ::12:34",
                             },
                             {
                                 "ColonsFields3",
                                 "v6_colons_3_fields: ::12:34:5",
                                 "v6_colons_3_fields: <REDACTED-IPV6: 1>",
                             },
                             {
                                 "V6Fields3Colons",
                                 "v6_3_fields_colons: 12:34:5::",
                                 "v6_3_fields_colons: <REDACTED-IPV6: 1>",
                             },
                             {
                                 "ColonsFields7",
                                 "v6_colons_7_fields: ::12:234:35:46:5:6:7",
                                 "v6_colons_7_fields: <REDACTED-IPV6: 1>",
                             },
                             {
                                 "V6Fields7Colons",
                                 "v6_7_fields_colons: 12:234:35:46:5:6:7::",
                                 "v6_7_fields_colons: <REDACTED-IPV6: 1>",
                             },
                             {
                                 "ColonsFields8",
                                 "v6_colons_8_fields: ::12:234:35:46:5:6:7:8",
                                 "v6_colons_8_fields: <REDACTED-IPV6: 1>:8",
                             },
                             {
                                 "V6Fields8Colons",
                                 "v6_8_fields_colons: 12:234:35:46:5:6:7:8::",
                                 "v6_8_fields_colons: <REDACTED-IPV6: 1>::",
                             },

                         })),
                         GetTestName<IpTestParam>);

TEST_P(IPv6ReplacerTest, ReplaceIPv6) {
  IpTestParam param = GetParam();

  RedactionIdCache cache(inspect::UintProperty{});
  Replacer replacer = ReplaceIPv6();
  ASSERT_NE(replacer, nullptr);
  EXPECT_EQ(replacer(cache, param.text), param.expected_output);
}

TEST(MacReplacerTest, GetOuiPrefix) {
  std::string mac_colons = "12:34:56:78:90:ff";
  EXPECT_EQ(mac_utils::GetOuiPrefix(mac_colons), "12:34:56:");
  std::string mac_dashes = "12-34-56-78-90-ff";
  EXPECT_EQ(mac_utils::GetOuiPrefix(mac_dashes), "12-34-56-");
  std::string mac_dots = "12.34.56.78.90.ff";
  EXPECT_EQ(mac_utils::GetOuiPrefix(mac_dots), "12.34.56.");
}

TEST(MacReplacerTest, CanonicalizeMac) {
  EXPECT_EQ(mac_utils::CanonicalizeMac("12:34:56:78:90:ff"), "12:34:56:78:90:ff");
  EXPECT_EQ(mac_utils::CanonicalizeMac("12:34:56:78:90:FF"), "12:34:56:78:90:ff");
  EXPECT_EQ(mac_utils::CanonicalizeMac("12-34-56-78-90-ff"), "12:34:56:78:90:ff");
  EXPECT_EQ(mac_utils::CanonicalizeMac("12.34.56.78.90.ff"), "12:34:56:78:90:ff");
}

TEST(MacReplacerTest, ReplaceMac) {
  RedactionIdCache cache(inspect::UintProperty{});
  Replacer replacer = ReplaceMac();
  ASSERT_NE(replacer, nullptr);

  std::string text = R"(
00:0a:95:9F:68:16
12-34-95-9F-68-16
d.e.a.d.be.ef
ff.f-ff:f-ff:f
)";
  EXPECT_EQ(replacer(cache, text),
            R"(
00:0a:95:<REDACTED-MAC: 1>
12-34-95-<REDACTED-MAC: 2>
d.e.a.<REDACTED-MAC: 3>
ff.f-ff:<REDACTED-MAC: 4>
)");
}

TEST(MacReplacerTest, ReplaceMacIgnoresDelimitersAndCaseForIds) {
  RedactionIdCache cache(inspect::UintProperty{});
  Replacer replacer = ReplaceMac();
  ASSERT_NE(replacer, nullptr);

  std::string text = R"(
12-3f-95-9f-68-6
12:3F:95:9F:68:06
12.3f.95.9F.68.06
)";
  EXPECT_EQ(replacer(cache, text),
            R"(
12-3f-95-<REDACTED-MAC: 1>
12:3F:95:<REDACTED-MAC: 1>
12.3f.95.<REDACTED-MAC: 1>
)");
}

TEST(SsidReplacerTest, ReplaceSsid) {
  RedactionIdCache cache(inspect::UintProperty{});
  Replacer replacer = ReplaceSsid();
  ASSERT_NE(replacer, nullptr);

  std::string text = R"(
<ssid->
<ssid-4567fedcba>
<ssid-0123456789012345678901234567890123456789012345678901234567890123>
<ssid-01234567890123456789012345678901234567890123456789012345678901234>
)";
  EXPECT_EQ(replacer(cache, text), R"(
<REDACTED-SSID: 1>
<REDACTED-SSID: 2>
<REDACTED-SSID: 3>
<REDACTED-SSID: 4>
)");
}

TEST(SensitiveReplacerTest, ReplaceSensitive) {
  RedactionIdCache cache(inspect::UintProperty{});
  Replacer replacer = ReplaceSensitive();
  ASSERT_NE(replacer, nullptr);

  std::string text = R"(
plain: normal text
SENSITIVE{12345}
addr: SENSITIVE{[1, 2, 3, 4]}
SENSITIVE{SensitiveStruct { field: "secret" }}
SENSITIVE{SensitiveStruct { field: "escaped \" { } \" string" }}
SENSITIVE{unquoted_trailing_backslash: foo \} after_backslash
SENSITIVE{windows_path: C:\path\to\file}
nested: SENSITIVE{Outer { inner: SENSITIVE{Inner { x: 1 }} }}
unclosed: SENSITIVE{something)";
  EXPECT_EQ(replacer(cache, text), R"(
plain: normal text
<REDACTED-SENSITIVE>
addr: <REDACTED-SENSITIVE>
<REDACTED-SENSITIVE>
<REDACTED-SENSITIVE>
<REDACTED-SENSITIVE> after_backslash
<REDACTED-SENSITIVE>
nested: <REDACTED-SENSITIVE>
unclosed: <REDACTED-SENSITIVE>)");
}

TEST(SensitiveReplacerTest, ReplaceSensitiveUnmatchedBraces) {
  RedactionIdCache cache(inspect::UintProperty{});
  Replacer replacer = ReplaceSensitive();
  ASSERT_NE(replacer, nullptr);

  // Unclosed sensitive block fails closed by redacting to the end of text.
  std::string unclosed = "error: SENSITIVE{RustError(something occurred";
  EXPECT_EQ(replacer(cache, unclosed), "error: <REDACTED-SENSITIVE>");

  // Unmatched curly braces within string literals do not close the block.
  std::string unmatched_in_str =
      "error: SENSITIVE{RustError(\"encountered unmatched { in expression\")}";
  EXPECT_EQ(replacer(cache, unmatched_in_str), "error: <REDACTED-SENSITIVE>");

  // Unmatched closing brace outside of a SENSITIVE block is preserved.
  std::string extra_closing = "struct: SENSITIVE{MyStruct { x: 1 }}}";
  EXPECT_EQ(replacer(cache, extra_closing), "struct: <REDACTED-SENSITIVE>}");
}

}  // namespace
}  // namespace forensics
