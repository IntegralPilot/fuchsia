// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(RecoverableCompilationTests, BadRecoverInLibraryConsume) {
  TestLibrary library(R"FIDL(
library example;

protocol P {};
protocol P {};              // Error: name collision

type foo = struct {};
type Foo = struct {};       // Error: canonical name collision
)FIDL");
  library.ExpectFail(ErrNameCollision, Element::Kind::kProtocol, "P", Element::Kind::kProtocol,
                     "example.fidl:4:10");
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStruct, "foo",
                     Element::Kind::kStruct, "Foo", "example.fidl:8:6", "foo");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInLibraryCompile) {
  TestLibrary library(R"FIDL(
library example;

type Union = union {
    1: string_value string;
    2: unknown_value vector;      // Error: expected 1 layout parameter
};

type Enum = enum {
    ZERO = 0;
    ONE = 1;
    TWO = 1;                      // Error: duplicate value
    THREE = 3;
};

type OtherEnum = enum {
    NONE = 0;
    ONE = 1;
    TWO = "2";                    // Error: invalid type
};

type NonDenseTable = table {
    65: s string;                 // Error: too many ordinals
};
)FIDL");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "vector", 1, 0);
  library.ExpectFail(ErrDuplicateMemberValue, Decl::Kind::kEnum, "TWO", "ONE", "example.fidl:11:5");
  library.ExpectFail(ErrCouldNotResolveMember, Decl::Kind::kEnum);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"2\"", "string:1", "uint32");
  library.ExpectFail(ErrTableOrdinalTooLarge);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInLibraryVerifyAttributePlacement) {
  TestLibrary library(R"FIDL(
library example;

@unknown            // Error: invalid placement
type Table = table {
    1: foo string;
};

type Struct = struct {
    foo uint16;
};
)FIDL");
  library.ExpectFail(ErrInvalidAttributePlacement, "unknown");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInAttributeCompile) {
  TestLibrary library(R"FIDL(
library example;

@foo(first="a", first="b")   // Error: duplicate args
@bar(first=3, second=4)      // Error: x2 can only use string or bool
@foo                         // Error: duplicate attribute
type Enum = enum {
    FOO                      // Error: cannot resolve enum member
        = "not a number";    // Error: cannot be interpreted as uint32
};
)FIDL");
  library.ExpectFail(ErrDuplicateAttributeArg, "foo", "first", "example.fidl:4:6");
  library.ExpectFail(ErrCanOnlyUseStringOrBool, "first", "bar");
  library.ExpectFail(ErrCanOnlyUseStringOrBool, "second", "bar");
  library.ExpectFail(ErrDuplicateAttribute, "foo", "example.fidl:4:2");
  library.ExpectFail(ErrCouldNotResolveMember, Decl::Kind::kEnum);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"not a number\"", "string:12", "uint32");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInConst) {
  TestLibrary library(R"FIDL(
library example;

@attr(1)
const FOO string = 2;
)FIDL");
  library.ExpectFail(ErrCanOnlyUseStringOrBool, "value", "attr");
  library.ExpectFail(ErrCannotResolveConstantValue);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "2", "untyped numeric", "string");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInBits) {
  TestLibrary library(R"FIDL(
library example;

type Foo = bits {
    BAR                    // Error: cannot resolve bits member
        = "not a number";  // Error: cannot interpret as uint32
    QUX = vector;          // Error: cannot resolve bits member
    TWO = 2;
    BAZ = 2;               // Error: duplicate value 2
    XYZ = 3;               // Error: not a power of two
};
)FIDL");
  library.ExpectFail(ErrCouldNotResolveMember, Decl::Kind::kBits);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"not a number\"", "string:12", "uint32");
  library.ExpectFail(ErrCouldNotResolveMember, Decl::Kind::kBits);
  library.ExpectFail(ErrDuplicateMemberValue, Decl::Kind::kBits, "BAZ", "TWO", "example.fidl:8:5");
  library.ExpectFail(ErrBitsMemberMustBePowerOfTwo);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInEnum) {
  TestLibrary library(R"FIDL(
library example;

type Foo = flexible enum : uint8 {
    BAR                    // Error: cannot resolve enum member
        = "not a number";  // Error: cannot interpret as uint32
    QUX = vector;          // Error: cannot resolve enum member
    TWO = 2;
    BAZ = 2;               // Error: duplicate value 2
    XYZ = 255;             // Error: max value on flexible enum
};
)FIDL");
  library.ExpectFail(ErrCouldNotResolveMember, Decl::Kind::kEnum);
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"not a number\"", "string:12", "uint8");
  library.ExpectFail(ErrCouldNotResolveMember, Decl::Kind::kEnum);
  library.ExpectFail(ErrDuplicateMemberValue, Decl::Kind::kEnum, "BAZ", "TWO", "example.fidl:8:5");
  library.ExpectFail(ErrFlexibleEnumMemberWithMaxValue, "255");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInStruct) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
    bar string<1>;     // Error: unexpected layout parameter
    qux vector;        // Error: expected 1 layout parameter
    @allow_deprecated_struct_defaults
    baz bool           // Error: cannot resolve default value
        = "not bool";  // Error: cannot interpret as bool
};
)FIDL");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "string", 0, 1);
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "vector", 1, 0);
  library.ExpectFail(ErrCouldNotResolveMemberDefault, "baz");
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"not bool\"", "string:8", "bool");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInTable) {
  TestLibrary library(R"FIDL(
library example;

type Foo = table {
    1: bar string:optional;  // Error: table member cannot be optional
    1: qux                   // Error: duplicate ordinal
       vector;               // Error: expected 1 layout parameter
    65: s string;            // Error: too many ordinals
};
)FIDL");
  library.ExpectFail(ErrOptionalTableMember);
  library.ExpectFail(ErrDuplicateTableFieldOrdinal, "example.fidl:5:5");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "vector", 1, 0);
  library.ExpectFail(ErrTableOrdinalTooLarge);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInUnion) {
  TestLibrary library(R"FIDL(
library example;

type Foo = union {
    1: bar string:optional;  // Error: union member cannot be optional
    1: qux                   // Error: duplicate ordinal
        vector;              // Error: expected 1 layout parameter
};
)FIDL");
  library.ExpectFail(ErrOptionalUnionMember);
  library.ExpectFail(ErrDuplicateUnionMemberOrdinal, "example.fidl:5:5");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "vector", 1, 0);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInProtocol) {
  TestLibrary library(R"FIDL(
library example;

protocol Foo {
    compose vector;              // Error: expected protocol
    @selector("not good")        // Error: invalid selector
    Bar(struct {}) -> (struct {  // Error: empty struct invalid
        b bool:optional;         // Error: bool cannot be optional
    }) error vector;             // Error: expected 1 layout parameter
};
)FIDL");
  library.ExpectFail(ErrComposingNonProtocol);
  library.ExpectFail(ErrInvalidSelectorValue);
  library.ExpectFail(ErrEmptyPayloadStructs);
  library.ExpectFail(ErrCannotBeOptional, "bool");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "vector", 1, 0);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(RecoverableCompilationTests, BadRecoverInService) {
  TestLibrary library(R"FIDL(
library example;

protocol P {};
service Foo {
    bar string;                   // Error: must be client_end
    baz vector;                   // Error: expected 1 layout parameter
    qux server_end:P;             // Error: must be client_end
    opt client_end:<P,optional>;  // Error: cannot be optional
};
)FIDL");
  library.ExpectFail(ErrOnlyClientEndsInServices);
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "vector", 1, 0);
  library.ExpectFail(ErrOnlyClientEndsInServices);
  library.ExpectFail(ErrOptionalServiceMember);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
