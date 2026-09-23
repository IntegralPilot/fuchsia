// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.
#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/experimental_flags.h"
#include "tools/fidl/fidlc/src/formatter.h"
#include "tools/fidl/fidlc/src/utils.h"

#define ASSERT_FORMATTED(source, expected)                       \
  std::string actual = Format(source);                           \
  ASSERT_EQ(RemoveWhitespace(source), RemoveWhitespace(actual)); \
  ASSERT_EQ(expected, actual);

namespace fidlc {
namespace {

std::string Format(const std::string& source, bool reformat_and_compare = true) {
  SourceFile source_file("example.fidl", source);

  // We use a column width of 40, rather than the "real world" 100, to make tests easier to read
  // and write.
  Reporter reporter;
  auto formatter = Formatter(40, &reporter);
  ExperimentalFlagSet experimental_flags;
  auto result = formatter.Format(source_file, experimental_flags);

  // If we're still going to reformat, then this is the first pass.  Otherwise, we're on the second
  // pass.
  if (!result.has_value()) {
    return reformat_and_compare ? "PARSE_FAILED" : "SECOND_PASS_PARSE_FAILED";
  }

  // Running the newly formatted output through the formatted another time tests that well-formatted
  // inputs are always left unchanged by the formatter.
  if (reformat_and_compare) {
    return Format(result.value(), false);
  }

  if (source != result.value()) {
    return "FORMAT_PASSES_NOT_EQUAL";
  }
  return "\n" + result.value();
}

// Ensure that the formatter does not attempt to format unparsable FIDL.
TEST(FormatterTests, BadErrorOnInvalidInput) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyStruct = struct {
  vector<bool> my_member;
};
)FIDL";

  ASSERT_EQ("PARSE_FAILED", Format(unformatted));
}

// Ensure that an already properly formatted alias declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, AliasAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

alias MyAlias_Abcdefghijklmnopqr = bool;
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

