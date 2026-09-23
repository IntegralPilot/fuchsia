// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/names.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(AliasTests, BadDuplicateAlias) {
  TestLibrary library(R"FIDL(
library example;

type Message = struct {
    f alias_of_int16;
};

alias alias_of_int16 = int16;
alias alias_of_int16 = int16;
)FIDL");
  library.ExpectFail(ErrNameCollision, Element::Kind::kAlias, "alias_of_int16",
                     Element::Kind::kAlias, "example.fidl:8:7");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, GoodAliasOfStruct) {
  TestLibrary library(R"FIDL(
library example;
type TypeDecl = struct {
    field1 uint16;
    field2 uint16;
};
alias AliasOfDecl = TypeDecl;
)FIDL");
  ASSERT_COMPILED(library);
  auto type_decl = library.LookupStruct("TypeDecl");
  ASSERT_NE(type_decl, nullptr);
  EXPECT_EQ(type_decl->members.size(), 2u);
  ASSERT_TRUE(library.HasAlias("AliasOfDecl"));
}

TEST(AliasTests, GoodPrimitive) {
  TestLibrary library(R"FIDL(
library example;

type Message = struct {
    f alias_of_int16;
};

alias alias_of_int16 = int16;
)FIDL");
  ASSERT_COMPILED(library);
  auto msg = library.LookupStruct("Message");
  ASSERT_NE(msg, nullptr);
  ASSERT_EQ(msg->members.size(), 1u);

  auto type = msg->members[0].type_ctor->type;
  ASSERT_EQ(type->kind, Type::Kind::kPrimitive);
  ASSERT_FALSE(type->IsNullable());

  auto primitive_type = static_cast<const PrimitiveType*>(type);
  ASSERT_EQ(primitive_type->subtype, PrimitiveSubtype::kInt16);

  auto invocation = msg->members[0].type_ctor->resolved_params;
  ASSERT_NE(invocation.from_alias, nullptr);
  EXPECT_EQ(FullyQualifiedName(invocation.from_alias->name), "example/alias_of_int16");
  EXPECT_EQ(invocation.element_type_resolved, nullptr);
  EXPECT_EQ(invocation.size_resolved, nullptr);
  EXPECT_EQ(invocation.nullability, Nullability::kNonnullable);
}

TEST(AliasTests, GoodPrimitiveAliasBeforeUse) {
  TestLibrary library(R"FIDL(
library example;

alias alias_of_int16 = int16;

type Message = struct {
    f alias_of_int16;
};
)FIDL");
  ASSERT_COMPILED(library);
  auto msg = library.LookupStruct("Message");
  ASSERT_NE(msg, nullptr);
  ASSERT_EQ(msg->members.size(), 1u);

  auto type = msg->members[0].type_ctor->type;
  ASSERT_EQ(type->kind, Type::Kind::kPrimitive);
  ASSERT_FALSE(type->IsNullable());

  auto primitive_type = static_cast<const PrimitiveType*>(type);
  ASSERT_EQ(primitive_type->subtype, PrimitiveSubtype::kInt16);

  auto invocation = msg->members[0].type_ctor->resolved_params;
  ASSERT_NE(invocation.from_alias, nullptr);
  EXPECT_EQ(FullyQualifiedName(invocation.from_alias->name), "example/alias_of_int16");
  EXPECT_EQ(invocation.element_type_resolved, nullptr);
  EXPECT_EQ(invocation.size_resolved, nullptr);
  EXPECT_EQ(invocation.nullability, Nullability::kNonnullable);
}

