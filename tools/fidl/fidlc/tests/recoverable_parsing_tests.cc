// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(RecoverableParsingTests, BadUnexpectedToken) {
  TestLibrary library;
  library.AddFile("bad/fi-0007.noformat.test.fidl");
  library.ExpectFail(ErrUnexpectedToken);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverAtEndOfFile) {
  TestLibrary library(R"FIDL(
library example;

type Enum = enum {
    ONE;          // First error
};

type Bits = bits {
    CONSTANT = ;  // Second error
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kSemicolon),
                     Token::KindAndSubkind(Token::Kind::kEqual));
  library.ExpectFail(ErrUnexpectedToken);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverAtEndOfDecl) {
  TestLibrary library(R"FIDL(
library example;

type Enum = enum {
    VARIANT = 0;
    MISSING_EQUALS 5;
};

type Union = union {
    1: string_value string;
    2 missing_colon uint16;
};

type Struct = struct {
    value string;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kNumericLiteral),
                     Token::KindAndSubkind(Token::Kind::kEqual));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kColon));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverAtEndOfMember) {
  TestLibrary library(R"FIDL(
library example;

type SettingType = enum {
    UNKNOWN = 0;
    TIME_ZONE = 1;
    CONNECTIVITY 2;                    // Error: missing equals
};

type SettingData = union {
    1: string_value string;
    2 time_zone_value ConnectedState;  // Error: missing colon
    /// Unattached doc comment.        // erroneous doc comment is skipped during recovery
};

type LoginOverride = {                 // Error: missing keyword
    NONE = 0;
    AUTH.PROVIDER = 2,                 // Error: '.' in identifier
};

type AccountSettings = table {
    1: mo.de LoginOverride;            // Error: '.' in identifier
    3: setting OtherSetting;
};

type TimeZoneInfo = struct {
    current TimeZone:optional;
    available vector<<TimeZone>;       // Error: extra <
};

type TimeZone = struct {
    id string;
    name string;
    region vector<string>;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kNumericLiteral),
                     Token::KindAndSubkind(Token::Kind::kEqual));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kColon));
  library.ExpectFail(ErrMissingOrdinalBeforeMember);
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kLeftCurly),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kDot),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kLeftAngle),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadDoNotCompileAfterParsingFails) {
  TestLibrary library(R"FIDL(
library example;

const compound.identifier uint8 = 0;  // Syntax error

type NameCollision = struct {};
type NameCollision = struct {};       // This name collision error will not be
                                      // reported, because if parsing fails
                                      // compilation is skipped
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kDot),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverToNextBitsMember) {
  TestLibrary library(R"FIDL(
library example;

type Bits = bits {
    ONE 0x1;      // First error
    TWO = 0x2;
    FOUR = 0x4    // Second error
    EIGHT = 0x8;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kNumericLiteral),
                     Token::KindAndSubkind(Token::Kind::kEqual));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverToNextEnumMember) {
  TestLibrary library(R"FIDL(
library example;

type Enum = enum {
    ONE 1;      // First error
    TWO = 2;
    THREE = 3   // Second error
    FOUR = 4;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kNumericLiteral),
                     Token::KindAndSubkind(Token::Kind::kEqual));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverToNextProtocolMember) {
  TestLibrary library(R"FIDL(
library example;

protocol P {
    compose A B;                                 // 2 Errors (on 'B', ';')
    MethodWithoutSemicolon()
    ValidMethod();                               // Error (expecting ';')
    -> Event(struct { TypeWithoutParamName; });  // Error
    MissingParen server_end:Protocol protocol);  // Error
    -> Event(struct { missing_paren T };         // 2 Errors (on '}', ';')
    ValidMethod();
    Method() -> (struct { num uint16; }) error;  // Error
};
)FIDL");
  // NOTE(https://fxbug.dev/42152439): the difference in errors is due to the change in
  // test input (for the TypeWithoutParams and MissingParen cases) rather than
  // any real behavior change
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrInvalidProtocolMember);
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kSemicolon),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrInvalidProtocolMember);
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kRightCurly),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kSemicolon),
                     Token::KindAndSubkind(Token::Kind::kRightParen));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kSemicolon),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverableParamListParsing) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
  Method(/// Doc comment
      struct { b bool; }) -> (/// Doc comment
      struct { b bool; });
};
)FIDL");

  library.ExpectFail(ErrDocCommentOnParameters);
  library.ExpectFail(ErrDocCommentOnParameters);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverableUnmatchedDelimiterInParamList) {
  TestLibrary library(R"FIDL(
library example;

protocol Example {
  Method() -> (vector<);
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kRightParen),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrUnexpectedToken);
  library.ExpectFail(ErrUnexpectedToken);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverToNextServiceMember) {
  TestLibrary library(R"FIDL(
library example;

protocol P {};
protocol Q {};
protocol R {};

service Service {
  p P extra_token; // First error
  q Q              // Second error
  r R;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kSemicolon),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverToNextStructMember) {
  TestLibrary library(R"FIDL(
library example;

type Struct = struct {
    string_value string extra_token; // Error
    uint_value uint8;
    vector_value vector<handle>      // Error
    int_value int32;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kSemicolon),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverToNextTableMember) {
  TestLibrary library(R"FIDL(
library example;

type Table = table {
    1: string_value string              // Error
    2: uint_value uint8;
    3: value_with space vector<handle>; // Error
    4: int_value int32;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kNumericLiteral),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Subkind::kVector),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  // NOTE(https://fxbug.dev/42152439): the difference here is just due to the type/member
  // reordering, not a behavior change
  library.ExpectFail(ErrMissingOrdinalBeforeMember);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverToNextUnionMember) {
  TestLibrary library(R"FIDL(
library example;

type Union = union {
    1 missing_colon string;     // First error
    3: uint_value uint8;
    4: missing_semicolon string // Second error
    5: int_value int16;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kColon));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kNumericLiteral),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverFinalMemberMissingSemicolon) {
  TestLibrary library(R"FIDL(
library example;

type Struct = struct {
    uint_value uint8;
    foo string // First error
};

// Recovered back to top-level parsing.
type Good = struct {};

extra_token // Second error
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kRightCurly),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrExpectedDeclaration, "extra_token");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, BadRecoverFinalMemberMissingNameAndSemicolon) {
  TestLibrary library(R"FIDL(
library example;

type Struct = struct {
    uint_value uint8;
    string_value }; // First error

// Does not recover back to top-level parsing. End the struct.
};

// Back to top-level parsing.
type Good = struct {};

extra_token // Second error
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kRightCurly),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrExpectedDeclaration, "extra_token");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// This test ensures that recoverable parsing works as intended for constraints,
// and returns useful and actionable information back to users.
TEST(RecoverableParsingTests, BadConstraintsRecoverability) {
  TestLibrary library(R"FIDL(
library example;
type TypeDecl = struct {
    // errors[0]: no constraints specified
    f0 vector<uint16>:;
    // errors[1]: no constraints specified
    f1 vector<uint16>:<>;
    // errors[2]: leading comma
    f2 vector<uint16>:<,16,optional>;
    // errors[3]: trailing comma
    f3 vector<uint16>:<16,optional,>;
    // errors[4]: double comma
    f4 vector<uint16>:<16,,optional>;
    // errors[5]: missing comma, errors[6], errors[7]: consume > and ; trying
    // to get to next member
    f5 vector<uint16>:<16 optional>;
    // errors[8] missing close bracket
    f7 vector<uint16>:<16;
    // errors[10]: invalid constant
    f8 vector<uint16>:1~6,optional;
    // errors[11]: unexpected token
    f9 vector<uint16>:,16,,optional,;
};
)FIDL");

  library.ExpectFail(ErrUnexpectedToken);
  library.ExpectFail(ErrUnexpectedToken);
  library.ExpectFail(ErrUnexpectedToken);
  library.ExpectFail(ErrUnexpectedToken);
  library.ExpectFail(ErrUnexpectedToken);
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kRightAngle));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kIdentifier),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kRightAngle),
                     Token::KindAndSubkind(Token::Kind::kIdentifier));
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kSemicolon),
                     Token::KindAndSubkind(Token::Kind::kRightAngle));
  library.ExpectFail(ErrInvalidCharacter, "~");
  library.ExpectFail(ErrUnexpectedToken);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, UnexpectedLineBreakInLiteral) {
  TestLibrary library;
  library.AddFile("bad/fi-0002.noformat.test.fidl");
  library.ExpectFail(ErrUnexpectedLineBreak);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, UnexpectedControlCharacter) {
  TestLibrary library;
  library.AddFile("bad/fi-0184.noformat.test.fidl");
  library.ExpectFail(ErrUnexpectedControlCharacter, "9");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, InvalidEscapeSequenceInLiteral) {
  TestLibrary library;
  library.AddFile("bad/fi-0003.noformat.test.fidl");
  // TODO(https://fxbug.dev/42063301): fidlc should recover from all three failures
  library.ExpectFail(ErrInvalidEscapeSequence, "\\ ");
  library.ExpectFail(ErrInvalidEscapeSequence, "\\i");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, InvalidHexDigit) {
  TestLibrary library;
  library.AddFile("bad/fi-0004.noformat.test.fidl");
  library.ExpectFail(ErrInvalidHexDigit, 'G');
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableParsingTests, UnicodeEscapeMissingBraces) {
  TestLibrary library;
  library.AddFile("bad/fi-0185.noformat.test.fidl");
  library.ExpectFail(ErrUnicodeEscapeMissingBraces);
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.data(), "\\u");
}

