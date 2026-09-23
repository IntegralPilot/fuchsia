// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <sstream>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

template <class PrimitiveType>
void CheckConstEq(TestLibrary& library, std::string_view name, PrimitiveType expected_value,
                  Constant::Kind expected_constant_kind,
                  ConstantValue::Kind expected_constant_value_kind) {
  auto const_decl = library.LookupConstant(name);
  ASSERT_NE(const_decl, nullptr);
  ASSERT_EQ(expected_constant_kind, const_decl->value->kind);
  ASSERT_EQ(expected_constant_value_kind, const_decl->value->Value().kind);
  auto& numeric_const_value =
      static_cast<const NumericConstantValue<PrimitiveType>&>(const_decl->value->Value());
  EXPECT_EQ(expected_value, numeric_const_value.value);
}

TEST(ConstsTests, GoodLiteralsTest) {
  TestLibrary library(R"FIDL(
library example;

const C_SIMPLE uint32 = 11259375;
const C_HEX_S uint32 = 0xABCDEF;
const C_HEX_L uint32 = 0XABCDEF;
const C_BINARY_S uint32 = 0b101010111100110111101111;
const C_BINARY_L uint32 = 0B101010111100110111101111;
)FIDL");
  ASSERT_COMPILED(library);

  auto check_const_eq = [](TestLibrary& library, std::string_view name, uint32_t expected_value) {
    CheckConstEq<uint32_t>(library, name, expected_value, Constant::Kind::kLiteral,
                           ConstantValue::Kind::kUint32);
  };

  check_const_eq(library, "C_SIMPLE", 11259375);
  check_const_eq(library, "C_HEX_S", 11259375);
  check_const_eq(library, "C_HEX_L", 11259375);
  check_const_eq(library, "C_BINARY_S", 11259375);
  check_const_eq(library, "C_BINARY_L", 11259375);
}