TEST(AliasTests, BadSelfReferentialAlias) {
  TestLibrary library(R"FIDL(
library example;

alias uint32 = uint32;

type Message = struct {
    f uint32;
};
)FIDL");
  library.ExpectFail(ErrIncludeCycle, "alias 'uint32' -> alias 'uint32'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, BadNoOptionalOnPrimitive) {
  TestLibrary library;
  library.AddFile("bad/fi-0156.test.fidl");
  library.ExpectFail(ErrCannotBeOptional, "int16");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, BadMultipleConstraintsOnPrimitive) {
  TestLibrary library(R"FIDL(
library test.optionals;

type Bad = struct {
    opt_num int64:<1, 2, 3>;
};
)FIDL");
  library.ExpectFail(ErrTooManyConstraints, "int64", 0, 3);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, BadInvalidSizeConstraintType) {
  TestLibrary library;
  library.AddFile("bad/fi-0101-a.test.fidl");
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "\"255\"", "string:3", "uint32");
  library.ExpectFail(ErrCouldNotResolveSizeBound);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, BadInvalidSizeConstraintIsNotValue) {
  TestLibrary library;
  library.AddFile("bad/fi-0101-b.test.fidl");
  library.ExpectFail(ErrCouldNotResolveSizeBound);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, BadNoOptionalOnAliasedPrimitive) {
  TestLibrary library(R"FIDL(
library test.optionals;

alias alias = int64;

type Bad = struct {
    opt_num alias:optional;
};
)FIDL");
  library.ExpectFail(ErrCannotBeOptional, "alias");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, GoodVectorParameterizedOnDecl) {
  TestLibrary library(R"FIDL(
library example;

type Message = struct {
    f alias_of_vector_of_string;
};

alias alias_of_vector_of_string = vector<string>;
)FIDL");
  ASSERT_COMPILED(library);
  auto msg = library.LookupStruct("Message");
  ASSERT_NE(msg, nullptr);
  ASSERT_EQ(msg->members.size(), 1u);

  auto type = msg->members[0].type_ctor->type;
  ASSERT_EQ(type->kind, Type::Kind::kVector);
  ASSERT_FALSE(type->IsNullable());

  auto vector_type = static_cast<const VectorType*>(type);
  ASSERT_EQ(vector_type->element_type->kind, Type::Kind::kString);
  ASSERT_EQ(vector_type->ElementCount(), kMaxSize);

  auto invocation = msg->members[0].type_ctor->resolved_params;
  ASSERT_NE(invocation.from_alias, nullptr);
  EXPECT_EQ(FullyQualifiedName(invocation.from_alias->name), "example/alias_of_vector_of_string");
  EXPECT_EQ(invocation.element_type_resolved, nullptr);
  EXPECT_EQ(invocation.size_resolved, nullptr);
  EXPECT_EQ(invocation.nullability, Nullability::kNonnullable);
}

TEST(AliasTests, BadVectorParameterizedOnUse) {
  TestLibrary library(R"FIDL(
library example;

type Message = struct {
    f alias_of_vector<uint8>;
};

alias alias_of_vector = vector;
)FIDL");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "alias_of_vector", 0, 1);
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "vector", 1, 0);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, BadVectorBoundedOnDecl) {
  TestLibrary library(R"FIDL(
library example;

type Message = struct {
    f alias_of_vector_max_8<string>;
};

alias alias_of_vector_max_8 = vector:8;
)FIDL");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "alias_of_vector_max_8", 0, 1);
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "vector", 1, 0);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, GoodVectorBoundedOnUse) {
  TestLibrary library(R"FIDL(
library example;

type Message = struct {
    f alias_of_vector_of_string:8;
};

alias alias_of_vector_of_string = vector<string>;
)FIDL");
  ASSERT_COMPILED(library);
  auto msg = library.LookupStruct("Message");
  ASSERT_NE(msg, nullptr);
  ASSERT_EQ(msg->members.size(), 1u);

  auto type = msg->members[0].type_ctor->type;
  ASSERT_EQ(type->kind, Type::Kind::kVector);
  ASSERT_FALSE(type->IsNullable());

  auto vector_type = static_cast<const VectorType*>(type);
  ASSERT_EQ(vector_type->element_type->kind, Type::Kind::kString);
  ASSERT_EQ(vector_type->ElementCount(), 8u);

  auto invocation = msg->members[0].type_ctor->resolved_params;
  ASSERT_NE(invocation.from_alias, nullptr);
  EXPECT_EQ(FullyQualifiedName(invocation.from_alias->name), "example/alias_of_vector_of_string");
  EXPECT_EQ(invocation.element_type_resolved, nullptr);
  EXPECT_NE(invocation.size_resolved, nullptr);
  EXPECT_EQ(invocation.size_resolved->value, 8u);
  EXPECT_EQ(invocation.nullability, Nullability::kNonnullable);
}

TEST(AliasTests, GoodUnboundedVectorBoundTwice) {
  TestLibrary library;
  library.AddFile("good/fi-0158.test.fidl");

  ASSERT_COMPILED(library);
}