// Test that an alias declaration gets wrapped properly.
TEST(FormatterTests, AliasOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

alias MyAlias_Abcdefghijklmnopqrs = bool;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

alias MyAlias_Abcdefghijklmnopqrs
        = bool;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Test with comments, doc comments, and attributes added and spaced out.
TEST(FormatterTests, AliasWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

 // comment

  /// doc comment

   @attr

    alias MyAlias_Abcdefghijklmnopqr = bool;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment

/// doc comment
@attr
alias MyAlias_Abcdefghijklmnopqr = bool;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to AliasFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, AliasMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(library foo.bar;alias MyAlias_Abcdefghijklmnopqr=bool;)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
alias MyAlias_Abcdefghijklmnopqr = bool;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to AliasFormatted, except that every token is on a newline.
TEST(FormatterTests, AliasMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

alias
MyAlias_Abcdefghijklmnopqr
=
bool
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

alias MyAlias_Abcdefghijklmnopqr = bool;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// TODO(https://fxbug.dev/42158340): more tests need to be added here once multiple arguments are
// supported for
//  attributes.

// Ensure that already properly formatted attributes declarations are not modified by another run
// through the formatter.
TEST(FormatterTests, AttributesAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
@attr_without_args
@attr_with_one_arg("abcdefghijklmnopqr")
@attr_with_two_args(a=true, b="abc")
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, AttributesSingle) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
   @attr_with_one_arg("abcd")
library foo.bar;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
@attr_with_one_arg("abcd")
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Attributes with arguments should overflow gracefully, while attributes without them should not.
TEST(FormatterTests, AttributesOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
@attr_without_args_abcdefghijklmnopqrstuv
@attr_with_one_arg("abcdefghijklmnopqrs")
@attr_with_two_args(a=true, b="abcd", c="wxyz")
library foo.bar;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
@attr_without_args_abcdefghijklmnopqrstuv
@attr_with_one_arg(
        "abcdefghijklmnopqrs")
@attr_with_two_args(
        a=true,
        b="abcd",
        c="wxyz")
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}
TEST(FormatterTests, AttributesWithComment) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
 @attr_without_args

  // comment

   @attr_with_one_arg("abcdefghijklmnopqr")
    library foo.bar;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
@attr_without_args

// comment

@attr_with_one_arg("abcdefghijklmnopqr")
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, AttributesWithDocComment) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
    /// doc comment 1
    /// doc comment 2

   @attr_without_args @attr_with_one_arg("abcdefghijklmnopqr")

library foo.bar;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
/// doc comment 1
/// doc comment 2
@attr_without_args
@attr_with_one_arg("abcdefghijklmnopqr")
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to AttributesFormatted.  The only difference is that
// the newlines and unnecessary spaces have been removed.
TEST(FormatterTests, AttributesMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(@attr_without_args @attr_with_one_arg("abcdefghijklmnopqr")library foo.bar;)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
@attr_without_args
@attr_with_one_arg("abcdefghijklmnopqr")
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, AttributesMaximalNewLines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
@attr_without_args
@attr_with_one_arg
(
"abcdefghijklmnopqr"
)
library
foo
.
bar
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
@attr_without_args
@attr_with_one_arg("abcdefghijklmnopqr")
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, AttributesWeird) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

protocol MyProtocol {
    /// Foo
@custom // Bar
        MyMethod();
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

protocol MyProtocol {
    /// Foo
    @custom // Bar
    MyMethod();
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, AttributesInlineAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyStruct = struct {
    my_field1 @no_arg_abcdefg struct {};
    my_field2 @one_arg("123") struct {};
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, AttributesInlineOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyStruct = struct {
    my_field1 @no_arg_abcdefgh struct {};
    my_field2 @one_arg("1234") struct {};
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyStruct = struct {
    my_field1
            @no_arg_abcdefgh
            struct {};
    my_field2
            @one_arg("1234")
            struct {};
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted bits declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, BitsAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyBits_Abcdefghijklmnopqrs = bits {
    value1_abcdefghijklmnopqrstuvwx = 0;
    value2_abcdefghijklmnopqrstu = 0x01;
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, BitsOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyBits_Abcdefghijklmnopqrst = bits {
    value1_abcdefghijklmnopqrstuvwxy = 0;
    value2_abcdefghijklmnopqrstuv = 0x01;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyBits_Abcdefghijklmnopqrst
        = bits {
    value1_abcdefghijklmnopqrstuvwxy
            = 0;
    value2_abcdefghijklmnopqrstuv
            = 0x01;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, BitsUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyBits_Abcdefghij= flexible bits {
 value1_abcdefghijklmnopqrstuvwx =0;
  value2_abcdefghijklmnopqrstu= 0x01;};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyBits_Abcdefghij = flexible bits {
    value1_abcdefghijklmnopqrstuvwx = 0;
    value2_abcdefghijklmnopqrstu = 0x01;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, BitsWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

 // comment 1
  /// doc comment 1

   @foo

    type MyBits_Abcdefghijklmnopqrs = bits {
    value1_abcdefghijklmnopqrstuvwx = 0;
  // comment 2

   /// doc comment 2

    @bar
     value2_abcdefghijklmnopqrstu = 0x01;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment 1
/// doc comment 1
@foo
type MyBits_Abcdefghijklmnopqrs = bits {
    value1_abcdefghijklmnopqrstuvwx = 0;
    // comment 2

    /// doc comment 2
    @bar
    value2_abcdefghijklmnopqrstu = 0x01;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to BitsFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, BitsMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;type MyBits_Abcdefghijklmnopqrs=bits{value1_abcdefghijklmnopqrstuvwx=0;value2_abcdefghijklmnopqrstu=0x01;};)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
type MyBits_Abcdefghijklmnopqrs = bits {
    value1_abcdefghijklmnopqrstuvwx = 0;
    value2_abcdefghijklmnopqrstu = 0x01;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to BitsFormatted, except that every token is on a newline.
TEST(FormatterTests, BitsMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;

type
MyBits_Abcdefghijklmnopqrs
=
bits
{
value1_abcdefghijklmnopqrstuvwx
=
0
;
value2_abcdefghijklmnopqrstu
=
0x01
;
}
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyBits_Abcdefghijklmnopqrs = bits {
    value1_abcdefghijklmnopqrstuvwx = 0;
    value2_abcdefghijklmnopqrstu = 0x01;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that already properly formatted const declarations are not modified by another run
// through the formatter.
TEST(FormatterTests, ConstAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

const MY_TRUE_ABCDEFGHIJKLM bool = true;
const MY_FALSE_ABCDEFGHIJK bool = false;
const MY_UINT64_AB uint64 = 12345678900;


const MY_FLOAT64_ABCDEF float64 = 12.34;
const MY_STRING_ABCDEFGH string = "foo";
const MY_OR_A uint64 = 1 | MY_UINT64_AB;
const MY_ORS_ABCDEFG uint64 = 1 | 2 | 3;
const MY_REF_ABCD uint64 = MY_UINT64_AB;
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}
TEST(FormatterTests, ConstUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

const    MY_TRUE_ABCDEFGHIJKLM bool = true;
const MY_FALSE_ABCDEFGHIJK bool =    false;
const MY_UINT64_AB uint64 = 12345678900   ;


  const MY_FLOAT64_ABCDEF float64 = 12.34;
   const MY_STRING_ABCDEFGH
    string = "foo";
const MY_OR_A uint64 = 1
|   MY_UINT64_AB;
const MY_ORS_ABCDEFG uint64=1|2|3;
 const MY_REF_ABCD uint64 = MY_UINT64_AB
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

const MY_TRUE_ABCDEFGHIJKLM bool = true;
const MY_FALSE_ABCDEFGHIJK bool = false;
const MY_UINT64_AB uint64 = 12345678900;


const MY_FLOAT64_ABCDEF float64 = 12.34;
const MY_STRING_ABCDEFGH string = "foo";
const MY_OR_A uint64 = 1 | MY_UINT64_AB;
const MY_ORS_ABCDEFG uint64 = 1 | 2 | 3;
const MY_REF_ABCD uint64 = MY_UINT64_AB;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// The const declaration has two levels of subspanning: the first is split at the equal sign, while
// the second is split at the type declaration.  This test cases tests for "partial" overflows where
// the first level of subspanning is invoked: the whole line is too long, but the `const NAME TYPE`
// portion still fits on the first line.
TEST(FormatterTests, ConstPartialOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

const MY_TRUE_ABCDEFGHIJKLMN bool = true;
const MY_FALSE_ABCDEFGHIJKL bool = false;
const MY_UINT64_ABC uint64 = 12345678900;


const MY_FLOAT64_ABCDEFG float64 = 12.34;
const MY_STRING_ABCDEFGHI string = "foo";
const MY_REF_ABCD uint64 = MY_UINT64_ABC;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

const MY_TRUE_ABCDEFGHIJKLMN bool
        = true;
const MY_FALSE_ABCDEFGHIJKL bool
        = false;
const MY_UINT64_ABC uint64
        = 12345678900;


const MY_FLOAT64_ABCDEFG float64
        = 12.34;
const MY_STRING_ABCDEFGHI string
        = "foo";
const MY_REF_ABCD uint64
        = MY_UINT64_ABC;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Tests cases where even the nested subspan to the left of the equal sign is longer than the
// overflow window.  Note that this test case looks a bit unusual because the name is very long, but
// the type is very short.  In reality, both would probably have to be quite long to cause this kind
// of overflow, so the output will look less "lopsided."
TEST(FormatterTests, ConstTotalOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

const MY_WAY_TOO_LONG_TRUE_ABCDEFGHIJKLMN bool = true;
const MY_WAY_TOO_LONG_FALSE_ABCDEFGHIJKLM bool = false;
const MY_WAY_TOO_LONG_UINT64_ABCDEFGHIJKL uint64 = 12345678900;


const MY_WAY_TOO_LONG_FLOAT64_ABCDEFGHIJK float64 = 12.34;
const MY_WAY_TOO_LONG_STRING_ABCDEFGHIJKL string = "foo";
const MY_WAY_TOO_LONG_REF_ABCDEFGHIJKLMNO uint64 = MY_WAY_TOO_LONG_UINT64_ABCDEFGHIJKL;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

const MY_WAY_TOO_LONG_TRUE_ABCDEFGHIJKLMN
        bool
        = true;
const MY_WAY_TOO_LONG_FALSE_ABCDEFGHIJKLM
        bool
        = false;
const MY_WAY_TOO_LONG_UINT64_ABCDEFGHIJKL
        uint64
        = 12345678900;


const MY_WAY_TOO_LONG_FLOAT64_ABCDEFGHIJK
        float64
        = 12.34;
const MY_WAY_TOO_LONG_STRING_ABCDEFGHIJKL
        string
        = "foo";
const MY_WAY_TOO_LONG_REF_ABCDEFGHIJKLMNO
        uint64
        = MY_WAY_TOO_LONG_UINT64_ABCDEFGHIJKL;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Test with comments, doc comments, and attributes added and spaced out.
TEST(FormatterTests, ConstWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

 // comment

  /// doc comment

   @attr

    const MY_TRUE_ABCDEFGHIJKLM bool = true;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment

/// doc comment
@attr
const MY_TRUE_ABCDEFGHIJKLM bool = true;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to ConstFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, ConstMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;const MY_TRUE_ABCDEFGHIJKLM bool=true;const MY_FALSE_ABCDEFGHIJK bool=false;const MY_UINT64_AB uint64=12345678900;const MY_FLOAT64_ABCDEF float64=12.34;const MY_STRING_ABCDEFGH string="foo";const MY_OR_A uint64=1|MY_UINT64_AB;const MY_ORS_ABCDEFG uint64=1|2|3;const MY_REF_ABCD uint64=MY_UINT64_AB;)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
const MY_TRUE_ABCDEFGHIJKLM bool = true;
const MY_FALSE_ABCDEFGHIJK bool = false;
const MY_UINT64_AB uint64 = 12345678900;
const MY_FLOAT64_ABCDEF float64 = 12.34;
const MY_STRING_ABCDEFGH string = "foo";
const MY_OR_A uint64 = 1 | MY_UINT64_AB;
const MY_ORS_ABCDEFG uint64 = 1 | 2 | 3;
const MY_REF_ABCD uint64 = MY_UINT64_AB;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to ConstFormatted, except that every token is on a newline.
TEST(FormatterTests, ConstMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

const
MY_TRUE_ABCDEFGHIJKLM
bool
=
true
;
const
MY_FALSE_ABCDEFGHIJK
bool
=
false
;
const
MY_UINT64_AB
uint64
=
12345678900
;


const
MY_FLOAT64_ABCDEF
float64
=
12.34
;
const
MY_STRING_ABCDEFGH
string
=
"foo"
;
const
MY_OR_A
uint64
=
1
|
MY_UINT64_AB
;
const
MY_ORS_ABCDEFG
uint64
=
1
|
2
|
3
;
const
MY_REF_ABCD
uint64
=
MY_UINT64_AB
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

const MY_TRUE_ABCDEFGHIJKLM bool = true;
const MY_FALSE_ABCDEFGHIJK bool = false;
const MY_UINT64_AB uint64 = 12345678900;


const MY_FLOAT64_ABCDEF float64 = 12.34;
const MY_STRING_ABCDEFGH string = "foo";
const MY_OR_A uint64 = 1 | MY_UINT64_AB;
const MY_ORS_ABCDEFG uint64 = 1 | 2 | 3;
const MY_REF_ABCD uint64 = MY_UINT64_AB;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted enum declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, EnumAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEnum_Abcdefghij = enum : uint32 {
    value1_abcdefghijklmnopqrstuvwx = 0;
    value2_abcdefghijklmnopqrstuvw = 01;

    @unknown
    value3_abcdefghijklmnopqrstuv = 002;
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, EnumOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyEnum_Abcdefghijk = enum : uint32 {
    value1_abcdefghijklmnopqrstuvwxy = 0;
    value2_abcdefghijklmnopqrstuvwx = 01;

    @unknown
    value3_abcdefghijklmnopqrstuvw = 002;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEnum_Abcdefghijk
        = enum : uint32 {
    value1_abcdefghijklmnopqrstuvwxy
            = 0;
    value2_abcdefghijklmnopqrstuvwx
            = 01;

    @unknown
    value3_abcdefghijklmnopqrstuvw
            = 002;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, EnumUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyEnum_Abc= strict enum : uint32 {
 value1_abcdefghijklmnopqrstuvwx =0;
  value2_abcdefghijklmnopqrstuvw= 01;

     @unknown
      value3_abcdefghijklmnopqrstuv = 002 ;};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEnum_Abc = strict enum : uint32 {
    value1_abcdefghijklmnopqrstuvwx = 0;
    value2_abcdefghijklmnopqrstuvw = 01;

    @unknown
    value3_abcdefghijklmnopqrstuv = 002;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, EnumWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

 // comment 1
  /// doc comment 1

   @foo

    type MyEnum_Abcdefghij = enum : uint32 {
    value1_abcdefghijklmnopqrstuvwx = 0;
  // comment 2

   /// doc comment 2

    @bar
     value2_abcdefghijklmnopqrstuvw = 01;

    @unknown
    value3_abcdefghijklmnopqrstuv = 002;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment 1
/// doc comment 1
@foo
type MyEnum_Abcdefghij = enum : uint32 {
    value1_abcdefghijklmnopqrstuvwx = 0;
    // comment 2

    /// doc comment 2
    @bar
    value2_abcdefghijklmnopqrstuvw = 01;

    @unknown
    value3_abcdefghijklmnopqrstuv = 002;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to EnumFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, EnumMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;type MyEnum_Abcdefghij=enum:uint32{value1_abcdefghijklmnopqrstuvwx=0;value2_abcdefghijklmnopqrstuvw=01;@unknown value3_abcdefghijklmnopqrstuv=002;};)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
type MyEnum_Abcdefghij = enum : uint32 {
    value1_abcdefghijklmnopqrstuvwx = 0;
    value2_abcdefghijklmnopqrstuvw = 01;
    @unknown
    value3_abcdefghijklmnopqrstuv = 002;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to EnumFormatted, except that every token is on a newline.
TEST(FormatterTests, EnumMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;

type
MyEnum_Abcdefghij
=
enum
:
uint32
{
value1_abcdefghijklmnopqrstuvwx
=
0
;
value2_abcdefghijklmnopqrstuvw
=
01
;

@unknown
value3_abcdefghijklmnopqrstuv
=
002
;
}
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEnum_Abcdefghij = enum : uint32 {
    value1_abcdefghijklmnopqrstuvwx = 0;
    value2_abcdefghijklmnopqrstuvw = 01;

    @unknown
    value3_abcdefghijklmnopqrstuv = 002;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, EnumMemberless) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type EmptyEnum = strict enum:uint8{};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type EmptyEnum = strict enum : uint8 {};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, EnumMemberlessCommentAfterColon) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type EmptyEnum = strict enum:// Comment
uint8{};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type EmptyEnum = strict enum : // Comment
        uint8 {};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted library declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, LibraryAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

// Test that the library declaration is never wrapped.
TEST(FormatterTests, LibraryOverflowNoop) {
  // ---------------40---------------- |
  std::string noop = R"FIDL(
library my.overlong.severely.overflowing.name;
)FIDL";

  ASSERT_FORMATTED(noop, noop);
}

// No overflow, but incorrect leading spacing and newlines.
TEST(FormatterTests, LibraryUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
  library

  foo.bar;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Test with comments, doc comments, and attributes added and spaced out.
TEST(FormatterTests, LibraryWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
 // comment

  /// doc comment

   @attr

    library foo.bar;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
// comment

/// doc comment
@attr
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to LibraryFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, LibraryMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(library foo.bar;)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to LibraryFormatted, except that every token is on a newline.
TEST(FormatterTests, LibraryMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted resource declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, ResourceAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

resource_definition default_abcdefghij {
    properties {
        obj_type subtype_abcdefghijklmn;
    };
};

resource_definition subtype_a : uint32 {
    properties {
        obj_type subtype_abcdefghijklmn;
        rights rights_abcdefghijklmnopq;
    };
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

// No part of a resource_definition should wrap on overflow.
TEST(FormatterTests, ResourceOverflowNoop) {
  // ---------------40---------------- |
  std::string noop = R"FIDL(
library foo.bar;

resource_definition default_abcdefghijk {
    properties {
        obj_type subtype_abcdefghijklmno;
    };
};

resource_definition subtype_ab : uint32 {
    properties {
        obj_type subtype_abcdefghijklmno;
        rights rights_abcdefghijklmnopqr;
    };
};
)FIDL";

  ASSERT_FORMATTED(noop, noop);
}

TEST(FormatterTests, ResourceUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

resource_definition default_abcdefghij

{
    properties  { obj_type subtype_abcdefghijklmn;
};};

resource_definition subtype_a: uint32 {properties {
obj_type subtype_abcdefghijklmn ;
  rights rights_abcdefghijklmnopq;
};
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

resource_definition default_abcdefghij {
    properties {
        obj_type subtype_abcdefghijklmn;
    };
};

resource_definition subtype_a : uint32 {
    properties {
        obj_type subtype_abcdefghijklmn;
        rights rights_abcdefghijklmnopq;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, ResourceWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

 // comment 1
  /// doc comment 1

   @foo
    resource_definition default_abcdefghij {
    properties {
  // comment 2

   /// doc comment 2

     @bar

        obj_type subtype_abcdefghijklmn;
    };
};

resource_definition subtype_a : uint32 {
    properties {
        obj_type subtype_abcdefghijklmn;
// comment 3
/// doc comment 3

     @baz
        rights rights_abcdefghijklmnopq;
    };
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment 1
/// doc comment 1
@foo
resource_definition default_abcdefghij {
    properties {
        // comment 2

        /// doc comment 2
        @bar
        obj_type subtype_abcdefghijklmn;
    };
};

resource_definition subtype_a : uint32 {
    properties {
        obj_type subtype_abcdefghijklmn;
        // comment 3
        /// doc comment 3
        @baz
        rights rights_abcdefghijklmnopq;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to ResourceFormatted.  The only difference is that
// the newlines and unnecessary spaces have been removed.
TEST(FormatterTests, ResourceMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;resource_definition default_abcdefghij{properties{obj_type subtype_abcdefghijklmn;};};resource_definition subtype_a:uint32{properties{obj_type subtype_abcdefghijklmn;rights rights_abcdefghijklmnopq;};};)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
resource_definition default_abcdefghij {
    properties {
        obj_type subtype_abcdefghijklmn;
    };
};
resource_definition subtype_a : uint32 {
    properties {
        obj_type subtype_abcdefghijklmn;
        rights rights_abcdefghijklmnopq;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to ResourceFormatted, except that every token is on a newline.
TEST(FormatterTests, ResourceMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;

resource_definition
default_abcdefghij
{
properties
{
obj_type
subtype_abcdefghijklmn
;
}
;
}
;

resource_definition
subtype_a
:
uint32
{
properties
{
obj_type
subtype_abcdefghijklmn
;
rights
rights_abcdefghijklmnopq
;
}
;
}
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

resource_definition default_abcdefghij {
    properties {
        obj_type subtype_abcdefghijklmn;
    };
};

resource_definition subtype_a : uint32 {
    properties {
        obj_type subtype_abcdefghijklmn;
        rights rights_abcdefghijklmnopq;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted service declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, ServiceAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

service MyEmptyService_Abcdefghijklm {};

service MyPopulatedService_Abcdefghik {
    import_ab client_end:foo.baz.Import;
    local_abcdefghijkl client_end:Local;
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

// No part of the service should wrap if it overflows.
TEST(FormatterTests, ServiceOverflowNoop) {
  // ---------------40---------------- |
  std::string noop = R"FIDL(
library foo.bar;

service MyEmptyService_Abcdefghijklmn {};

service MyPopulatedService_Abcdefghikl {
    import_abc client_end:foo.baz.Import;
    local_abcdefghijklm client_end:Local;
};
)FIDL";

  ASSERT_FORMATTED(noop, noop);
}

TEST(FormatterTests, ServiceUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

service
MyEmptyService_Abcdefghijklmn {  };

service MyPopulatedService_Abcdefghikl
{
  import_abc client_end:foo.baz.Import ;
    local_abcdefghijklm client_end: Local;};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

service MyEmptyService_Abcdefghijklmn {};

service MyPopulatedService_Abcdefghikl {
    import_abc client_end:foo.baz.Import;
    local_abcdefghijklm client_end:Local;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Test with comments, doc comments, and attributes added and spaced out.
TEST(FormatterTests, ServiceWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;


 // comment 1
  /// doc comment 1

   @foo
service MyEmptyService_Abcdefghijklmn {};

service MyPopulatedService_Abcdefghikl {
    import_abc client_end:foo.baz.Import;
  // comment 2

   /// doc comment 2

     @bar

      local_abcdefghijklm client_end:Local;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;


// comment 1
/// doc comment 1
@foo
service MyEmptyService_Abcdefghijklmn {};

service MyPopulatedService_Abcdefghikl {
    import_abc client_end:foo.baz.Import;
    // comment 2

    /// doc comment 2
    @bar
    local_abcdefghijklm client_end:Local;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to ServiceFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, ServiceMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;service MyEmptyService_Abcdefghijklm{};service MyPopulatedService_Abcdefghikl{import_ab client_end:foo.baz.Import;local_abcdefghijkl client_end:Local;};)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
service MyEmptyService_Abcdefghijklm {};
service MyPopulatedService_Abcdefghikl {
    import_ab client_end:foo.baz.Import;
    local_abcdefghijkl client_end:Local;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to ServiceFormatted, except that every token is on a newline.
TEST(FormatterTests, ServiceMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;

service
MyEmptyService_Abcdefghijklmn
{
}
;

service
MyPopulatedService_Abcdefghikl
{
import_abc
client_end
:
foo
.
baz
.
Import
;
local_abcdefghijklm
client_end
:
Local
;
}
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

service MyEmptyService_Abcdefghijklmn {};

service MyPopulatedService_Abcdefghikl {
    import_abc client_end:foo.baz.Import;
    local_abcdefghijklm client_end:Local;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted struct declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, StructAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEmptyStruct_Abcdefgh = struct {};

type MyPopulatedStruct_Abcdef = struct {
    field1_abcdefghijklmnopqrstuvw bool;
    field2_abcdefghijklmno bool = false;

    field3_abcdefghijklmnopqrst struct {
        nested1_abcdef vector<uint8>:16;
        nested2_abcdef string = "abcde";
    };
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, ProtocolNoArgumentsAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqrst {
    compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrstuvwxyz();
    OneWayNull_Abcdefghijklm(struct {});

    TwoWay_Abcdefghijklmnopqrst() -> ();
    TwoWayNil(struct {}) -> (struct {});
    TwoWayError_Ab() -> () error uint32;

    compose Empty_Abcdefghijklmnopqrstu;

    -> Event_Abcdefghijklmnopqrstuvwx();
    -> EventNull_Abcdefghijk(struct {});
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

// Aside from the contents of the request/response layouts themselves, nothing in a protocol
// definition should cause wrapping on overflow.
TEST(FormatterTests, ProtocolNoArgumentsOverflowNoop) {
  // ---------------40---------------- |
  std::string noop = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstuv {};

protocol Composed_Abcdefghijklmnopqrs {
    compose Empty_Abcdefghijklmnopqrstuv;
};

protocol Populated_Abcdefghijklmnopqrst {
    compose Composed_Abcdefghijklmnopqrs;
    OneWay_Abcdefghijklmnopqrstuvwxyzz();
    OneWayNull_Abcdefghijklmn(struct {});

    TwoWay_Abcdefghijklmnopqrstu() -> ();
    TwoWayNils(struct {}) -> (struct {});
    TwoWayError_Abc() -> () error uint32;

    compose Empty_Abcdefghijklmnopqrstuv;

    -> Event_Abcdefghijklmnopqrstuvwxy();
    -> EventNull_Abcdefghijkl(struct {});
};
)FIDL";

  ASSERT_FORMATTED(noop, noop);
}

TEST(FormatterTests, ProtocolNoArgumentsUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstu {   }
;

protocol
Composed_Abcdefghijklmnopqr   {
    compose Empty_Abcdefghijklmnopqrstu ;
};

protocol Populated_Abcdefghijklmnopqrs   {
 compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrstuvwxyz( );
    OneWayNull_Abcdefghijklm (struct {});

    TwoWay_Abcdefghijklmnopqrst()   -> ();
    TwoWayNil(struct {}) ->   (struct{ });
    TwoWayError_Ab() -> ()error
uint32;

    compose Empty_Abcdefghijklmnopqrstu   ;

    ->Event_Abcdefghijklmnopqrstuvwx() ;
    -> EventNull_Abcdefghijk(  struct {  });};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqr {
    compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrstuvwxyz();
    OneWayNull_Abcdefghijklm(struct {});

    TwoWay_Abcdefghijklmnopqrst() -> ();
    TwoWayNil(struct {}) -> (struct {});
    TwoWayError_Ab() -> () error uint32;

    compose Empty_Abcdefghijklmnopqrstu;

    -> Event_Abcdefghijklmnopqrstuvwx();
    -> EventNull_Abcdefghijk(struct {});
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, ProtocolNoArgumentsWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

 // comment 1
  /// doc comment 1

   @foo

    protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqr {
    // comment 2

   /// doc comment 2

     @bar

      compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrstuvwxyz();

// comment 3
/// doc comment 3

     @baz
    OneWayNull_Abcdefghijklm(struct {});

    TwoWay_Abcdefghijklmnopqrst() -> ();
    TwoWayNil(struct {}) -> (struct {});
    TwoWayError_Ab() -> () error uint32;

    // comment 4
    /// doc comment 4
    @qux
    compose Empty_Abcdefghijklmnopqrstu;

   // comment 5

  /// doc comment 5
@abc
    -> Event_Abcdefghijklmnopqrstuvwx();
    -> EventNull_Abcdefghijk(struct {});
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment 1
/// doc comment 1
@foo
protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqr {
    // comment 2

    /// doc comment 2
    @bar
    compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrstuvwxyz();

    // comment 3
    /// doc comment 3
    @baz
    OneWayNull_Abcdefghijklm(struct {});

    TwoWay_Abcdefghijklmnopqrst() -> ();
    TwoWayNil(struct {}) -> (struct {});
    TwoWayError_Ab() -> () error uint32;

    // comment 4
    /// doc comment 4
    @qux
    compose Empty_Abcdefghijklmnopqrstu;

    // comment 5

    /// doc comment 5
    @abc
    -> Event_Abcdefghijklmnopqrstuvwx();
    -> EventNull_Abcdefghijk(struct {});
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to ProtocolNoArgumentsFormatted.  The only difference
// is that the newlines and unnecessary spaces have been removed.
TEST(FormatterTests, ProtocolNoArgumentsMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;protocol Empty_Abcdefghijklmnopqrstu{};protocol Composed_Abcdefghijklmnopqrst{compose Empty_Abcdefghijklmnopqrstu;};protocol Populated_Abcdefghijklmnopqrs{compose Composed_Abcdefghijklmnopqr;OneWay_Abcdefghijklmnopqrstuvwxyz();OneWayNull_Abcdefghijklm(struct{});TwoWay_Abcdefghijklmnopqrst()->();TwoWayNil(struct{})->(struct{});TwoWayError_Ab()->()error uint32;compose Empty_Abcdefghijklmnopqrstu;->Event_Abcdefghijklmnopqrstuvwx();->EventNull_Abcdefghijk(struct{});};)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
protocol Empty_Abcdefghijklmnopqrstu {};
protocol Composed_Abcdefghijklmnopqrst {
    compose Empty_Abcdefghijklmnopqrstu;
};
protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrstuvwxyz();
    OneWayNull_Abcdefghijklm(struct {});
    TwoWay_Abcdefghijklmnopqrst() -> ();
    TwoWayNil(struct {}) -> (struct {});
    TwoWayError_Ab() -> () error uint32;
    compose Empty_Abcdefghijklmnopqrstu;
    -> Event_Abcdefghijklmnopqrstuvwx();
    -> EventNull_Abcdefghijk(struct {});
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to ProtocolNoArgumentsFormatted, except that every token is on a newline.
TEST(FormatterTests, ProtocolNoArgumentsMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;

protocol
Empty_Abcdefghijklmnopqrstu
{
}
;

protocol
Composed_Abcdefghijklmnopqr
{
compose Empty_Abcdefghijklmnopqrstu
;
}
;

protocol
Populated_Abcdefghijklmnopqrs
{
compose
Composed_Abcdefghijklmnopqr
;
OneWay_Abcdefghijklmnopqrstuvwxyz(
)
;
OneWayNull_Abcdefghijklm(
struct
{
}
)
;

TwoWay_Abcdefghijklmnopqrst
(
)
->
(
)
;
TwoWayNil
(
struct
{
}
)
->
(
struct
{
}
)
;
TwoWayError_Ab
(
)
->
(
)
error
uint32
;

compose
Empty_Abcdefghijklmnopqrstu
;

->
Event_Abcdefghijklmnopqrstuvwx
(
)
;
->
EventNull_Abcdefghijk
(
struct
{
}
)
;
}
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqr {
    compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrstuvwxyz();
    OneWayNull_Abcdefghijklm(struct {});

    TwoWay_Abcdefghijklmnopqrst() -> ();
    TwoWayNil(struct {}) -> (struct {});
    TwoWayError_Ab() -> () error uint32;

    compose Empty_Abcdefghijklmnopqrstu;

    -> Event_Abcdefghijklmnopqrstuvwx();
    -> EventNull_Abcdefghijk(struct {});
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, ProtocolWithArgumentsAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqr {
    compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrst(struct {
        req1_abcdefghijklmnopqrstu bool;
    });

    TwoWay_Abcdefghijklmnopqrst(struct {
        req2_abcdefghijklmnopqrstu bool;
    }) -> (struct {
        res3_abcdefghijklmnopqrstu bool;
    });
    TwoWayError_Abcdefghijklmno(struct {
        req4_abcdefghijklm bool = false;
        req5_abcdefghijklmnopqr struct {
            inner1_abcdefghijklmno int8;
        };
    }) -> (struct {
        res6_abcdefghijklmnopqrstu bool;
    }) error uint32;

    compose Empty_Abcdefghijklmnopqrstu;

    -> Event_Abcdefghijklmnopqr(struct {
        res7_abcdefghijklmnopqrstu bool;
    });
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, ProtocolWithArgumentsOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstuv {};

protocol Composed_Abcdefghijklmnopqrs {
    compose Empty_Abcdefghijklmnopqrstuv;
};

protocol Populated_Abcdefghijklmnopqrst {
    compose Composed_Abcdefghijklmnopqrs;
    OneWay_Abcdefghijklmnopqrstu(struct {
        req1_abcdefghijklmnopqrstuv bool;
    });

    TwoWay_Abcdefghijklmnopqrstu(struct {
        req2_abcdefghijklmnopqrstuv bool;
    }) -> (struct {
        res3_abcdefghijklmnopqrstuv bool;
    });
    TwoWayError_Abcdefghijklmnop(struct {
        req4_abcdefghijklmo bool = false;
        req5_abcdefghijklmnopqrs struct {
            inner1_abcdefghijklmnop int8;
        };
    }) -> (struct {
        res6_abcdefghijklmnopqrstuv bool;
    }) error uint32;

    compose Empty_Abcdefghijklmnopqrstuv;

    -> Event_Abcdefghijklmnopqrs(struct {
        res7_abcdefghijklmnopqrstuv bool;
    });
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstuv {};

protocol Composed_Abcdefghijklmnopqrs {
    compose Empty_Abcdefghijklmnopqrstuv;
};

protocol Populated_Abcdefghijklmnopqrst {
    compose Composed_Abcdefghijklmnopqrs;
    OneWay_Abcdefghijklmnopqrstu(struct {
        req1_abcdefghijklmnopqrstuv
                bool;
    });

    TwoWay_Abcdefghijklmnopqrstu(struct {
        req2_abcdefghijklmnopqrstuv
                bool;
    }) -> (struct {
        res3_abcdefghijklmnopqrstuv
                bool;
    });
    TwoWayError_Abcdefghijklmnop(struct {
        req4_abcdefghijklmo
                bool
                = false;
        req5_abcdefghijklmnopqrs
                struct {
            inner1_abcdefghijklmnop
                    int8;
        };
    }) -> (struct {
        res6_abcdefghijklmnopqrstuv
                bool;
    }) error uint32;

    compose Empty_Abcdefghijklmnopqrstuv;

    -> Event_Abcdefghijklmnopqrs(struct {
        res7_abcdefghijklmnopqrstuv
                bool;
    });
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, ProtocolWithArgumentsUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstu  {};

protocol Composed_Abcdefghijklmnopqr
{ compose Empty_Abcdefghijklmnopqrstu; };

protocol Populated_Abcdefghijklmnopqrs
 {
    compose
Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrst(struct {
        req1_abcdefghijklmnopqrstu bool ;
    }  );

    TwoWay_Abcdefghijklmnopqrst(struct   {
        req2_abcdefghijklmnopqrstu bool;
    }) ->(struct {
  res3_abcdefghijklmnopqrstu  bool;
    });
    TwoWayError_Abcdefghijklmno  (struct {
        req4_abcdefghijklm bool= false;
        req5_abcdefghijklmnopqr struct {
            inner1_abcdefghijklmno   int8;};
} )->(   struct
{
res6_abcdefghijklmnopqrstu
bool;}
)
error uint32;

 compose Empty_Abcdefghijklmnopqrstu;

    ->Event_Abcdefghijklmnopqr(  struct { res7_abcdefghijklmnopqrstu bool;});};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqr {
    compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrst(struct {
        req1_abcdefghijklmnopqrstu bool;
    });

    TwoWay_Abcdefghijklmnopqrst(struct {
        req2_abcdefghijklmnopqrstu bool;
    }) -> (struct {
        res3_abcdefghijklmnopqrstu bool;
    });
    TwoWayError_Abcdefghijklmno(struct {
        req4_abcdefghijklm bool = false;
        req5_abcdefghijklmnopqr struct {
            inner1_abcdefghijklmno int8;
        };
    }) -> (struct {
        res6_abcdefghijklmnopqrstu bool;
    }) error uint32;

    compose Empty_Abcdefghijklmnopqrstu;

    -> Event_Abcdefghijklmnopqr(struct {
        res7_abcdefghijklmnopqrstu bool;
    });
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, ProtocolWithArgumentsWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

 // comment 1
  /// doc comment 1

   @foo

    protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqr {
    // comment 2

   /// doc comment 2

     @bar

      compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;

// comment 3
/// doc comment 3

     @baz
    OneWay_Abcdefghijklmnopqrst(struct {
        req1_abcdefghijklmnopqrstu bool;
    });

    TwoWay_Abcdefghijklmnopqrst(struct {
        req2_abcdefghijklmnopqrstu bool;
    }) -> (struct {
        res3_abcdefghijklmnopqrstu bool;
    });
    TwoWayError_Abcdefghijklmno(struct {
        req4_abcdefghijklm bool = false;
        req5_abcdefghijklmnopqr struct {
            inner1_abcdefghijklmno int8;
        };
    }) -> (struct {
        res6_abcdefghijklmnopqrstu bool;
    }) error uint32;

    // comment 4
    /// doc comment 4
    @qux
    compose Empty_Abcdefghijklmnopqrstu;

   // comment 5

  /// doc comment 5
@abc
    -> Event_Abcdefghijklmnopqr(struct {
        res7_abcdefghijklmnopqrstu bool;
    });
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment 1
/// doc comment 1
@foo
protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqr {
    // comment 2

    /// doc comment 2
    @bar
    compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;

    // comment 3
    /// doc comment 3
    @baz
    OneWay_Abcdefghijklmnopqrst(struct {
        req1_abcdefghijklmnopqrstu bool;
    });

    TwoWay_Abcdefghijklmnopqrst(struct {
        req2_abcdefghijklmnopqrstu bool;
    }) -> (struct {
        res3_abcdefghijklmnopqrstu bool;
    });
    TwoWayError_Abcdefghijklmno(struct {
        req4_abcdefghijklm bool = false;
        req5_abcdefghijklmnopqr struct {
            inner1_abcdefghijklmno int8;
        };
    }) -> (struct {
        res6_abcdefghijklmnopqrstu bool;
    }) error uint32;

    // comment 4
    /// doc comment 4
    @qux
    compose Empty_Abcdefghijklmnopqrstu;

    // comment 5

    /// doc comment 5
    @abc
    -> Event_Abcdefghijklmnopqr(struct {
        res7_abcdefghijklmnopqrstu bool;
    });
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to ProtocolWithArgumentsFormatted.  The only
// difference is that the newlines and unnecessary spaces have been removed.
TEST(FormatterTests, ProtocolWithArgumentsMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;protocol Empty_Abcdefghijklmnopqrstu{};protocol Composed_Abcdefghijklmnopqrst{compose Empty_Abcdefghijklmnopqrstu;};protocol Populated_Abcdefghijklmnopqrs{compose Composed_Abcdefghijklmnopqr;OneWay_Abcdefghijklmnopqrst(struct{req1_abcdefghijklmnopqrstu bool;});TwoWay_Abcdefghijklmnopqrst(struct{req2_abcdefghijklmnopqrstu bool;})->(struct{res3_abcdefghijklmnopqrstu bool;});TwoWayError_Abcdefghijklmno(struct{req4_abcdefghijklm bool=false;req5_abcdefghijklmnopqr struct{inner1_abcdefghijklmno int8;};})->(struct{res6_abcdefghijklmnopqrstu bool;})error uint32;compose Empty_Abcdefghijklmnopqrstu;->Event_Abcdefghijklmnopqr(struct{res7_abcdefghijklmnopqrstu bool;});};)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
protocol Empty_Abcdefghijklmnopqrstu {};
protocol Composed_Abcdefghijklmnopqrst {
    compose Empty_Abcdefghijklmnopqrstu;
};
protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrst(struct {
        req1_abcdefghijklmnopqrstu bool;
    });
    TwoWay_Abcdefghijklmnopqrst(struct {
        req2_abcdefghijklmnopqrstu bool;
    }) -> (struct {
        res3_abcdefghijklmnopqrstu bool;
    });
    TwoWayError_Abcdefghijklmno(struct {
        req4_abcdefghijklm bool = false;
        req5_abcdefghijklmnopqr struct {
            inner1_abcdefghijklmno int8;
        };
    }) -> (struct {
        res6_abcdefghijklmnopqrstu bool;
    }) error uint32;
    compose Empty_Abcdefghijklmnopqrstu;
    -> Event_Abcdefghijklmnopqr(struct {
        res7_abcdefghijklmnopqrstu bool;
    });
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to ProtocolWithArgumentsFormatted, except that every token is on a newline.
TEST(FormatterTests, ProtocolWithArgumentsMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;

protocol
Empty_Abcdefghijklmnopqrstu
{
}
;

protocol
Composed_Abcdefghijklmnopqr
{
compose Empty_Abcdefghijklmnopqrstu;
}
;

protocol
Populated_Abcdefghijklmnopqrs
{
compose
Composed_Abcdefghijklmnopqr
;
OneWay_Abcdefghijklmnopqrst
(
struct
{
req1_abcdefghijklmnopqrstu
bool
;
}
)
;

TwoWay_Abcdefghijklmnopqrst
(
struct
{
req2_abcdefghijklmnopqrstu
bool
;
}
)
->
(
struct
{
res3_abcdefghijklmnopqrstu
bool
;
}
)
;
TwoWayError_Abcdefghijklmno
(
struct
{
req4_abcdefghijklm
bool
=
false
;
req5_abcdefghijklmnopqr
struct
{
inner1_abcdefghijklmno
int8
;
}
;
}
)
->
(struct
{
res6_abcdefghijklmnopqrstu
bool
;
}
)
error
uint32
;

compose
Empty_Abcdefghijklmnopqrstu
;

->
Event_Abcdefghijklmnopqr(struct
{
res7_abcdefghijklmnopqrstu
bool
;
}
)
;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

protocol Empty_Abcdefghijklmnopqrstu {};

protocol Composed_Abcdefghijklmnopqr {
    compose Empty_Abcdefghijklmnopqrstu;
};

protocol Populated_Abcdefghijklmnopqrs {
    compose Composed_Abcdefghijklmnopqr;
    OneWay_Abcdefghijklmnopqrst(struct {
        req1_abcdefghijklmnopqrstu bool;
    });

    TwoWay_Abcdefghijklmnopqrst(struct {
        req2_abcdefghijklmnopqrstu bool;
    }) -> (struct {
        res3_abcdefghijklmnopqrstu bool;
    });
    TwoWayError_Abcdefghijklmno(struct {
        req4_abcdefghijklm bool = false;
        req5_abcdefghijklmnopqr struct {
            inner1_abcdefghijklmno int8;
        };
    }) -> (struct {
        res6_abcdefghijklmnopqrstu bool;
    }) error uint32;

    compose Empty_Abcdefghijklmnopqrstu;

    -> Event_Abcdefghijklmnopqr(struct {
        res7_abcdefghijklmnopqrstu bool;
    });
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// https://fxbug.dev/42158841
TEST(FormatterTests, ProtocolMethodBeforeCompose) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library example;

protocol MyProtocol {
    MyMethod(struct { t T; }) -> (struct { u U; });

    compose Bar;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example;

protocol MyProtocol {
    MyMethod(struct {
        t T;
    }) -> (struct {
        u U;
    });

    compose Bar;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, StructOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyEmptyStruct_Abcdefghi = struct {};
type MyPopulatedStruct_Abcdefg = struct {
    field1_abcdefghijklmnopqrstuvwx bool;
    field2_abcdefghijklmnop bool = false;

    field3_abcdefghijklmnopqrstu struct {
        nested1_abcdefg vector<uint8>:16;
        nested2_abcdef string = "abcdef";
    };
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEmptyStruct_Abcdefghi
        = struct {};
type MyPopulatedStruct_Abcdefg
        = struct {
    field1_abcdefghijklmnopqrstuvwx
            bool;
    field2_abcdefghijklmnop
            bool
            = false;

    field3_abcdefghijklmnopqrstu
            struct {
        nested1_abcdefg
                vector<uint8>:16;
        nested2_abcdef
                string
                = "abcdef";
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, StructUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyEmptyStruct_Abcdefgh = struct {
};

type MyStruct_Abcdef= resource struct {
 field1_abcdefghijklmnopqrstuvw bool;
      field2_abcdefghijklmno bool = false;

       field3_abcdefghijklmnopqrst struct {
 nested1_abcdef vector<  uint8>:16;
   nested2_abcdef string = "abcde";};


};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEmptyStruct_Abcdefgh = struct {};

type MyStruct_Abcdef = resource struct {
    field1_abcdefghijklmnopqrstuvw bool;
    field2_abcdefghijklmno bool = false;

    field3_abcdefghijklmnopqrst struct {
        nested1_abcdef vector<uint8>:16;
        nested2_abcdef string = "abcde";
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Test with comments, doc comments, and attributes added.
TEST(FormatterTests, StructWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

 // comment 1
  /// doc comment 1

   @foo

    type MyEmptyStruct_Abcdefgh = struct {};

type MyPopulatedStruct_Abcdef = struct {
    field1_abcdefghijklmnopqrstuvw bool;

  // comment 2

   /// doc comment 2

     @bar

      field2_abcdefghijklmno bool = false;
    field3_abcdefghijklmnopqrst struct {
     // comment 3
      /// doc comment 3
       @baz("qux")
        nested1_abcdef vector<uint8>:16;
        nested2_abcdef string = "abcde";
    };
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment 1
/// doc comment 1
@foo
type MyEmptyStruct_Abcdefgh = struct {};

type MyPopulatedStruct_Abcdef = struct {
    field1_abcdefghijklmnopqrstuvw bool;

    // comment 2

    /// doc comment 2
    @bar
    field2_abcdefghijklmno bool = false;
    field3_abcdefghijklmnopqrst struct {
        // comment 3
        /// doc comment 3
        @baz("qux")
        nested1_abcdef vector<uint8>:16;
        nested2_abcdef string = "abcde";
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to StructFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, StructMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;type MyEmptyStruct_Abcdefgh=struct{};type MyPopulatedStruct_Abcdef=struct{field1_abcdefghijklmnopqrstuvw bool;field2_abcdefghijklmno bool=false;field3_abcdefghijklmnopqrst struct{nested1_abcdef vector<uint8>:16;nested2_abcdef string="abcde";};};)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
type MyEmptyStruct_Abcdefgh = struct {};
type MyPopulatedStruct_Abcdef = struct {
    field1_abcdefghijklmnopqrstuvw bool;
    field2_abcdefghijklmno bool = false;
    field3_abcdefghijklmnopqrst struct {
        nested1_abcdef vector<uint8>:16;
        nested2_abcdef string = "abcde";
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to StructFormatted, except that every token is on a newline.
TEST(FormatterTests, StructMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;
type
MyEmptyStruct_Abcdefgh
=
struct
{
}
;
type
MyPopulatedStruct_Abcdef
=
struct
{
field1_abcdefghijklmnopqrstuvw
bool
;
field2_abcdefghijklmno
bool
=
false
;
field3_abcdefghijklmnopqrst
struct
{
nested1_abcdef
vector
<
uint8
>
:
16
;
nested2_abcdef
string
=
"abcde"
;
}
;
}
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
type MyEmptyStruct_Abcdefgh = struct {};
type MyPopulatedStruct_Abcdef = struct {
    field1_abcdefghijklmnopqrstuvw bool;
    field2_abcdefghijklmno bool = false;
    field3_abcdefghijklmnopqrst struct {
        nested1_abcdef vector<uint8>:16;
        nested2_abcdef string = "abcde";
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted table declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, TableAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEmptyTable_Abcdefghij = table {};

type MyPopulatedTable_Abcdefgh = table {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;

    3: field3_abcdefghijklmnopqr table {
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, TableOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyEmptyTable_Abcdefghijk = table {};
type MyPopulatedTable_Abcdefghi = table {
    1: field1_abcdefghijklmnopqrstu bool;
    2: field2 bool;

    3: field3_abcdefghijklmnopqrs table {
        1: nested1_abcd vector<uint8>:16;
    };
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEmptyTable_Abcdefghijk
        = table {};
type MyPopulatedTable_Abcdefghi
        = table {
    1: field1_abcdefghijklmnopqrstu
            bool;
    2: field2 bool;

    3: field3_abcdefghijklmnopqrs
            table {
        1: nested1_abcd
                vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, TableUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyEmptyTable_Abcdefghij =   table  { } ;

type MyPopulatedTable_Abcdefgh= table {
    1:   field1_abcdefghijklmnopqrst bool;
    2  : field2 bool;

    3:field3_abcdefghijklmnopqr    table
{
        1
:nested1_abc  vector<uint8>:16  ;};

};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEmptyTable_Abcdefghij = table {};

type MyPopulatedTable_Abcdefgh = table {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;

    3: field3_abcdefghijklmnopqr table {
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, TableOrdinalsMultipleDigits) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyTable = table {
    1: field1 bool;
   12: field12 bool;
    // comment 1
  123: field123 bool;
 1234: field1234 bool;
12345: field12345 bool;
123456: field123456 table {
        1: field1 bool;
       12: field12 bool;
      123: field123 bool;

        // comment 2
     1234: field1234 bool;
    12345: field12345 bool;
   123456: field123456 table {
            1: field1 bool;
           12: field12 bool;
          123: field123 bool;
         1234: field1234 bool;
            // comment 3

        12345: field12345 bool;
       123456: field123456
                    table {};
        };
    };
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyTable = table {
    1: field1 bool;
    12: field12 bool;
    // comment 1
    123: field123 bool;
    1234: field1234 bool;
    12345: field12345 bool;
    123456: field123456 table {
        1: field1 bool;
        12: field12 bool;
        123: field123 bool;

        // comment 2
        1234: field1234 bool;
        12345: field12345 bool;
        123456: field123456 table {
            1: field1 bool;
            12: field12 bool;
            123: field123 bool;
            1234: field1234 bool;
            // comment 3

            12345: field12345 bool;
            123456: field123456
                    table {};
        };
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Test with comments, doc comments, and attributes added.
TEST(FormatterTests, TableWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(
library foo.bar;

 // comment 1
  /// doc comment 1

   @foo

    type MyEmptyTable_Abcdefghij = table {};

type MyPopulatedTable_Abcdefgh = table {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;

  // comment 2

   /// doc comment 2

     @bar

      3: field3_abcdefghijklmnopqr table {
        // comment 3
         /// doc comment 3
          @baz("qux")
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment 1
/// doc comment 1
@foo
type MyEmptyTable_Abcdefghij = table {};

type MyPopulatedTable_Abcdefgh = table {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;

    // comment 2

    /// doc comment 2
    @bar
    3: field3_abcdefghijklmnopqr table {
        // comment 3
        /// doc comment 3
        @baz("qux")
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to TableFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, TableMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;type MyEmptyTable_Abcdefghij=table{};type MyPopulatedTable_Abcdefgh=table{1:field1_abcdefghijklmnopqrst bool;2:field2 bool;3:field3_abcdefghijklmnopqr table{1:nested1_abc vector<uint8>:16;};};)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
type MyEmptyTable_Abcdefghij = table {};
type MyPopulatedTable_Abcdefgh = table {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;
    3: field3_abcdefghijklmnopqr table {
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to TableFormatted, except that every token is on a newline.
TEST(FormatterTests, TableMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;

type
MyEmptyTable_Abcdefghij
=
table
{
}
;

type
MyPopulatedTable_Abcdefgh
=
table
{
1
:
field1_abcdefghijklmnopqrst
bool
;
2
:
field2
bool
;
3
:
field3_abcdefghijklmnopqr
table
{
1
:
nested1_abc
vector
<
uint8
>
:
16
;
}
;
}
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyEmptyTable_Abcdefghij = table {};

type MyPopulatedTable_Abcdefgh = table {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;
    3: field3_abcdefghijklmnopqr table {
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted union declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, UnionAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyUnion_Abcdefghijklmnopq = union {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;

    3: field3_abcdefghijklmnopqr union {
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, UnionOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyUnion_Abcdefghijklmnopqr = union {
    1: field1_abcdefghijklmnopqrstu bool;
    2: field2 bool;

    3: field3_abcdefghijklmnopqrs union {
        1: nested1_abcd vector<uint8>:16;
    };
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyUnion_Abcdefghijklmnopqr
        = union {
    1: field1_abcdefghijklmnopqrstu
            bool;
    2: field2 bool;

    3: field3_abcdefghijklmnopqrs
            union {
        1: nested1_abcd
                vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, UnionUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyUnion_A= strict resource union {
    1:   field1_abcdefghijklmnopqrst bool;
    2  : field2 bool;

    3:field3_abcdefghijklmnopqr    union
{
        1
:nested1_abc  vector<uint8>:16  ;};

};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyUnion_A = strict resource union {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;

    3: field3_abcdefghijklmnopqr union {
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, UnionOrdinalsMultipleDigits) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyUnion = flexible resource union {
    1: field1 bool;
   12: field12 bool;
    // comment 1
  123: field123 bool;
 1234: field1234 bool;
12345: field12345 bool;
123456: field123456 flexible union {
        1: field1 bool;
       12: field12 bool;
      123: field123 bool;

        // comment 2
     1234: field1234 bool;
    12345: field12345 bool;
   123456: field123456
                strict union {
            1: field1 bool;
           12: field12 bool;
          123: field123 bool;
         1234: field1234 bool;
            // comment 3

        12345: field12345 bool;
       123456: field123456
                    struct {};
        };
    };
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyUnion = flexible resource union {
    1: field1 bool;
    12: field12 bool;
    // comment 1
    123: field123 bool;
    1234: field1234 bool;
    12345: field12345 bool;
    123456: field123456 flexible union {
        1: field1 bool;
        12: field12 bool;
        123: field123 bool;

        // comment 2
        1234: field1234 bool;
        12345: field12345 bool;
        123456: field123456
                strict union {
            1: field1 bool;
            12: field12 bool;
            123: field123 bool;
            1234: field1234 bool;
            // comment 3

            12345: field12345 bool;
            123456: field123456
                    struct {};
        };
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Test with comments, doc comments, and attributes added.
TEST(FormatterTests, UnionWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(
library foo.bar;

 // comment 1
  /// doc comment 1

   @foo
type MyUnion_Abcdefgh = resource union {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;

  // comment 2

   /// doc comment 2

     @bar

      3: field3_abcdefghijklmnopqr union {
     // comment 3
         /// doc comment 3
          @baz("qux")
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment 1
/// doc comment 1
@foo
type MyUnion_Abcdefgh = resource union {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;

    // comment 2

    /// doc comment 2
    @bar
    3: field3_abcdefghijklmnopqr union {
        // comment 3
        /// doc comment 3
        @baz("qux")
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to UnionFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, UnionMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted =
      R"FIDL(library foo.bar;type MyUnion_Abcdefghijklmnopq=union{1:field1_abcdefghijklmnopqrst bool;2:field2 bool;3:field3_abcdefghijklmnopqr union{1:nested1_abc vector<uint8>:16;};};)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
type MyUnion_Abcdefghijklmnopq = union {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;
    3: field3_abcdefghijklmnopqr union {
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to UnionFormatted, except that every token is on a newline.
TEST(FormatterTests, UnionMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library
foo
.
bar
;

type
MyUnion_Abcdefghijklmnopq
=
union
{
1
:
field1_abcdefghijklmnopqrst
bool
;
2
:
field2
bool
;
3
:
field3_abcdefghijklmnopqr
union
{
1
:
nested1_abc
vector
<
uint8
>
:
16
;
}
;
}
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyUnion_Abcdefghijklmnopq = union {
    1: field1_abcdefghijklmnopqrst bool;
    2: field2 bool;
    3: field3_abcdefghijklmnopqr union {
        1: nested1_abc vector<uint8>:16;
    };
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted using declaration is not modified by another run
// through the formatter.
TEST(FormatterTests, UsingAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

using imported.abcdefhijklmnopqrstubwxy;
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, UsingUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

  using imported.
 abcdefhijklmnopqrstubwxy;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

using imported.abcdefhijklmnopqrstubwxy;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Test that a using declaration with no alias does not get wrapped.
TEST(FormatterTests, UsingOverflowNoop) {
  // ---------------40---------------- |
  std::string noop = R"FIDL(
library foo.bar;

using imported.abcdefhijklmnopqrstubwxyz;
)FIDL";

  ASSERT_FORMATTED(noop, noop);
}

// Test with comments, doc comments, and attributes added and spaced out.
TEST(FormatterTests, UsingWithAllAnnotations) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

 // comment

  /// doc comment

   @attr

    using imported.abcdefhijklmnopqrstubwxy;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment

/// doc comment
@attr
using imported.abcdefhijklmnopqrstubwxy;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to UsingFormatted.  The only difference is that the
// newlines and unnecessary spaces have been removed.
TEST(FormatterTests, UsingMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(library foo.bar;using imported.abcdefhijklmnopqrstubwxy;)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
using imported.abcdefhijklmnopqrstubwxy;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to UsingFormatted, except that every token is on a newline.
TEST(FormatterTests, UsingMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

using
imported
.
abcdefhijklmnopqrstubwxy
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

using imported.abcdefhijklmnopqrstubwxy;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Ensure that an already properly formatted aliased using declaration is not modified by another
// run through the formatter.
TEST(FormatterTests, UsingWithAliasAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

using baz.qux as abcdefghijklmnopqrstuv;
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, UsingWithAliasUnformatted) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

  using    baz.qux as
abcdefghijklmnopqrstuv;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

using baz.qux as abcdefghijklmnopqrstuv;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Test that the aliased using declaration is properly wrapped
TEST(FormatterTests, UsingWithAliasOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

using baz.qux as abcdefghijklmnopqrstuvw;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

using baz.qux
        as abcdefghijklmnopqrstuvw;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// This test's input is semantically identical to UsingWithAliasFormatted.  The only difference is
// that the newlines and unnecessary spaces have been removed.
TEST(FormatterTests, UsingWithAliasMinimalWhitespace) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(library foo.bar;using baz.qux as abcdefghijklmnopqrstuv;)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
using baz.qux as abcdefghijklmnopqrstuv;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Input is identical to UsingWithAliasFormatted, except that every token is on a newline.
TEST(FormatterTests, UsingWithAliasMaximalNewlines) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

using
baz
.
qux
as
abcdefghijklmnopqrstuv
;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

using baz.qux as abcdefghijklmnopqrstuv;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// What happens when we have both an inline and standalone comment surrounding each token?
TEST(FormatterTests, CommentsMaximal) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
// 0
// 0.1
/// 0.2
/// 0.3
library // A
// 1
foo // B
// 2
. // C
// 3
bar // D
// 4
; // E
// 5



// 6
// 6.1


// 7
/// 7.1
/// 7.2
using // F
// 8
baz // G
// 9
as // H
// 10
quz // I
; // 11
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
// 0
// 0.1
/// 0.2
/// 0.3
library // A
        // 1
        foo // B
        // 2
        . // C
        // 3
        bar // D
        // 4
        ; // E
// 5



// 6
// 6.1


// 7
/// 7.1
/// 7.2
using // F
        // 8
        baz // G
        // 9
        as // H
        // 10
        quz // I
        ; // 11
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, CommentsNormalNoop) {
  // ---------------40---------------- |
  std::string noop = R"FIDL(
// C1
library foo.bar; // C2
// C3
using baz.qux; // C4
)FIDL";

  ASSERT_FORMATTED(noop, noop);
}

// Ensure that overlong comments are not wrapped.
TEST(FormatterTests, CommentsOverflowNoop) {
  // ---------------40---------------- |
  std::string noop = R"FIDL(
// C1: This is my very very long comment.
library foo.bar; // C2
// C3: This is my very very long comment.
using baz.qux; // C4
)FIDL";

  ASSERT_FORMATTED(noop, noop);
}

// Regression test for https://fxbug.dev/42059235.
TEST(FormatterTests, CommentsBeforeStatementOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library example;

// Comment
using dependency as import_abcdefghijklm;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example;

// Comment
using dependency
        as import_abcdefghijklm;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, CommentsMultiline) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
// C1a
// C1b
library foo.bar;  // C2

// C3a
// C3b
using baz.qux;  // C4

// C5a
// C5b
resource_definition thing : uint8 {  // C6
// C7a
// C7b
    properties {  // C8
// C9a
// C9b
        stuff rights;  // C10
    };
};

// C11a
// C11b
const MY_CONST string = "abc";  // C12

// C13a
// C13b
type MyEnum = enum {  // C14
// C15a
// C17b
    MY_VALUE = 1;  // C16
};

// C17a
// C17b
type MyTable = resource table {  // C18
// C19a
// C19b
    1: field thing;  // C20
};

// C21a
// C21b
alias MyAlias = MyStruct;  // C22

// C23a
// C23b
protocol MyProtocol {  // C24
// C25a
// C25b
    MyMethod(resource struct {  // C26
// C27a
// C27b
        data MyTable;  // C28
    }) -> () error MyEnum;  // C29
};  // 30

// C29a
// C29b
service MyService {  // C32
// C31a
// C31b
    my_protocol client_end:MyProtocol;  // C34
};  // C35
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
// C1a
// C1b
library foo.bar; // C2

// C3a
// C3b
using baz.qux; // C4

// C5a
// C5b
resource_definition thing : uint8 { // C6
    // C7a
    // C7b
    properties { // C8
        // C9a
        // C9b
        stuff rights; // C10
    };
};

// C11a
// C11b
const MY_CONST string = "abc"; // C12

// C13a
// C13b
type MyEnum = enum { // C14
    // C15a
    // C17b
    MY_VALUE = 1; // C16
};

// C17a
// C17b
type MyTable = resource table { // C18
    // C19a
    // C19b
    1: field thing; // C20
};

// C21a
// C21b
alias MyAlias = MyStruct; // C22

// C23a
// C23b
protocol MyProtocol { // C24
    // C25a
    // C25b
    MyMethod(resource struct { // C26
        // C27a
        // C27b
        data MyTable; // C28
    }) -> () error MyEnum; // C29
}; // 30

// C29a
// C29b
service MyService { // C32
    // C31a
    // C31b
    my_protocol client_end:MyProtocol; // C34
}; // C35
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, CommentsWeird) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
   // C1
     /// D1
/// D2
         /// D3
 @foo( // C2
     "abc"
  // C3
)
library foo.

// C4

        // C5

bar; @attr using // C6

baz;
using qux // C7
;

type // C8
MyStruct = struct

// C9

{ my_field // C10
bool;

// C11


};

   // C12




)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
// C1
/// D1
/// D2
/// D3
@foo( // C2
        "abc"
        // C3
        )
library foo.

        // C4

        // C5

        bar;
@attr
using // C6
        baz;
using qux // C7
;

type // C8
        MyStruct = struct

        // C9

        {
    my_field // C10
    bool;

    // C11


};

// C12
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// TODO(https://fxbug.dev/42169303): This test currently behaves correctly per the specified
// line-wrapping algorithm, but the output is unintuitive and unexpected. Once the referenced bug is
// fixed, this test should result in the `unformatted` input being unmodified.
TEST(FormatterTests, DISABLED_CommentsEmptyLayout) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

MyStruct = struct {
    // Comment
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

MyStruct = struct {
        // Comment
        };
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, DocCommentsMultiline) {
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

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, DocCommentsThenComments) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
/// C1a
// C1b
library foo.bar;  // C2

/// C3a
// C3b
using baz.qux;  // C4

/// C5a
// C5b
resource_definition thing : uint8 {  // C6
    properties {  // C8
/// C9a
// C9b
        stuff rights;  // C10
    };
};

/// C11a
// C11b
const MY_CONST string = "abc";  // C12

/// C13a
// C13b
type MyEnum = enum {  // C14
/// C15a
// C17b
    MY_VALUE = 1;  // C16
};

/// C17a
// C17b
type MyTable = resource table {  // C18
/// C19a
// C19b
    1: field thing;  // C20
};

/// C21a
// C21b
alias MyAlias = MyStruct;  // C22

/// C23a
// C23b
protocol MyProtocol {  // C24
/// C25a
// C25b
    MyMethod(resource struct {  // C26
/// C27a
// C27b
        data MyTable;  // C28
    }) -> () error MyEnum;  // C29
};  // 30

/// C29a
// C29b
service MyService {  // C32
/// C31a
// C31b
    my_protocol client_end:MyProtocol;  // C34
};  // C35
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
/// C1a
// C1b
library foo.bar; // C2

/// C3a
// C3b
using baz.qux; // C4

/// C5a
// C5b
resource_definition thing : uint8 { // C6
    properties { // C8
        /// C9a
        // C9b
        stuff rights; // C10
    };
};

/// C11a
// C11b
const MY_CONST string = "abc"; // C12

/// C13a
// C13b
type MyEnum = enum { // C14
    /// C15a
    // C17b
    MY_VALUE = 1; // C16
};

/// C17a
// C17b
type MyTable = resource table { // C18
    /// C19a
    // C19b
    1: field thing; // C20
};

/// C21a
// C21b
alias MyAlias = MyStruct; // C22

/// C23a
// C23b
protocol MyProtocol { // C24
    /// C25a
    // C25b
    MyMethod(resource struct { // C26
        /// C27a
        // C27b
        data MyTable; // C28
    }) -> () error MyEnum; // C29
}; // 30

/// C29a
// C29b
service MyService { // C32
    /// C31a
    // C31b
    my_protocol client_end:MyProtocol; // C34
}; // C35
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, DocCommentsThenAttributes) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
/// C1a
@attr1
library foo.bar;  // C2

/// C3a
@attr3
using baz.qux;  // C4

/// C5a
@attr5
resource_definition thing : uint8 {  // C6
    properties {  // C8
/// C9a
@attr9
stuff rights; // C10
    };
};

/// C11a
@attr11
const MY_CONST string = "abc";  // C12

/// C13a
@attr13
type MyEnum = enum {  // C14
/// C15a
@attr17
    MY_VALUE = 1;  // C16
};

/// C17a
@attr17
type MyTable = resource table {  // C18
/// C19a
@attr19
    1: field thing;  // C20
};

/// C21a
@attr21
alias MyAlias = MyStruct;  // C22

/// C23a
@attr23
protocol MyProtocol {  // C24
/// C25a
@attr25
    MyMethod(resource struct {  // C26
/// C27a
@attr27
        data MyTable;  // C28
    }) -> () error MyEnum;  // C29
};  // 30

/// C29a
@attr29
service MyService {  // C32
/// C31a
@attr31
    my_protocol client_end:MyProtocol;  // C34
};  // C35
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
/// C1a
@attr1
library foo.bar; // C2

/// C3a
@attr3
using baz.qux; // C4

/// C5a
@attr5
resource_definition thing : uint8 { // C6
    properties { // C8
        /// C9a
        @attr9
        stuff rights; // C10
    };
};

/// C11a
@attr11
const MY_CONST string = "abc"; // C12

/// C13a
@attr13
type MyEnum = enum { // C14
    /// C15a
    @attr17
    MY_VALUE = 1; // C16
};

/// C17a
@attr17
type MyTable = resource table { // C18
    /// C19a
    @attr19
    1: field thing; // C20
};

/// C21a
@attr21
alias MyAlias = MyStruct; // C22

/// C23a
@attr23
protocol MyProtocol { // C24
    /// C25a
    @attr25
    MyMethod(resource struct { // C26
        /// C27a
        @attr27
        data MyTable; // C28
    }) -> () error MyEnum; // C29
}; // 30

/// C29a
@attr29
service MyService { // C32
    /// C31a
    @attr31
    my_protocol client_end:MyProtocol; // C34
}; // C35
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, DocCommentsThenAttributesThenInlineComments) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
/// C1a
@attr1  // C1b
library foo.bar;  // C2

/// C3a
@attr3  // C3b
using baz.qux;  // C4

/// C5a
@attr5  // C5b
resource_definition thing : uint8 {  // C6
    properties {  // C8
/// C9a
@attr9  // C9b
        stuff rights;  // C10
    };
};

/// C11a
@attr11  // C11b
const MY_CONST string = "abc";  // C12

/// C13a
@attr13  // C13b
type MyEnum = enum {  // C14
/// C15a
@attr17  // C17b
    MY_VALUE = 1;  // C16
};

/// C17a
@attr17  // C17b
type MyTable = resource table {  // C18
/// C19a
@attr19  // C19b
    1: field thing;  // C20
};

/// C21a
@attr21  // C21b
alias MyAlias = MyStruct;  // C22

/// C23a
@attr23  // C23b
protocol MyProtocol {  // C24
/// C25a
@attr25  // C25b
    MyMethod(resource struct {  // C26
/// C27a
@attr27  // C27b
        data MyTable;  // C28
    }) -> () error MyEnum;  // C29
};  // 30

/// C29a
@attr29  // C29b
service MyService {  // C32
/// C31a
@attr31  // C31b
    my_protocol client_end:MyProtocol;  // C34
};  // C35
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
/// C1a
@attr1 // C1b
library foo.bar; // C2

/// C3a
@attr3 // C3b
using baz.qux; // C4

/// C5a
@attr5 // C5b
resource_definition thing : uint8 { // C6
    properties { // C8
        /// C9a
        @attr9 // C9b
        stuff rights; // C10
    };
};

/// C11a
@attr11 // C11b
const MY_CONST string = "abc"; // C12

/// C13a
@attr13 // C13b
type MyEnum = enum { // C14
    /// C15a
    @attr17 // C17b
    MY_VALUE = 1; // C16
};

/// C17a
@attr17 // C17b
type MyTable = resource table { // C18
    /// C19a
    @attr19 // C19b
    1: field thing; // C20
};

/// C21a
@attr21 // C21b
alias MyAlias = MyStruct; // C22

/// C23a
@attr23 // C23b
protocol MyProtocol { // C24
    /// C25a
    @attr25 // C25b
    MyMethod(resource struct { // C26
        /// C27a
        @attr27 // C27b
        data MyTable; // C28
    }) -> () error MyEnum; // C29
}; // 30

/// C29a
@attr29 // C29b
service MyService { // C32
    /// C31a
    @attr31 // C31b
    my_protocol client_end:MyProtocol; // C34
}; // C35
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, DocCommentsThenAttributesThenStandaloneComments) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
/// C1a
@attr1
// C1b
library foo.bar;  // C2

/// C3a
@attr3
// C3b
using baz.qux;  // C4

/// C5a
@attr5
// C5b
resource_definition thing : uint8 {  // C6
    properties {  // C8
/// C9a
@attr9
// C9b
        stuff rights;  // C10
    };
};

/// C11a
@attr11
// C11b
const MY_CONST string = "abc";  // C12

/// C13a
@attr13
// C13b
type MyEnum = enum {  // C14
/// C15a
@attr17
// C17b
    MY_VALUE = 1;  // C16
};

/// C17a
@attr17
// C17b
type MyTable = resource table {  // C18
/// C19a
@attr19
// C19b
    1: field thing;  // C20
};

/// C21a
@attr21
// C21b
alias MyAlias = MyStruct;  // C22

/// C23a
@attr23
// C23b
protocol MyProtocol {  // C24
/// C25a
@attr25
// C25b
    MyMethod(resource struct {  // C26
/// C27a
@attr27
// C27b
        data MyTable;  // C28
    }) -> () error MyEnum;  // C29
};  // 30

/// C29a
@attr29
// C29b
service MyService {  // C32
/// C31a
@attr31
// C31b
    my_protocol client_end:MyProtocol;  // C34
};  // C35
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
/// C1a
@attr1
// C1b
library foo.bar; // C2

/// C3a
@attr3
// C3b
using baz.qux; // C4

/// C5a
@attr5
// C5b
resource_definition thing : uint8 { // C6
    properties { // C8
        /// C9a
        @attr9
        // C9b
        stuff rights; // C10
    };
};

/// C11a
@attr11
// C11b
const MY_CONST string = "abc"; // C12

/// C13a
@attr13
// C13b
type MyEnum = enum { // C14
    /// C15a
    @attr17
    // C17b
    MY_VALUE = 1; // C16
};

/// C17a
@attr17
// C17b
type MyTable = resource table { // C18
    /// C19a
    @attr19
    // C19b
    1: field thing; // C20
};

/// C21a
@attr21
// C21b
alias MyAlias = MyStruct; // C22

/// C23a
@attr23
// C23b
protocol MyProtocol { // C24
    /// C25a
    @attr25
    // C25b
    MyMethod(resource struct { // C26
        /// C27a
        @attr27
        // C27b
        data MyTable; // C28
    }) -> () error MyEnum; // C29
}; // 30

/// C29a
@attr29
// C29b
service MyService { // C32
    /// C31a
    @attr31
    // C31b
    my_protocol client_end:MyProtocol; // C34
}; // C35
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, NewlinesAbsent) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;
// comment
using imported.abcdefhijklmnopqrstubwxy;
/// doc comment
alias MyAlias_Abcdefghijklmnopqr = bool;
@foo
@bar
const MY_TRUE_ABCDEFGHIJKLM bool = true;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;
// comment
using imported.abcdefhijklmnopqrstubwxy;
/// doc comment
alias MyAlias_Abcdefghijklmnopqr = bool;
@foo
@bar
const MY_TRUE_ABCDEFGHIJKLM bool = true;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// For this test and the one below, new lines are generally expected to be retained.  An exception
// is made for doc comment and attribute blocks, which must never have newlines between the
// respective attributes, or between the last attribute and the declaration the block is describing.
TEST(FormatterTests, NewlinesSingle) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

// comment

using imported.abcdefhijklmnopqrstubwxy;

/// doc comment

alias MyAlias_Abcdefghijklmnopqr = bool;

@foo

@bar

const MY_TRUE_ABCDEFGHIJKLM bool = true;

)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

// comment

using imported.abcdefhijklmnopqrstubwxy;

/// doc comment
alias MyAlias_Abcdefghijklmnopqr = bool;

@foo
@bar
const MY_TRUE_ABCDEFGHIJKLM bool = true;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, NewlinesDouble) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(

library foo.bar;


// comment


using imported.abcdefhijklmnopqrstubwxy;


/// doc comment


alias MyAlias_Abcdefghijklmnopqr = bool;


@foo


@bar


const MY_TRUE_ABCDEFGHIJKLM bool = true;


)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;


// comment


using imported.abcdefhijklmnopqrstubwxy;


/// doc comment
alias MyAlias_Abcdefghijklmnopqr = bool;


@foo
@bar
const MY_TRUE_ABCDEFGHIJKLM bool = true;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, ListSpacing) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

const RIGHTS_BASIC rights = rights.TRANSFER|rights.DUPLICATE|rights.WAIT|rights.INSPECT;
alias constrained_handle = zx.Handle:<VMO,RIGHTS_BASIC>;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

const RIGHTS_BASIC rights
        = rights.TRANSFER | rights.DUPLICATE | rights.WAIT | rights.INSPECT;
alias constrained_handle
        = zx.Handle:<VMO, RIGHTS_BASIC>;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Regression test for https://fxbug.dev/42163023.
TEST(FormatterTests, DocCommentThenCommentThenChildComment) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

/// Doc comment.
// Outer comment.
type MyEnum = strict enum : uint16 {
            // Inner comment.
    MEMBER = 0;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

/// Doc comment.
// Outer comment.
type MyEnum = strict enum : uint16 {
    // Inner comment.
    MEMBER = 0;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, InlineAttribute) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

protocol Foo {
  Bar(@foo struct {});
  Baz(@bar struct { data uint8; }) -> (@baz @qux struct { data uint8; });
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

protocol Foo {
    Bar(@foo struct {});
    Baz(@bar struct {
        data uint8;
    }) -> (@baz @qux struct {
        data uint8;
    });
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, VectorWithInlineAttributeAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyTable = struct {
    anon vector<@foo("bar") table {
        1: inner bool;
    }>:123;
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

TEST(FormatterTests, VectorWithInlineAttributeOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyTable = struct {
    anon_abcd vector< @foo("bar") table {
        1: inner_abcdefghijklmnopqr bool;
    }>:123;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyTable = struct {
    anon_abcd
            vector<@foo("bar") table {
        1: inner_abcdefghijklmnopqr
                bool;
    }>:123;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Don't wrap if <8 chars have been used before the wrapping, as this will cause greater offsetting
// with no readability benefit.  For example:
//
//     foo zx.Handle:<VMO, RIGHT_A | RIGHT_B>;
//
// would otherwise get divided into:
//
//     foo
//             zx.Handle:<VMO, RIGHT_A | RIGHT_B>;
//
// which looks and reads strictly worse.
TEST(FormatterTests, NoPointlessWrapping) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

type MyStruct = resource struct {
    lilname zx.Handle:<VMO, RIGHT_A | RIGHT_B>;
    longname zx.Handle:<VMO, RIGHT_A | RIGHT_B>;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

type MyStruct = resource struct {
    lilname zx.Handle:<VMO, RIGHT_A | RIGHT_B>;
    longname
            zx.Handle:<VMO, RIGHT_A | RIGHT_B>;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, ProtocolModifierDoesntWrap) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

open protocol FooBarBazQuixLongNameSomething {
};

ajar protocol FooBarBazQuixLongNameSomething {
};

closed protocol FooBarBazQuixLongNameSomething {
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

open protocol FooBarBazQuixLongNameSomething {};

ajar protocol FooBarBazQuixLongNameSomething {};

closed protocol FooBarBazQuixLongNameSomething {};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, ProtocolModifierDoesntPreventContentsWrapping) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

open protocol FooBarBazQuixLongNameSomething { Test(); };

ajar protocol FooBarBazQuixLongNameSomething { Test(); };

closed protocol FooBarBazQuixLongNameSomething { Test(); };
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

open protocol FooBarBazQuixLongNameSomething {
    Test();
};

ajar protocol FooBarBazQuixLongNameSomething {
    Test();
};

closed protocol FooBarBazQuixLongNameSomething {
    Test();
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, MethodModifierDoesntWrapNoop) {
  // ---------------40---------------- |
  std::string noop = R"FIDL(
library foo.bar;

open protocol Test {
    strict FooBarBazQuixLongNameSomething();
    flexible BazFooQuixBarLongNameSomething();
    strict LongNameSomethingFooBarBazQuix() -> ();
    flexible LongNameSomethingBazFooQuixBar() -> ();
};
)FIDL";

  ASSERT_FORMATTED(noop, noop);
}

TEST(FormatterTests, MethodModifierDoesntPreventContentsWrapping) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

open protocol Test {
    strict FooBarBazQuixLongNameSomething(struct { x int32; });
    flexible BazFooQuixBarLongNameSomething(struct { x int32; });
    strict LongNameSomethingFooBarBazQuix(struct { x int32; }) -> (struct { a int32; b int64; c int32; d int64; });
    flexible LongNameSomethingBazFooQuixBar(struct { x int32; }) -> (struct { a int32; b int64; c int32; d int64; });
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

open protocol Test {
    strict FooBarBazQuixLongNameSomething(struct {
        x int32;
    });
    flexible BazFooQuixBarLongNameSomething(struct {
        x int32;
    });
    strict LongNameSomethingFooBarBazQuix(struct {
        x int32;
    }) -> (struct {
        a int32;
        b int64;
        c int32;
        d int64;
    });
    flexible LongNameSomethingBazFooQuixBar(struct {
        x int32;
    }) -> (struct {
        a int32;
        b int64;
        c int32;
        d int64;
    });
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

TEST(FormatterTests, MethodModifierDoesntPreventProtocolWrapping) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library foo.bar;

open protocol Test {
    strict FooBarBazQuixLongNameSomething(); flexible BazFooQuixBarLongNameSomething(); strict LongNameSomethingFooBarBazQuix() -> (); flexible LongNameSomethingBazFooQuixBar() -> ();
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library foo.bar;

open protocol Test {
    strict FooBarBazQuixLongNameSomething();
    flexible BazFooQuixBarLongNameSomething();
    strict LongNameSomethingFooBarBazQuix() -> ();
    flexible LongNameSomethingBazFooQuixBar() -> ();
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Regression test for https://fxbug.dev/42172119.
TEST(FormatterTests, AliasAnonymousLayoutInLayoutParameterAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example;

alias MyVec = vector<struct {
    inner bool;
}>;
alias MyEmptyVec = vector<struct {}>;
alias MyArr = array<struct {
    inner bool;
}, 1>;
alias MyEmptyArr = array<struct {}, 1>;
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

// Regression test for https://fxbug.dev/42172119.
TEST(FormatterTests, AliasAnonymousLayoutInLayoutParameterOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library example;

alias MyVec_abcdefghijk = vector<struct {
    inner bool;
}>;
alias MyEmptyVec_abc = vector<struct {}>;
alias MyArr_abcdefghijkl = array<struct {
    inner bool;
}, 1>;
alias MyEmptyArr_a = array<struct {}, 1>;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example;

alias MyVec_abcdefghijk
        = vector<struct {
    inner bool;
}>;
alias MyEmptyVec_abc
        = vector<struct {}>;
alias MyArr_abcdefghijkl
        = array<struct {
    inner bool;
}, 1>;
alias MyEmptyArr_a
        = array<struct {}, 1>;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Regression test for https://fxbug.dev/42172119.
TEST(FormatterTests, LayoutMemberAnonymousLayoutInLayoutParameterAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example;

type MyStruct = struct {
    box box<struct {
        inner bool;
    }>;
};
type MyTable = table {
    1: vec vector<table {
        1: inner bool;
    }>;
};
type MyUnion = union {
    1: arr array<flexible union {
        1: inner bool;
    }, 1>;
};

type MyEmptyStruct = struct {
    box box<struct {}>;
};
type MyEmptyTable = table {
    1: arr array<table {}, 1>;
};
type MyEmptyUnion = union {
    1: vec vector<flexible union {}>;
};
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

// Regression test for https://fxbug.dev/42172119.
TEST(FormatterTests, LayoutMemberAnonymousLayoutInLayoutParameterOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library example;

type MyStruct = struct {
    box_abcdefghijklmnopqrst box<struct {
        inner bool;
    }>;
};
type MyTable = table {
    1: vec_abcdefghijklmno vector<table {
        1: inner bool;
    }>;
};
type MyUnion = union {
    1: arr_abcdefg array<flexible union {
        1: inner bool;
    }, 1>;
};

type MyEmptyStruct = struct {
    box_abcdefghijklmnopq box<struct {}>;
};
type MyEmptyTable = table {
    1: arr_abcdefghij array<table {}, 1>;
};
type MyEmptyUnion = union {
    1: vec_abc vector<flexible union {}>;
};
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example;

type MyStruct = struct {
    box_abcdefghijklmnopqrst
            box<struct {
        inner bool;
    }>;
};
type MyTable = table {
    1: vec_abcdefghijklmno
            vector<table {
        1: inner bool;
    }>;
};
type MyUnion = union {
    1: arr_abcdefg
            array<flexible union {
        1: inner bool;
    }, 1>;
};

type MyEmptyStruct = struct {
    box_abcdefghijklmnopq
            box<struct {}>;
};
type MyEmptyTable = table {
    1: arr_abcdefghij
            array<table {}, 1>;
};
type MyEmptyUnion = union {
    1: vec_abc
            vector<flexible union {}>;
};
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Regression test for https://fxbug.dev/42064676.
TEST(FormatterTests, ConstraintOnInlineAnonymousLayoutNormal) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library example;

type MyStruct = struct {
    member flexible union {}:optional;
}:optional;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example;

type MyStruct = struct {
    member flexible union {}:optional;
}:optional;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Regression test for https://fxbug.dev/42064676.
TEST(FormatterTests, ConstraintOnInlineAnonymousLayoutOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library example;

type MyStruct = struct {
    member_ab flexible union {}:optional;
}:optional;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example;

type MyStruct = struct {
    member_ab
            flexible union {}:optional;
}:optional;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

// Regression test for https://fxbug.dev/42063857.
TEST(FormatterTests, InlineCommentTrailingNewlineAlreadyFormattedNoop) {
  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example; // C1

alias MyAlias = uint8;
)FIDL";

  ASSERT_FORMATTED(formatted, formatted);
}

// Regression test for https://fxbug.dev/42063857.
TEST(FormatterTests, InlineCommentTrailingNewlineOverflow) {
  // ---------------40---------------- |
  std::string unformatted = R"FIDL(
library example; // C1

alias MyAlias_abcdefghijklmnopq = uint64; // C2


alias MyOtherAlias = uint8;
)FIDL";

  // ---------------40---------------- |
  std::string formatted = R"FIDL(
library example; // C1

alias MyAlias_abcdefghijklmnopq
        = uint64; // C2


alias MyOtherAlias = uint8;
)FIDL";

  ASSERT_FORMATTED(unformatted, formatted);
}

}  // namespace
}  // namespace fidlc
