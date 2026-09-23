// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fit/function.h>

#include <sstream>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/utils.h"

namespace fidlc {
namespace {

void CompareIdToWords(std::string_view id, std::string_view expected_lowercase_words) {
  std::ostringstream actual;
  for (const auto& word : SplitIdentifierWords(id)) {
    if (actual.tellp() > 0) {
      actual << ' ';
    }
    actual << word;
  }
  ASSERT_EQ(expected_lowercase_words, actual.str()) << "Failed for " << id;
}

TEST(UtilsTests, IdToWords) {
  CompareIdToWords("agent_request_count", "agent request count");
  CompareIdToWords("common", "common");
  CompareIdToWords("Service", "service");
  CompareIdToWords("Blink32", "blink32");
  CompareIdToWords("the21jumpStreet", "the21jump street");
  CompareIdToWords("the21JumpStreet", "the21 jump street");
  CompareIdToWords("onOntologyUpdate", "on ontology update");
  CompareIdToWords("urlLoader", "url loader");
  CompareIdToWords("onUrlLoader", "on url loader");
  CompareIdToWords("OnOntologyUpdate", "on ontology update");
  CompareIdToWords("UrlLoader", "url loader");
  CompareIdToWords("OnUrlLoader", "on url loader");
  CompareIdToWords("kUrlLoader", "url loader");
  CompareIdToWords("kOnUrlLoader", "on url loader");
  CompareIdToWords("WhatIfSomeoneDoes_This", "what if someone does this");
  CompareIdToWords("SOME_CONST", "some const");
  CompareIdToWords("NAME_MIN_LEN", "name min len");
  CompareIdToWords("OnPress", "on press");
  CompareIdToWords("URLLoader", "url loader");
  CompareIdToWords("PPPOE", "pppoe");
  CompareIdToWords("PPP_O_E", "ppp o e");
  CompareIdToWords("PPP_o_E", "ppp o e");

  // Note the next two tests have expected results that may seem
  // counter-intuitive, but if IDs like "URLLoader" are expected to
  // translate to the words "url loader", then these translations
  // are consistent.
  CompareIdToWords("PppOE", "ppp oe");
  CompareIdToWords("PPPoE", "pp po e");
}

void CaseTest(bool valid_conversion, std::string_view case_name,
              fit::function<bool(std::string_view)> is_case,
              fit::function<std::string(std::string_view)> to_case, std::string_view original,
              std::string_view expected) {
  EXPECT_FALSE(is_case(original)) << original << " is " << case_name;
  std::string converted = to_case(original);
  EXPECT_EQ(converted, expected) << "from '" << original << "' to '" << converted << "' != '"
                                 << expected << "'";
  if (valid_conversion) {
    EXPECT_TRUE(is_case(expected))
        << "from '" << original << "' expected '" << expected << "' is not " << case_name;
    EXPECT_TRUE(is_case(converted))
        << "from '" << original << "' to '" << converted << "' is not " << case_name;
  } else {
    EXPECT_FALSE(is_case(converted)) << "from '" << original << "' to '" << converted
                                     << "' was not expected to match " << case_name << ", but did!";
  }
}

#define ASSERT_CASE(CASE, FROM, TO) \
  CaseTest(/*valid_conversion=*/true, #CASE, Is##CASE##Case, To##CASE##Case, FROM, TO)

#define ASSERT_BAD_CASE(CASE, FROM, TO) \
  CaseTest(/*valid_conversion=*/false, #CASE, Is##CASE##Case, To##CASE##Case, FROM, TO)

TEST(UtilsTests, UpperCamelCase) {
  ASSERT_CASE(UpperCamel, "x", "X");
  ASSERT_CASE(UpperCamel, "xy", "Xy");
  ASSERT_BAD_CASE(UpperCamel, "x_y", "XY");
  ASSERT_CASE(UpperCamel, "xyz_123", "Xyz123");
  ASSERT_CASE(UpperCamel, "xy_z_123", "XyZ123");
  ASSERT_CASE(UpperCamel, "xy_z123", "XyZ123");
  ASSERT_CASE(UpperCamel, "days_in_a_week", "DaysInAWeek");
  ASSERT_CASE(UpperCamel, "android8_0_0", "Android8_0_0");
  ASSERT_CASE(UpperCamel, "android_8_0_0", "Android8_0_0");
  ASSERT_CASE(UpperCamel, "x_marks_the_spot", "XMarksTheSpot");
  ASSERT_CASE(UpperCamel, "RealID", "RealId");
  ASSERT_CASE(UpperCamel, "real_id", "RealId");
  ASSERT_BAD_CASE(UpperCamel, "real_i_d", "RealID");
  ASSERT_CASE(UpperCamel, "real3d", "Real3d");
  ASSERT_CASE(UpperCamel, "real3_d", "Real3D");
  ASSERT_CASE(UpperCamel, "real_3d", "Real3d");
  ASSERT_CASE(UpperCamel, "real_3_d", "Real3D");
  ASSERT_CASE(UpperCamel, "hello_e_world", "HelloEWorld");
  ASSERT_CASE(UpperCamel, "hello_eworld", "HelloEworld");
  ASSERT_CASE(UpperCamel, "URLLoader", "UrlLoader");
  ASSERT_CASE(UpperCamel, "is_21Jump_street", "Is21JumpStreet");
  ASSERT_CASE(UpperCamel, "URLloader", "UrLloader");
  ASSERT_CASE(UpperCamel, "URLLoader", "UrlLoader");
  ASSERT_CASE(UpperCamel, "url_loader", "UrlLoader");
  ASSERT_CASE(UpperCamel, "URL_LOADER", "UrlLoader");
  ASSERT_CASE(UpperCamel, "urlLoader", "UrlLoader");
  ASSERT_CASE(UpperCamel, "kUrlLoader", "UrlLoader");
  ASSERT_CASE(UpperCamel, "kURLLoader", "UrlLoader");
}

TEST(UtilsTests, LowerCamelCase) {
  ASSERT_CASE(LowerCamel, "X", "x");
  ASSERT_CASE(LowerCamel, "XY", "xy");
  ASSERT_CASE(LowerCamel, "X_Y", "xY");
  ASSERT_CASE(LowerCamel, "XYZ_123", "xyz123");
  ASSERT_CASE(LowerCamel, "XY_Z_123", "xyZ123");
  ASSERT_CASE(LowerCamel, "XY_Z123", "xyZ123");
  ASSERT_CASE(LowerCamel, "DAYS_IN_A_WEEK", "daysInAWeek");
  ASSERT_CASE(LowerCamel, "ANDROID8_0_0", "android8_0_0");
  ASSERT_CASE(LowerCamel, "ANDROID_8_0_0", "android8_0_0");
  ASSERT_CASE(LowerCamel, "X_MARKS_THE_SPOT", "xMarksTheSpot");
  ASSERT_CASE(LowerCamel, "realID", "realId");
  ASSERT_CASE(LowerCamel, "REAL_ID", "realId");
  ASSERT_BAD_CASE(LowerCamel, "REAL_I_D", "realID");
  ASSERT_CASE(LowerCamel, "REAL3D", "real3D");
  ASSERT_CASE(LowerCamel, "REAL3_D", "real3D");
  ASSERT_CASE(LowerCamel, "REAL_3D", "real3D");
  ASSERT_CASE(LowerCamel, "REAL_3_D", "real3D");
  ASSERT_CASE(LowerCamel, "HELLO_E_WORLD", "helloEWorld");
  ASSERT_CASE(LowerCamel, "HELLO_EWORLD", "helloEworld");
  ASSERT_CASE(LowerCamel, "URLLoader", "urlLoader");
  ASSERT_CASE(LowerCamel, "is_21Jump_street", "is21JumpStreet");
  ASSERT_CASE(LowerCamel, "URLloader", "urLloader");
  ASSERT_CASE(LowerCamel, "UrlLoader", "urlLoader");
  ASSERT_CASE(LowerCamel, "URLLoader", "urlLoader");
  ASSERT_CASE(LowerCamel, "url_loader", "urlLoader");
  ASSERT_CASE(LowerCamel, "URL_LOADER", "urlLoader");
  ASSERT_CASE(LowerCamel, "kUrlLoader", "urlLoader");
  ASSERT_CASE(LowerCamel, "kURLLoader", "urlLoader");
}

TEST(UtilsTests, UpperSnakeCase) {
  ASSERT_CASE(UpperSnake, "x", "X");
  ASSERT_CASE(UpperSnake, "xy", "XY");
  ASSERT_CASE(UpperSnake, "xY", "X_Y");
  ASSERT_CASE(UpperSnake, "xyz123", "XYZ123");
  ASSERT_CASE(UpperSnake, "xyz_123", "XYZ_123");
  ASSERT_CASE(UpperSnake, "xyZ123", "XY_Z123");
  ASSERT_CASE(UpperSnake, "daysInAWeek", "DAYS_IN_A_WEEK");
  ASSERT_CASE(UpperSnake, "android8_0_0", "ANDROID8_0_0");
  ASSERT_CASE(UpperSnake, "android_8_0_0", "ANDROID_8_0_0");
  ASSERT_CASE(UpperSnake, "xMarksTheSpot", "X_MARKS_THE_SPOT");
  ASSERT_CASE(UpperSnake, "realId", "REAL_ID");
  ASSERT_CASE(UpperSnake, "realID", "REAL_ID");
  ASSERT_CASE(UpperSnake, "real3d", "REAL3D");
  ASSERT_CASE(UpperSnake, "real3D", "REAL3_D");
  ASSERT_CASE(UpperSnake, "real_3d", "REAL_3D");
  ASSERT_CASE(UpperSnake, "real_3D", "REAL_3_D");
  ASSERT_CASE(UpperSnake, "helloEWorld", "HELLO_E_WORLD");
  ASSERT_CASE(UpperSnake, "helloEworld", "HELLO_EWORLD");
  ASSERT_CASE(UpperSnake, "URLLoader", "URL_LOADER");
  ASSERT_CASE(UpperSnake, "is_21Jump_street", "IS_21_JUMP_STREET");
  ASSERT_CASE(UpperSnake, "URLloader", "UR_LLOADER");
  ASSERT_CASE(UpperSnake, "UrlLoader", "URL_LOADER");
  ASSERT_CASE(UpperSnake, "URLLoader", "URL_LOADER");
  ASSERT_CASE(UpperSnake, "url_loader", "URL_LOADER");
  ASSERT_CASE(UpperSnake, "urlLoader", "URL_LOADER");
  ASSERT_CASE(UpperSnake, "kUrlLoader", "URL_LOADER");
  ASSERT_CASE(UpperSnake, "kURLLoader", "URL_LOADER");
}

TEST(UtilsTests, LowerSnakeCase) {
  ASSERT_CASE(LowerSnake, "X", "x");
  ASSERT_CASE(LowerSnake, "Xy", "xy");
  ASSERT_CASE(LowerSnake, "XY", "xy");
  ASSERT_CASE(LowerSnake, "Xyz123", "xyz123");
  ASSERT_CASE(LowerSnake, "Xyz_123", "xyz_123");
  ASSERT_CASE(LowerSnake, "XyZ123", "xy_z123");
  ASSERT_CASE(LowerSnake, "DaysInAWeek", "days_in_a_week");
  ASSERT_CASE(LowerSnake, "Android8_0_0", "android8_0_0");
  ASSERT_CASE(LowerSnake, "Android_8_0_0", "android_8_0_0");
  ASSERT_CASE(LowerSnake, "XMarksTheSpot", "x_marks_the_spot");
  ASSERT_CASE(LowerSnake, "RealId", "real_id");
  ASSERT_CASE(LowerSnake, "RealID", "real_id");
  ASSERT_CASE(LowerSnake, "Real3d", "real3d");
  ASSERT_CASE(LowerSnake, "Real3D", "real3_d");
  ASSERT_CASE(LowerSnake, "Real_3d", "real_3d");
  ASSERT_CASE(LowerSnake, "Real_3D", "real_3_d");
  ASSERT_CASE(LowerSnake, "HelloEWorld", "hello_e_world");
  ASSERT_CASE(LowerSnake, "HelloEworld", "hello_eworld");
  ASSERT_CASE(LowerSnake, "URLLoader", "url_loader");
  ASSERT_CASE(LowerSnake, "is_21Jump_street", "is_21_jump_street");
  ASSERT_CASE(LowerSnake, "URLloader", "ur_lloader");
  ASSERT_CASE(LowerSnake, "UrlLoader", "url_loader");
  ASSERT_CASE(LowerSnake, "URLLoader", "url_loader");
  ASSERT_CASE(LowerSnake, "URL_LOADER", "url_loader");
  ASSERT_CASE(LowerSnake, "urlLoader", "url_loader");
  ASSERT_CASE(LowerSnake, "kUrlLoader", "url_loader");
  ASSERT_CASE(LowerSnake, "kURLLoader", "url_loader");
}

TEST(UtilsTests, IsValidLibraryComponent) {
  ASSERT_TRUE(IsValidLibraryComponent("a"));
  ASSERT_TRUE(IsValidLibraryComponent("abc"));
  ASSERT_TRUE(IsValidLibraryComponent("a2b"));

  ASSERT_FALSE(IsValidLibraryComponent(""));
  ASSERT_FALSE(IsValidLibraryComponent("A"));
  ASSERT_FALSE(IsValidLibraryComponent("2"));
  ASSERT_FALSE(IsValidLibraryComponent("a_c"));
  ASSERT_FALSE(IsValidLibraryComponent("ab_"));
}

TEST(UtilsTests, IsValidIdentifierComponent) {
  ASSERT_TRUE(IsValidIdentifierComponent("a"));
  ASSERT_TRUE(IsValidIdentifierComponent("abc"));
  ASSERT_TRUE(IsValidIdentifierComponent("A"));
  ASSERT_TRUE(IsValidIdentifierComponent("a2b"));
  ASSERT_TRUE(IsValidIdentifierComponent("a_c"));

  ASSERT_FALSE(IsValidIdentifierComponent(""));
  ASSERT_FALSE(IsValidIdentifierComponent("2"));
  ASSERT_FALSE(IsValidIdentifierComponent("ab_"));
}

TEST(UtilsTests, IsValidFullyQualifiedMethodIdentifier) {
  ASSERT_TRUE(IsValidFullyQualifiedMethodIdentifier("lib/Protocol.Method"));
  ASSERT_TRUE(IsValidFullyQualifiedMethodIdentifier("long.lib/Protocol.Method"));

  ASSERT_FALSE(IsValidFullyQualifiedMethodIdentifier("Method"));
  ASSERT_FALSE(IsValidFullyQualifiedMethodIdentifier("lib/Protocol"));
  ASSERT_FALSE(IsValidFullyQualifiedMethodIdentifier("lonG.lib/Protocol.Method"));
  ASSERT_FALSE(IsValidFullyQualifiedMethodIdentifier("long.liB/Protocol.Method"));
}

TEST(UtilsTests, RemoveWhitespace) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
/// C1a
/// C1b
library foo.bar;  // C2

/// C3a
/// C3b
using baz.qux;  // C4

/// C5a
/// C5b
resource_definition thing : uint8 {  // C6
    properties {  // C8
/// C9a
/// C9b
        stuff rights;  // C10
    };
};

/// C11a
/// C11b
const MY_CONST string = "abc";  // C12

/// C13a
/// C13b
type MyEnum = enum {  // C14
/// C15a
/// C17b
    MY_VALUE = 1;  // C16
};

/// C17a
/// C17b
type MyTable = resource table {  // C18
/// C19a
/// C19b
    1: field thing;  // C20
};

/// C21a
/// C21b
alias MyAlias = MyStruct;  // C22

/// C23a
/// C23b
protocol MyProtocol {  // C24
/// C25a
/// C25b
    MyMethod(resource struct {  // C26
/// C27a
/// C27b
        data MyTable;  // C28
    }) -> () error MyEnum;  // C29
};  // 30

/// C29a
/// C29b
service MyService {  // C32
/// C31a
/// C31b
    my_protocol client_end:MyProtocol;  // C34
};  // C35
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
/// C1a
/// C1b
library foo.bar; // C2

/// C3a
/// C3b
using baz.qux; // C4

/// C5a
/// C5b
resource_definition thing : uint8 { // C6
    properties { // C8
        /// C9a
        /// C9b
        stuff rights; // C10
    };
};

/// C11a
/// C11b
const MY_CONST string = "abc"; // C12

/// C13a
/// C13b
type MyEnum = enum { // C14
    /// C15a
    /// C17b
    MY_VALUE = 1; // C16
};

/// C17a
/// C17b
type MyTable = resource table { // C18
    /// C19a
    /// C19b
    1: field thing; // C20
};

/// C21a
/// C21b
alias MyAlias = MyStruct; // C22

/// C23a
/// C23b
protocol MyProtocol { // C24
    /// C25a
    /// C25b
    MyMethod(resource struct { // C26
        /// C27a
        /// C27b
        data MyTable; // C28
    }) -> () error MyEnum; // C29
}; // 30

/// C29a
/// C29b
service MyService { // C32
    /// C31a
    /// C31b
    my_protocol client_end:MyProtocol; // C34
}; // C35
)FIDL";

  ASSERT_EQ(RemoveWhitespace(unformatted), RemoveWhitespace(formatted));
}

TEST(UtilsTests, Canonicalize) {
  EXPECT_EQ(Canonicalize(""), "");

  // Basic letter combinations.
  EXPECT_EQ(Canonicalize("a"), "a");
  EXPECT_EQ(Canonicalize("A"), "a");
  EXPECT_EQ(Canonicalize("ab"), "ab");
  EXPECT_EQ(Canonicalize("AB"), "ab");
  EXPECT_EQ(Canonicalize("Ab"), "ab");
  EXPECT_EQ(Canonicalize("aB"), "a_b");
  EXPECT_EQ(Canonicalize("a_b"), "a_b");
  EXPECT_EQ(Canonicalize("A_B"), "a_b");
  EXPECT_EQ(Canonicalize("A_b"), "a_b");
  EXPECT_EQ(Canonicalize("a_B"), "a_b");

  // Digits are treated like lowercase letters.
  EXPECT_EQ(Canonicalize("1"), "1");
  EXPECT_EQ(Canonicalize("a1"), "a1");
  EXPECT_EQ(Canonicalize("A1"), "a1");

  // Leading digits are illegal in FIDL identifiers, so these do not matter.
  EXPECT_EQ(Canonicalize("1a"), "1a");
  EXPECT_EQ(Canonicalize("1A"), "1_a");
  EXPECT_EQ(Canonicalize("12"), "12");

  // Lower/upper snake/camel case conventions.
  EXPECT_EQ(Canonicalize("lowerCamelCase"), "lower_camel_case");
  EXPECT_EQ(Canonicalize("UpperCamelCase"), "upper_camel_case");
  EXPECT_EQ(Canonicalize("lower_snake_case"), "lower_snake_case");
  EXPECT_EQ(Canonicalize("UpperSnake_CASE"), "upper_snake_case");
  EXPECT_EQ(Canonicalize("Camel_With_Underscores"), "camel_with_underscores");
  EXPECT_EQ(Canonicalize("camelWithAOneLetterWord"), "camel_with_a_one_letter_word");
  EXPECT_EQ(Canonicalize("1_2__3___underscores"), "1_2_3_underscores");

  // Acronym casing.
  EXPECT_EQ(Canonicalize("HTTPServer"), "http_server");
  EXPECT_EQ(Canonicalize("HttpServer"), "http_server");
  EXPECT_EQ(Canonicalize("URLIsATLA"), "url_is_atla");
  EXPECT_EQ(Canonicalize("UrlIsATla"), "url_is_a_tla");

  // Words with digits: H264 encoder.
  EXPECT_EQ(Canonicalize("h264encoder"), "h264encoder");
  EXPECT_EQ(Canonicalize("H264ENCODER"), "h264_encoder");
  EXPECT_EQ(Canonicalize("h264_encoder"), "h264_encoder");
  EXPECT_EQ(Canonicalize("H264_ENCODER"), "h264_encoder");
  EXPECT_EQ(Canonicalize("h264Encoder"), "h264_encoder");
  EXPECT_EQ(Canonicalize("H264Encoder"), "h264_encoder");

  // Words with digits: DDR4 memory.
  EXPECT_EQ(Canonicalize("ddr4memory"), "ddr4memory");
  EXPECT_EQ(Canonicalize("DDR4MEMORY"), "ddr4_memory");
  EXPECT_EQ(Canonicalize("ddr4_memory"), "ddr4_memory");
  EXPECT_EQ(Canonicalize("DDR4_MEMORY"), "ddr4_memory");
  EXPECT_EQ(Canonicalize("ddr4Memory"), "ddr4_memory");
  EXPECT_EQ(Canonicalize("Ddr4Memory"), "ddr4_memory");
  EXPECT_EQ(Canonicalize("DDR4Memory"), "ddr4_memory");

  // Words with digits: A2DP profile.
  EXPECT_EQ(Canonicalize("a2dpprofile"), "a2dpprofile");
  EXPECT_EQ(Canonicalize("A2DPPROFILE"), "a2_dpprofile");
  EXPECT_EQ(Canonicalize("a2dp_profile"), "a2dp_profile");
  EXPECT_EQ(Canonicalize("A2DP_PROFILE"), "a2_dp_profile");
  EXPECT_EQ(Canonicalize("a2dpProfile"), "a2dp_profile");
  EXPECT_EQ(Canonicalize("A2dpProfile"), "a2dp_profile");
  EXPECT_EQ(Canonicalize("A2DPProfile"), "a2_dp_profile");

  // Words with digits: R2D2 is one word.
  EXPECT_EQ(Canonicalize("r2d2isoneword"), "r2d2isoneword");
  EXPECT_EQ(Canonicalize("R2D2ISONEWORD"), "r2_d2_isoneword");
  EXPECT_EQ(Canonicalize("r2d2_is_one_word"), "r2d2_is_one_word");
  EXPECT_EQ(Canonicalize("R2D2_IS_ONE_WORD"), "r2_d2_is_one_word");
  EXPECT_EQ(Canonicalize("r2d2IsOneWord"), "r2d2_is_one_word");
  EXPECT_EQ(Canonicalize("R2d2IsOneWord"), "r2d2_is_one_word");
  EXPECT_EQ(Canonicalize("R2D2IsOneWord"), "r2_d2_is_one_word");

  // Leading and trailing underscores are illegal in FIDL identifiers, so these
  // do not matter.
  EXPECT_EQ(Canonicalize("_"), "");
  EXPECT_EQ(Canonicalize("_a"), "a");
  EXPECT_EQ(Canonicalize("a_"), "a_");
  EXPECT_EQ(Canonicalize("_a_"), "a_");
  EXPECT_EQ(Canonicalize("__a__"), "a_");
}

TEST(UtilsTests, StripStringLiteralQuotes) {
  EXPECT_EQ(StripStringLiteralQuotes("\"\""), "");
  EXPECT_EQ(StripStringLiteralQuotes("\"foobar\""), "foobar");
}

TEST(UtilsTests, StripDocCommentSlashes) {
  EXPECT_EQ(StripDocCommentSlashes(R"FIDL(
  /// A
  /// multiline
  /// comment!
)FIDL"),
            "\n A\n multiline\n comment!\n");

  EXPECT_EQ(StripDocCommentSlashes(R"FIDL(
  ///
  /// With
  ///
  /// empty
  ///
  /// lines
  ///
)FIDL"),
            "\n\n With\n\n empty\n\n lines\n\n");

  EXPECT_EQ(StripDocCommentSlashes(R"FIDL(
  /// With

  /// blank


  /// lines
)FIDL"),
            "\n With\n\n blank\n\n\n lines\n");

  EXPECT_EQ(StripDocCommentSlashes(R"FIDL(
	/// With
		/// tabs
	 /// in
 	/// addition
 	 /// to
	 	/// spaces
)FIDL"),
            "\n With\n tabs\n in\n addition\n to\n spaces\n");

  EXPECT_EQ(StripDocCommentSlashes(R"FIDL(
  /// Weird
/// Offsets
  /// Slash///
  ///Placement ///
       /// And
  ///   Spacing   )FIDL"),
            "\n Weird\n Offsets\n Slash///\nPlacement ///\n And\n   Spacing   \n");
}

TEST(UtilsTests, DecodeUnicodeHex) {
  EXPECT_EQ(DecodeUnicodeHex("0"), 0x0u);
  EXPECT_EQ(DecodeUnicodeHex("a"), 0xau);
  EXPECT_EQ(DecodeUnicodeHex("12"), 0x12u);
  EXPECT_EQ(DecodeUnicodeHex("123abc"), 0x123abcu);
  EXPECT_EQ(DecodeUnicodeHex("ffffff"), 0xffffffu);
}

TEST(UtilsTests, StringLiteralLength) {
  EXPECT_EQ(StringLiteralLength(R"("Hello")"), 5u);
  EXPECT_EQ(StringLiteralLength(R"("\\")"), 1u);
  EXPECT_EQ(StringLiteralLength(R"("\to")"), 2u);
  EXPECT_EQ(StringLiteralLength(R"("\n")"), 1u);
  EXPECT_EQ(StringLiteralLength(R"("\u{01F600}")"), 4u);
  EXPECT_EQ(StringLiteralLength(R"("\u{2713}")"), 3u);
  EXPECT_EQ(StringLiteralLength(R"("")"), 0u);
  EXPECT_EQ(StringLiteralLength(R"("$")"), 1u);
  EXPECT_EQ(StringLiteralLength(R"("¢")"), 2u);
  EXPECT_EQ(StringLiteralLength(R"("€")"), 3u);
  EXPECT_EQ(StringLiteralLength(R"("𐍈")"), 4u);
  EXPECT_EQ(StringLiteralLength(R"("😁")"), 4u);
}

}  // namespace
}  // namespace fidlc