TEST(RecoverableParsingTests, UnicodeEscapeUnterminated) {
  TestLibrary library;
  library.AddFile("bad/fi-0186.noformat.test.fidl");
  library.ExpectFail(ErrUnicodeEscapeUnterminated);
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.data(), "\\u{1F600");
}

TEST(RecoverableParsingTests, UnicodeEscapeEmpty) {
  TestLibrary library;
  library.AddFile("bad/fi-0187.noformat.test.fidl");
  library.ExpectFail(ErrUnicodeEscapeEmpty);
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.data(), "\\u{}");
}

TEST(RecoverableParsingTests, UnicodeEscapeTooLong) {
  TestLibrary library;
  library.AddFile("bad/fi-0188.noformat.test.fidl");
  library.ExpectFail(ErrUnicodeEscapeTooLong);
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.data(), "001F600");
}

TEST(RecoverableParsingTests, UnicodeEscapeTooLarge) {
  TestLibrary library;
  library.AddFile("bad/fi-0189.noformat.test.fidl");
  library.ExpectFail(ErrUnicodeEscapeTooLarge, "110000");
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.data(), "110000");
}

TEST(RecoverableParsingTests, ExpectedDeclaration) {
  TestLibrary library;
  library.AddFile("bad/fi-0006.noformat.test.fidl");
  library.ExpectFail(ErrExpectedDeclaration, "cosnt");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