TEST(ConstsTests, GoodConstTestBool) {
  TestLibrary library(R"FIDL(
library example;

const c bool = false;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadConstTestBoolWithString) {
  TestLibrary library;
  library.AddFile("bad/fi-0065-a.test.fidl");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"foo\"", "string:3", "bool");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestBoolWithNumeric) {
  TestLibrary library(R"FIDL(
library example;

const c bool = 6;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "6", "untyped numeric", "bool");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodConstTestInt32) {
  TestLibrary library(R"FIDL(
library example;

const c int32 = 42;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodConstTestInt32FromOtherConst) {
  TestLibrary library(R"FIDL(
library example;

const b int32 = 42;
const c int32 = b;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadConstTestInt32WithString) {
  TestLibrary library(R"FIDL(
library example;

const c int32 = "foo";
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"foo\"", "string:3", "int32");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestInt32WithBool) {
  TestLibrary library(R"FIDL(
library example;

const c int32 = true;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "true", "bool", "int32");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodConstTestInt64) {
  TestLibrary library;
  library.AddFile("good/fi-0066-b.test.fidl");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodConstTestUint64) {
  TestLibrary library;
  library.AddFile("good/fi-0066-a.test.fidl");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodConstTestUint64FromOtherUint32) {
  TestLibrary library(R"FIDL(
library example;

const a uint32 = 42;
const b uint64 = a;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadConstTestUint64Negative) {
  TestLibrary library;
  library.AddFile("bad/fi-0066.test.fidl");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrConstantOverflowsType, "-42", "uint64");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestUint64Overflow) {
  TestLibrary library(R"FIDL(
library example;

const a uint64 = 18446744073709551616;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrConstantOverflowsType, "18446744073709551616", "uint64");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodConstTestFloat32) {
  TestLibrary library(R"FIDL(
library example;

const b float32 = 1.61803;
const c float32 = -36.46216;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodConstTestFloat32HighLimit) {
  TestLibrary library(R"FIDL(
library example;

const hi float32 = 3.402823e38;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodConstTestFloat32LowLimit) {
  TestLibrary library(R"FIDL(
library example;

const lo float32 = -3.40282e38;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadConstTestFloat32HighLimit) {
  TestLibrary library(R"FIDL(
library example;

const hi float32 = 3.41e38;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrConstantOverflowsType, "3.41e38", "float32");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestFloat32LowLimit) {
  TestLibrary library(R"FIDL(
library example;

const b float32 = -3.41e38;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrConstantOverflowsType, "-3.41e38", "float32");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodConstTestString) {
  TestLibrary library;
  library.AddFile("good/fi-0002.test.fidl");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodConstTestStringFromOtherConst) {
  TestLibrary library(R"FIDL(
library example;

const c string:4 = "four";
const d string:5 = c;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadConstTestStringWithNumeric) {
  TestLibrary library(R"FIDL(
library example;

const c string = 4;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "4", "untyped numeric", "string");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestStringWithBool) {
  TestLibrary library(R"FIDL(
library example;

const c string = true;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "true", "bool", "string");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestStringWithStringTooLong) {
  TestLibrary library(R"FIDL(
library example;

const c string:4 = "hello";
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"hello\"", "string:5", "string:4");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodConstTestUsing) {
  TestLibrary library(R"FIDL(
library example;

alias foo = int32;
const c foo = 2;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadConstTestUsingWithInconvertibleValue) {
  TestLibrary library(R"FIDL(
library example;

alias foo = int32;
const c foo = "nope";
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"nope\"", "string:4", "int32");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestNullableString) {
  TestLibrary library;
  library.AddFile("bad/fi-0059.test.fidl");
  library.ExpectFail(ErrInvalidConstantType, "string?");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestArray) {
  TestLibrary library(R"FIDL(
library example;

const c array<int32,2> = -1;
)FIDL");
  library.ExpectFail(ErrInvalidConstantType, "array<int32, 2>");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestVector) {
  TestLibrary library(R"FIDL(
library example;

const c vector<int32>:2 = -1;
)FIDL");
  library.ExpectFail(ErrInvalidConstantType, "vector<int32>:2");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestHandleOfThread) {
  TestLibrary library(R"FIDL(
library example;

type ObjType = enum : uint32 {
    NONE = 0;
    THREAD = 2;
};

resource_definition handle : uint32 {
    properties {
        subtype ObjType;
    };
};

const c handle:THREAD = -1;
)FIDL");
  library.ExpectFail(ErrInvalidConstantType, "example/handle:thread");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodConstEnumMemberReference) {
  TestLibrary library(R"FIDL(
library example;

type MyEnum = strict enum : int32 {
    A = 5;
};
const c int32 = MyEnum.A;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodConstBitsMemberReference) {
  TestLibrary library(R"FIDL(
library example;

type MyBits = strict bits : uint32 {
    A = 0x00000001;
};
const c uint32 = MyBits.A;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodEnumTypedConstEnumMemberReference) {
  TestLibrary library(R"FIDL(
library example;

type MyEnum = strict enum : int32 {
    A = 5;
};
const c MyEnum = MyEnum.A;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodBitsTypedConstBitsMemberReference) {
  TestLibrary library(R"FIDL(
library example;

type MyBits = strict bits : uint32 {
    A = 0x00000001;
};
const c MyBits = MyBits.A;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodBitsTypedConstZero) {
  TestLibrary library(R"FIDL(
library example;

type MyBits = strict bits : uint32 {
    A = 0x00000001;
};
const c MyBits = 0;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadConstDifferentEnumMemberReference) {
  TestLibrary library;
  library.AddFile("bad/fi-0064.test.fidl");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrMismatchedNameTypeAssignment, "MyEnum", "OtherEnum");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstDifferentBitsMemberReference) {
  TestLibrary library(R"FIDL(
library example;

type MyBits = bits : uint32 { VALUE = 0x00000001; };
type OtherBits = bits : uint32 { VALUE = 0x00000004; };
const c MyBits = OtherBits.VALUE;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrMismatchedNameTypeAssignment, "MyBits", "OtherBits");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstAssignPrimitiveToEnum) {
  TestLibrary library(R"FIDL(
library example;

type MyEnum = enum : int32 { VALUE = 1; };
const c MyEnum = 5;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "5", "untyped numeric", "example/MyEnum");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstAssignPrimitiveToBits) {
  TestLibrary library(R"FIDL(
library example;

type MyBits = bits : uint32 { VALUE = 0x00000001; };
const c MyBits = 5;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "5", "untyped numeric", "example/MyBits");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodMaxBoundTest) {
  TestLibrary library(R"FIDL(
library example;

const S string:MAX = "";

type Example = struct {
    s string:MAX;
    v vector<bool>:MAX;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodMaxBoundTestConvertToUnbounded) {
  TestLibrary library(R"FIDL(
library example;

const A string:MAX = "foo";
const B string = A;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodMaxBoundTestConvertFromUnbounded) {
  TestLibrary library(R"FIDL(
library example;

const A string = "foo";
const B string:MAX = A;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadMaxBoundTestAssignToConst) {
  TestLibrary library(R"FIDL(
library example;

const FOO uint32 = MAX;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadMaxBoundTestLibraryQualified) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependency.fidl", R"FIDL(
library dependency;

type Example = struct {};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using dependency;

type Example = struct { s string:dependency.MAX; };
)FIDL");
  library.ExpectFail(ErrNameNotFound, "MAX", "library 'dependency'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadParameterizePrimitive) {
  TestLibrary library(R"FIDL(
library example;

const u uint8<string> = 0;
)FIDL");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "uint8", 0, 1);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestAssignTypeSimple) {
  TestLibrary library;
  library.AddFile("bad/fi-0063.test.fidl");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrExpectedValueButGotType, "MyType");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestAssignTypeName) {
  for (auto type_declaration : {
           "type Example = struct {};",
           "type Example = table {};",
           "service Example {};",
           "protocol Example {};",
           "type Example = bits { A = 1; };",
           "type Example = enum { A = 1; };",
           "type Example = union { 1: A bool; };",
           "alias Example = string;",
       }) {
    std::ostringstream s;
    s << "library example;\n" << type_declaration << "\nconst FOO uint32 = Example;";
    auto fidl = s.str();
    SCOPED_TRACE(fidl);
    TestLibrary library(fidl);
    library.ExpectFail(ErrCannotResolveConstantValue);
    library.ExpectFail(ErrExpectedValueButGotType, "Example");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ConstsTests, BadConstTestAssignBuiltinSimple) {
  TestLibrary library;
  library.AddFile("bad/fi-0060.test.fidl");
  library.ExpectFail(ErrCannotResolveConstantValue);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstTestAssignBuiltinType) {
  for (auto builtin : {"bool", "uint32", "box", "vector", "byte"}) {
    std::ostringstream ss;
    ss << "library example;\n";
    ss << "const FOO uint32 = " << builtin << ";\n";

    TestLibrary library(ss.str());
    // TODO(https://fxbug.dev/42182133): Should have a better error message.
    library.ExpectFail(ErrCannotResolveConstantValue);
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ConstsTests, BadConstTestAssignBuiltinNonType) {
  for (auto builtin : {"MAX", "optional", "NEXT", "HEAD"}) {
    std::ostringstream ss;
    ss << "library example;\n";
    ss << "const FOO uint32 = " << builtin << ";\n";

    TestLibrary library(ss.str());
    // TODO(https://fxbug.dev/42182133): Should have a better error message.
    library.ExpectFail(ErrCannotResolveConstantValue);
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ConstsTests, BadNameCollision) {
  TestLibrary library;
  library.AddFile("bad/fi-0034.test.fidl");
  library.ExpectFail(ErrNameCollision, Element::Kind::kConst, "COLOR", Element::Kind::kConst,
                     "bad/fi-0034.test.fidl:6:7");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodFixNameCollisionRename) {
  TestLibrary library;
  library.AddFile("good/fi-0034-b.test.fidl");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodFixNameCollisionRemove) {
  TestLibrary library;
  library.AddFile("good/fi-0034-a.test.fidl");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodMultiFileConstReference) {
  TestLibrary library;
  library.AddSource("first.fidl", R"FIDL(
library example;

type Protein = struct {
    amino_acids vector<uint64>:SMALL_SIZE;
};
)FIDL");
  library.AddSource("second.fidl", R"FIDL(
library example;

const SMALL_SIZE uint32 = 4;
)FIDL");

  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadUnknownEnumMemberTest) {
  TestLibrary library(R"FIDL(
library example;

type EnumType = enum : int32 {
    A = 0x00000001;
    B = 0x80;
    C = 0x2;
};

const dee EnumType = EnumType.D;
)FIDL");
  library.ExpectFail(ErrMemberNotFound, "enum 'EnumType'", "D");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadUnknownBitsMemberTest) {
  TestLibrary library(R"FIDL(
library example;

type BitsType = bits {
    A = 2;
    B = 4;
    C = 8;
};

const dee BitsType = BitsType.D;
)FIDL");
  library.ExpectFail(ErrMemberNotFound, "bits 'BitsType'", "D");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodOrOperatorTest) {
  TestLibrary library(R"FIDL(
library example;

type MyBits = strict bits : uint8 {
    A = 0x00000001;
    B = 0x00000002;
    C = 0x00000004;
    D = 0x00000008;
};
const bitsValue MyBits = MyBits.A | MyBits.B | MyBits.D;
const Result uint16 = MyBits.A | MyBits.B | MyBits.D;
)FIDL");
  ASSERT_COMPILED(library);

  CheckConstEq<uint16_t>(library, "Result", 11, Constant::Kind::kBinaryOperator,
                         ConstantValue::Kind::kUint16);
}

TEST(ConstsTests, BadOrOperatorDifferentTypesTest) {
  TestLibrary library;
  library.AddFile("bad/fi-0065-b.test.fidl");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "test.bad.fi0065b/TWO_FIFTY_SIX", "uint16",
                     "uint8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodOrOperatorDifferentTypesTest) {
  TestLibrary library(R"FIDL(
library example;

const one uint8 = 0x0001;
const two_fifty_six uint16 = 0x0100;
const two_fifty_seven uint16 = one | two_fifty_six;
)FIDL");
  ASSERT_COMPILED(library);

  CheckConstEq<uint16_t>(library, "two_fifty_seven", 257, Constant::Kind::kBinaryOperator,
                         ConstantValue::Kind::kUint16);
}

TEST(ConstsTests, BadOrOperatorNonPrimitiveTypesTest) {
  TestLibrary library;
  library.AddFile("bad/fi-0061.test.fidl");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrOrOperatorOnNonPrimitiveValue);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodOrOperatorParenthesesTest) {
  TestLibrary library(R"FIDL(
library example;

type MyBits = strict bits : uint8 {
    A = 0x00000001;
    B = 0x00000002;
    C = 0x00000004;
    D = 0x00000008;
};
const three MyBits = MyBits.A | MyBits.B;
const seven MyBits = three | MyBits.C;
const fifteen MyBits = (three | seven) | MyBits.D;
const bitsValue MyBits = MyBits.A | ( ( (MyBits.A | MyBits.B) | MyBits.D) | MyBits.C);
)FIDL");
  ASSERT_COMPILED(library);

  CheckConstEq<uint8_t>(library, "three", 3, Constant::Kind::kBinaryOperator,
                        ConstantValue::Kind::kUint8);
  CheckConstEq<uint8_t>(library, "seven", 7, Constant::Kind::kBinaryOperator,
                        ConstantValue::Kind::kUint8);
  CheckConstEq<uint8_t>(library, "fifteen", 15, Constant::Kind::kBinaryOperator,
                        ConstantValue::Kind::kUint8);
  CheckConstEq<uint8_t>(library, "bitsValue", 15, Constant::Kind::kBinaryOperator,
                        ConstantValue::Kind::kUint8);
}

TEST(ConstsTests, BadOrOperatorMissingRightParenTest) {
  TestLibrary library = TestLibrary(R"FIDL(
library example;

const three uint16 = 3;
const seven uint16 = 7;
const eight uint16 = 8;
const fifteen uint16 = ( three | seven | eight;
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kSemicolon),
                     Token::KindAndSubkind(Token::Kind::kRightParen));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadOrOperatorMissingLeftParenTest) {
  TestLibrary library = TestLibrary(R"FIDL(
library example;

const three uint16 = 3;
const seven uint16 = 7;
const eight uint16 = 8;
const fifteen uint16 = three | seven | eight );
)FIDL");
  library.ExpectFail(ErrExpectedDeclaration, ")");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kRightParen),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadOrOperatorMisplacedParenTest) {
  TestLibrary library = TestLibrary(R"FIDL(
library example;

const three uint16 = 3;
const seven uint16 = 7;
const eight uint16 = 8;
const fifteen uint16 = ( three | seven | ) eight;
)FIDL");
  library.ExpectFail(ErrUnexpectedToken);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadIdentifierConstMismatchedTypesTest) {
  TestLibrary library(R"FIDL(
library example;

type OneEnum = enum {
    A = 1;
};
type AnotherEnum = enum {
    B = 1;
};
const a OneEnum = OneEnum.A;
const b AnotherEnum = a;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrMismatchedNameTypeAssignment, "AnotherEnum", "OneEnum");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadEnumBitsConstMismatchedTypesTest) {
  TestLibrary library(R"FIDL(
library example;

type OneEnum = enum {
    A = 1;
};
type AnotherEnum = enum {
    B = 1;
};
const a OneEnum = AnotherEnum.B;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrMismatchedNameTypeAssignment, "OneEnum", "AnotherEnum");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConstReferencesInvalidConst) {
  // Test all orderings since this previously crashed only when the invalid
  // const (set to 1 instead of a string) was lexicographically smaller.
  {
    TestLibrary library(R"FIDL(
library example;
const A string = Z;
const Z string = 1;
)FIDL");
    library.ExpectFail(ErrCannotResolveConstantValue);
    library.ExpectFail(ErrCannotResolveConstantValue);
    library.ExpectFail(ErrTypeCannotBeConvertedToType, "1", "untyped numeric", "string");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
  {
    TestLibrary library(R"FIDL(
library example;
const A string = 1;
const Z string = A;
)FIDL");
    library.ExpectFail(ErrCannotResolveConstantValue);
    library.ExpectFail(ErrTypeCannotBeConvertedToType, "1", "untyped numeric", "string");
    library.ExpectFail(ErrCannotResolveConstantValue);
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
  {
    TestLibrary library(R"FIDL(
library example;
const Z string = A;
const A string = 1;
)FIDL");
    library.ExpectFail(ErrCannotResolveConstantValue);
    library.ExpectFail(ErrCannotResolveConstantValue);
    library.ExpectFail(ErrTypeCannotBeConvertedToType, "1", "untyped numeric", "string");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
  {
    TestLibrary library(R"FIDL(
library example;
const Z string = 1;
const A string = Z;
)FIDL");
    library.ExpectFail(ErrCannotResolveConstantValue);
    library.ExpectFail(ErrTypeCannotBeConvertedToType, "1", "untyped numeric", "string");
    library.ExpectFail(ErrCannotResolveConstantValue);
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ConstsTests, GoodDeclaration) {
  TestLibrary library;
  library.AddFile("good/fi-0006.test.fidl");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodIntegerConvertWider) {
  TestLibrary library(R"FIDL(
library example;
const X uint16 = 23;
const WIDE uint32 = X;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodIntegerConvertNarrower) {
  TestLibrary library(R"FIDL(
library example;
const X uint16 = 255;
const NARROW uint8 = X;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodIntegerConvertToSigned) {
  TestLibrary library(R"FIDL(
library example;
const X uint16 = 23;
const SIGNED int16 = X;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodIntegerConvertToUnsigned) {
  TestLibrary library(R"FIDL(
library example;
const X int16 = 23;
const UNSIGNED uint16 = X;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadIntegerConvertNarrowerOutOfRange) {
  TestLibrary library(R"FIDL(
library example;
const X uint16 = 256;
const NARROW uint8 = X;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/X", "uint16", "uint8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadIntegerConvertToSignedOutOfRange) {
  TestLibrary library(R"FIDL(
library example;
const X uint16 = 32768; // 2^15
const SIGNED int16 = X;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/X", "uint16", "int16");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadIntegerConvertToUnsignedNegative) {
  TestLibrary library(R"FIDL(
library example;
const X int16 = -1;
const UNSIGNED uint16 = X;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/X", "int16", "uint16");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, GoodConvertFloatWider) {
  TestLibrary library(R"FIDL(
library example;
const X float32 = 23;
const WIDE float64 = X;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, GoodConvertFloatNarrower) {
  TestLibrary library(R"FIDL(
library example;
const X float64 = 3.4028234663852886e38; // max float32
const NARROW float32 = X;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ConstsTests, BadConvertFloatNarrowerOutOfRangePositive) {
  TestLibrary library(R"FIDL(
library example;
const X float64 = 3.402823466385289e38; // just above max float32
const NARROW float32 = X;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/X", "float64", "float32");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ConstsTests, BadConvertFloatNarrowerOutOfRangeNegative) {
  TestLibrary library(R"FIDL(
library example;
const X float64 = -3.402823466385289e38; // just below min float32
const NARROW float32 = X;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/X", "float64", "float32");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// It would be reasonable to allow conversions that don't lose precision (e.g.
// for float64 up to 2^53-1) but for simplicity we don't allow it right now.
TEST(ConstsTests, BadConvertIntegerToFloat) {
  TestLibrary library(R"FIDL(
library example;
const X uint16 = 1;
const FLOAT float32 = X;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/X", "uint16", "float32");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// It would be reasonable to allow conversions from floats that are exact
// integers, or to truncate, but for simplicity we don't allow it right now.
TEST(ConstsTests, BadConvertFloatToInteger) {
  TestLibrary library(R"FIDL(
library example;
const X float32 = 1;
const INTEGER uint16 = X;
)FIDL");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "example/X", "float32", "uint16");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
