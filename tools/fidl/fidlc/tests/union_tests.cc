// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(UnionTests, GoodKeywordsAsFieldNames) {
  TestLibrary library(R"FIDL(
library test;

type struct = struct {
    field bool;
};

type Foo = strict union {
    1: union int64;
    2: library bool;
    3: uint32 uint32;
    4: member struct;
    5: reserved bool;
};
)FIDL");
  ASSERT_COMPILED(library);
  auto type_decl = library.LookupUnion("Foo");
  ASSERT_NE(type_decl, nullptr);
  EXPECT_EQ(type_decl->members.size(), 5u);
}

TEST(UnionTests, GoodRecursiveUnion) {
  TestLibrary library(R"FIDL(
library test;

type Value = strict union {
    1: bool_value bool;
    2: list_value vector<Value:optional>;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(UnionTests, GoodMutuallyRecursive) {
  TestLibrary library(R"FIDL(
library test;

type Foo = strict union {
    1: bar Bar;
};

type Bar = struct {
    foo Foo:optional;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(UnionTests, GoodFlexibleUnion) {
  TestLibrary library(R"FIDL(
library test;

type Foo = flexible union {
    1: bar string;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(UnionTests, GoodStrictUnion) {
  TestLibrary library;
  library.AddFile("good/fi-0018.test.fidl");
  ASSERT_COMPILED(library);
}

TEST(UnionTests, BadMustHaveExplicitOrdinals) {
  TestLibrary library;
  library.AddFile("bad/fi-0016-b.noformat.test.fidl");
  library.ExpectFail(ErrMissingOrdinalBeforeMember);
  library.ExpectFail(ErrMissingOrdinalBeforeMember);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, GoodExplicitOrdinals) {
  TestLibrary library(R"FIDL(
library test;

type Foo = strict union {
    1: foo int64;
    2: bar vector<uint32>:10;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto fidl_union = library.LookupUnion("Foo");
  ASSERT_NE(fidl_union, nullptr);

  ASSERT_EQ(fidl_union->members.size(), 2u);
  EXPECT_EQ(fidl_union->members[0].ordinal->value, 1u);
  EXPECT_EQ(fidl_union->members[1].ordinal->value, 2u);
}

TEST(UnionTests, GoodOrdinalGaps) {
  TestLibrary library(R"FIDL(
library test;

type Foo = strict union {
    2: foo int64;
    4: bar vector<uint32>:10;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto fidl_union = library.LookupUnion("Foo");
  ASSERT_NE(fidl_union, nullptr);

  ASSERT_EQ(fidl_union->members.size(), 2u);
  EXPECT_EQ(fidl_union->members[0].ordinal->value, 2u);
  EXPECT_EQ(fidl_union->members[1].ordinal->value, 4u);
}

TEST(UnionTests, GoodOrdinalsOutOfOrder) {
  TestLibrary library(R"FIDL(
library test;

type Foo = strict union {
    5: foo int64;
    2: bar vector<uint32>:10;
    4: baz uint32;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto fidl_union = library.LookupUnion("Foo");
  ASSERT_NE(fidl_union, nullptr);

  ASSERT_EQ(fidl_union->members.size(), 3u);
  EXPECT_EQ(fidl_union->members[0].ordinal->value, 2u);
  EXPECT_EQ(fidl_union->members[1].ordinal->value, 4u);
  EXPECT_EQ(fidl_union->members[2].ordinal->value, 5u);
}

TEST(UnionTests, BadOrdinalOutOfBoundsNegative) {
  TestLibrary library;
  library.AddFile("bad/fi-0017-b.noformat.test.fidl");
  library.ExpectFail(ErrOrdinalOutOfBound);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, BadOrdinalOutOfBoundsLarge) {
  TestLibrary library(R"FIDL(
library test;

type Foo = union {
  4294967296: foo string;
};
)FIDL");
  library.ExpectFail(ErrOrdinalOutOfBound);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, BadOrdinalsMustBeUnique) {
  TestLibrary library;
  library.AddFile("bad/fi-0097.test.fidl");
  library.ExpectFail(ErrDuplicateUnionMemberOrdinal, "bad/fi-0097.test.fidl:7:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, BadMemberNamesMustBeUnique) {
  TestLibrary library(R"FIDL(
library test;

type MyUnion = strict union {
    1: my_variant string;
    2: my_variant int32;
};
)FIDL");
  library.ExpectFail(ErrNameCollision, Element::Kind::kUnionMember, "my_variant",
                     Element::Kind::kUnionMember, "example.fidl:5:8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, BadCannotStartAtZero) {
  TestLibrary library;
  library.AddFile("bad/fi-0018.noformat.test.fidl");
  library.ExpectFail(ErrOrdinalsMustStartAtOne);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, BadDefaultNotAllowed) {
  TestLibrary library(R"FIDL(
library test;

type Foo = strict union {
    1: t int64 = 1;
};
)FIDL");
  library.ExpectFail(ErrUnexpectedTokenOfKind, Token::KindAndSubkind(Token::Kind::kEqual),
                     Token::KindAndSubkind(Token::Kind::kSemicolon));
  library.ExpectFail(ErrMissingOrdinalBeforeMember);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, GoodOrdinalGapStart) {
  TestLibrary library(R"FIDL(
library example;

type Example = strict union {
    2: two int64;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(UnionTests, GoodOrdinalGapMiddle) {
  TestLibrary library(R"FIDL(
library example;

type Example = strict union {
    1: one int64;
    3: three int64;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(UnionTests, BadNoNullableMembers) {
  TestLibrary library;
  library.AddFile("bad/fi-0049.test.fidl");
  library.ExpectFail(ErrOptionalUnionMember);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, BadNoDirectlyRecursiveUnions) {
  TestLibrary library(R"FIDL(
library example;

type Value = strict union {
  1: value Value;
};
)FIDL");
  library.ExpectFail(ErrIncludeCycle, "union 'Value' -> union 'Value'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, GoodEmptyFlexibleUnion) {
  TestLibrary library(R"FIDL(
library example;

type Foo = flexible union {};
)FIDL");
  ASSERT_COMPILED(library);

  auto fidl_union = library.LookupUnion("Foo");
  ASSERT_NE(fidl_union, nullptr);
  ASSERT_EQ(fidl_union->members.size(), 0u);
}

TEST(UnionTests, BadEmptyStrictUnion) {
  TestLibrary library(R"FIDL(
library example;

type Value = strict union {};
)FIDL");
  library.ExpectFail(ErrMustHaveOneMember);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UnionTests, GoodErrorSyntaxExplicitOrdinals) {
  TestLibrary library(R"FIDL(
library example;
open protocol Example {
    flexible Method() -> () error int32;
};
)FIDL");
  ASSERT_COMPILED(library);
  const Union* error_union = library.LookupUnion("Example_Method_Result");
  ASSERT_NE(error_union, nullptr);
  ASSERT_EQ(3u, error_union->members.size());
  ASSERT_EQ(1u, error_union->members[0].ordinal->value);
  ASSERT_EQ(2u, error_union->members[1].ordinal->value);
  ASSERT_EQ(3u, error_union->members[2].ordinal->value);
}

TEST(UnionTests, BadNoSelector) {
  TestLibrary library(R"FIDL(
library example;

type Foo = strict union {
  @selector("v2") 1: v string;
};
)FIDL");
  library.ExpectFail(ErrInvalidAttributePlacement, "selector");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