TEST(AliasTests, GoodVectorNullableOnDecl) {
  TestLibrary library(R"FIDL(
library example;

type Message = struct {
    f alias_of_vector_of_string_nullable;
};

alias alias_of_vector_of_string_nullable = vector<string>:optional;
)FIDL");
  ASSERT_COMPILED(library);
  auto msg = library.LookupStruct("Message");
  ASSERT_NE(msg, nullptr);
  ASSERT_EQ(msg->members.size(), 1u);

  auto type = msg->members[0].type_ctor->type;
  ASSERT_EQ(type->kind, Type::Kind::kVector);
  ASSERT_TRUE(type->IsNullable());

  auto vector_type = static_cast<const VectorType*>(type);
  ASSERT_EQ(vector_type->element_type->kind, Type::Kind::kString);
  ASSERT_EQ(vector_type->ElementCount(), kMaxSize);

  auto invocation = msg->members[0].type_ctor->resolved_params;
  ASSERT_NE(invocation.from_alias, nullptr);
  EXPECT_EQ(FullyQualifiedName(invocation.from_alias->name),
            "example/alias_of_vector_of_string_nullable");
  EXPECT_EQ(invocation.element_type_resolved, nullptr);
  EXPECT_EQ(invocation.size_resolved, nullptr);
  EXPECT_EQ(invocation.nullability, Nullability::kNonnullable);
}

TEST(AliasTests, GoodVectorNullableOnUse) {
  TestLibrary library(R"FIDL(
library example;

type Message = struct {
    f alias_of_vector_of_string:optional;
};

alias alias_of_vector_of_string = vector<string>;
)FIDL");
  ASSERT_COMPILED(library);
  auto msg = library.LookupStruct("Message");
  ASSERT_NE(msg, nullptr);
  ASSERT_EQ(msg->members.size(), 1u);

  auto type = msg->members[0].type_ctor->type;
  ASSERT_EQ(type->kind, Type::Kind::kVector);
  ASSERT_TRUE(type->IsNullable());

  auto vector_type = static_cast<const VectorType*>(type);
  ASSERT_EQ(vector_type->element_type->kind, Type::Kind::kString);
  ASSERT_EQ(vector_type->ElementCount(), kMaxSize);

  auto invocation = msg->members[0].type_ctor->resolved_params;
  ASSERT_NE(invocation.from_alias, nullptr);
  EXPECT_EQ(FullyQualifiedName(invocation.from_alias->name), "example/alias_of_vector_of_string");
  EXPECT_EQ(invocation.element_type_resolved, nullptr);
  EXPECT_EQ(invocation.size_resolved, nullptr);
  EXPECT_EQ(invocation.nullability, Nullability::kNullable);
}

TEST(AliasTests, BadCannotParameterizeTwice) {
  TestLibrary library(R"FIDL(
library example;

type Message = struct {
    f alias_of_vector_of_string<string>;
};

alias alias_of_vector_of_string = vector<string>;
)FIDL");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "alias_of_vector_of_string", 0, 1);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, BadCannotBoundTwice) {
  TestLibrary library;
  library.AddFile("bad/fi-0158.test.fidl");

  library.ExpectFail(ErrCannotBoundTwice, "ByteVec256");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, BadCannotNullTwice) {
  TestLibrary library;
  library.AddFile("bad/fi-0160.test.fidl");

  library.ExpectFail(ErrCannotIndicateOptionalTwice, "MyAlias");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, GoodMultiFileAliasReference) {
  TestLibrary library;
  library.AddSource("first.fidl", R"FIDL(
library example;

type Protein = struct {
  amino_acids AminoAcids;
};
)FIDL");
  library.AddSource("second.fidl", R"FIDL(
library example;

alias AminoAcids = vector<uint64>:32;
)FIDL");

  ASSERT_COMPILED(library);
}

TEST(AliasTests, GoodMultiFileNullableAliasReference) {
  TestLibrary library;
  library.AddSource("first.fidl", R"FIDL(
library example;

type Protein = struct {
    amino_acids AminoAcids:optional;
};
)FIDL");
  library.AddSource("second.fidl", R"FIDL(
library example;

alias AminoAcids = vector<uint64>:32;
)FIDL");

  ASSERT_COMPILED(library);
}

TEST(AliasTests, BadRecursiveAlias) {
  TestLibrary library(R"FIDL(
library example;

alias TheAlias = TheStruct;

type TheStruct = struct {
    many_mini_me vector<TheAlias>;
};
)FIDL");

  library.ExpectFail(ErrIncludeCycle, "alias 'TheAlias' -> struct 'TheStruct' -> alias 'TheAlias'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, BadCompoundIdentifier) {
  TestLibrary library(R"FIDL(
library example;

alias foo.bar.baz = uint8;
)FIDL");

  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kDot),
                     Token::KindAndSubkind(Token::Kind::kEqual));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(AliasTests, GoodUsingLibrary) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependent.fidl", R"FIDL(
library dependent;

type Bar = struct {
    s int8;
};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using dependent;

alias Bar2 = dependent.Bar;
)FIDL");
  ASSERT_COMPILED(library);
}

}  // namespace
}  // namespace fidlc
