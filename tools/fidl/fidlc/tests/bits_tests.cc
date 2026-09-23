// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(BitsTests, GoodSimple) {
  TestLibrary library;
  library.AddFile("good/fi-0067-a.test.fidl");

  ASSERT_COMPILED(library);
  auto type_decl = library.LookupBits("Fruit");
  ASSERT_NE(type_decl, nullptr);
  EXPECT_EQ(type_decl->members.size(), 3u);
  auto underlying = type_decl->subtype_ctor->type;
  ASSERT_EQ(underlying->kind, Type::Kind::kPrimitive);
  auto underlying_primitive = static_cast<const PrimitiveType*>(underlying);
  EXPECT_EQ(underlying_primitive->subtype, PrimitiveSubtype::kUint64);
}

TEST(BitsTests, GoodDefaultUint32) {
  TestLibrary library(R"FIDL(
library example;

type Fruit = bits {
    ORANGE = 1;
};
)FIDL");
  ASSERT_COMPILED(library);
  auto type_decl = library.LookupBits("Fruit");
  ASSERT_NE(type_decl, nullptr);
  auto underlying = type_decl->subtype_ctor->type;
  ASSERT_EQ(underlying->kind, Type::Kind::kPrimitive);
  auto underlying_primitive = static_cast<const PrimitiveType*>(underlying);
  EXPECT_EQ(underlying_primitive->subtype, PrimitiveSubtype::kUint32);
}

TEST(BitsTests, BadSigned) {
  TestLibrary library;
  library.AddFile("bad/fi-0069.test.fidl");
  library.ExpectFail(ErrBitsTypeMustBeUnsignedIntegralPrimitive, "int64");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(BitsTests, BadNonUniqueValues) {
  TestLibrary library(R"FIDL(
library example;

type Fruit = bits : uint64 {
    ORANGE = 1;
    APPLE = 1;
};
)FIDL");
  library.ExpectFail(ErrDuplicateMemberValue, Decl::Kind::kBits, "APPLE", "ORANGE",
                     "example.fidl:5:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(BitsTests, BadNonUniqueValuesOutOfLine) {
  TestLibrary library(R"FIDL(
library example;

type Fruit = bits {
    ORANGE = FOUR;
    APPLE = TWO_SQUARED;
};

const FOUR uint32 = 4;
const TWO_SQUARED uint32 = 4;
)FIDL");
  library.ExpectFail(ErrDuplicateMemberValue, Decl::Kind::kBits, "APPLE", "ORANGE",
                     "example.fidl:5:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(BitsTests, BadUnsignedWithNegativeMember) {
  TestLibrary library;
  library.AddFile("bad/fi-0102.test.fidl");
  library.ExpectFail(ErrCouldNotResolveMember, Decl::Kind::kBits);
  library.ExpectFail(ErrConstantOverflowsType, "-4", "uint64");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(BitsTests, BadMemberOverflow) {
  TestLibrary library(R"FIDL(
library example;

type Fruit = bits : uint8 {
    ORANGE = 1;
    APPLE = 256;
};
)FIDL");
  library.ExpectFail(ErrCouldNotResolveMember, Decl::Kind::kBits);
  library.ExpectFail(ErrConstantOverflowsType, "256", "uint8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(BitsTests, BadDuplicateMember) {
  TestLibrary library(R"FIDL(
library example;

type Fruit = bits : uint64 {
    ORANGE = 1;
    APPLE = 2;
    ORANGE = 4;
};
)FIDL");
  library.ExpectFail(ErrNameCollision, Element::Kind::kBitsMember, "ORANGE",
                     Element::Kind::kBitsMember, "example.fidl:5:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(BitsTests, BadNoMembersWhenStrict) {
  TestLibrary library(R"FIDL(
library example;

type B = strict bits {};
)FIDL");
  library.ExpectFail(ErrMustHaveOneMember);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(BitsTests, GoodNoMembersAllowedWhenFlexible) {
  TestLibrary library(R"FIDL(
library example;

type B = flexible bits {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(BitsTests, GoodNoMembersAllowedWhenDefaultsToFlexible) {
  TestLibrary library(R"FIDL(
library example;

type B = bits {};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(BitsTests, GoodKeywordNames) {
  TestLibrary library(R"FIDL(
library example;

type Fruit = bits : uint64 {
    library = 1;
    bits = 2;
    uint64 = 4;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(BitsTests, BadNonPowerOfTwo) {
  TestLibrary library;
  library.AddFile("bad/fi-0067.test.fidl");

  library.ExpectFail(ErrBitsMemberMustBePowerOfTwo);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(BitsTests, GoodWithMask) {
  TestLibrary library;
  library.AddFile("good/fi-0067-b.test.fidl");

  ASSERT_COMPILED(library);

  auto bits = library.LookupBits("Life");
  ASSERT_NE(bits, nullptr);
  EXPECT_EQ(bits->mask, 42u);
}

TEST(BitsTests, BadShantBeNullable) {
  TestLibrary library(R"FIDL(
library example;

type NotNullable = bits {
    MEMBER = 1;
};

type Struct = struct {
    not_nullable NotNullable:optional;
};
)FIDL");
  library.ExpectFail(ErrCannotBeOptional, "NotNullable");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(BitsTests, BadMultipleConstraints) {
  TestLibrary library(R"FIDL(
library example;

type NotNullable = bits {
    MEMBER = 1;
};

type Struct = struct {
    not_nullable NotNullable:<1, 2, 3>;
};
)FIDL");
  library.ExpectFail(ErrTooManyConstraints, "NotNullable", 1, 3);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
