// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(GeneratedNameTests, GoodInsideStruct) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  bar @generated_name("Good") struct {};
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  auto bar_type = foo->members[0].type_ctor->type;
  EXPECT_EQ(bar_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodInsideTable) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = table {
  1: bar @generated_name("Good") struct {};
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupTable("Foo");
  ASSERT_NE(foo, nullptr);
  auto bar_type = foo->members[0].type_ctor->type;
  EXPECT_EQ(bar_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodInsideUnion) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = union {
  1: bar @generated_name("Good") struct {};
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupUnion("Foo");
  ASSERT_NE(foo, nullptr);
  auto bar_type = foo->members[0].type_ctor->type;
  EXPECT_EQ(bar_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodInsideRequest) {
  TestLibrary library(R"FIDL(
library fidl.test;

protocol Foo {
  Bar(@generated_name("Good") struct { x uint32; });
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupProtocol("Foo");
  ASSERT_NE(foo, nullptr);

  auto id = static_cast<const IdentifierType*>(foo->methods[0].maybe_request->type);
  auto request_type = static_cast<const Struct*>(id->type_decl);
  EXPECT_EQ(request_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodInsideResponse) {
  TestLibrary library(R"FIDL(
library fidl.test;

protocol Foo {
  strict Bar() -> (@generated_name("Good") struct { x uint32; });
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupProtocol("Foo");
  ASSERT_NE(foo, nullptr);

  auto id = static_cast<const IdentifierType*>(foo->methods[0].maybe_response->type);
  auto response_type = static_cast<const Struct*>(id->type_decl);
  EXPECT_EQ(response_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodInsideResultSuccess) {
  TestLibrary library(R"FIDL(
library fidl.test;

protocol Foo {
  Bar() -> (@generated_name("Good") struct { x uint32; }) error uint32;
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupProtocol("Foo");
  ASSERT_NE(foo, nullptr);

  auto id = static_cast<const IdentifierType*>(foo->methods[0].maybe_response->type);
  auto result_union = static_cast<const Union*>(id->type_decl);
  auto success_type = result_union->members[0].type_ctor->type;
  EXPECT_EQ(success_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodInsideResultError) {
  TestLibrary library(R"FIDL(
library fidl.test;

protocol Foo {
  Bar() -> () error @generated_name("Good") enum { A = 1; };
};
)FIDL");
  ASSERT_COMPILED(library)
  auto foo = library.LookupProtocol("Foo");
  ASSERT_NE(foo, nullptr);

  auto id = static_cast<const IdentifierType*>(foo->methods[0].maybe_response->type);
  auto result_union = static_cast<const Union*>(id->type_decl);
  auto error_type = result_union->members[1].type_ctor->type;
  EXPECT_EQ(error_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodOnBits) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  bar @generated_name("Good") bits {
    A = 1;
  };
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  auto bar_type = foo->members[0].type_ctor->type;
  EXPECT_EQ(bar_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodOnEnum) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  bar @generated_name("Good") enum {
    A = 1;
  };
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  auto bar_type = foo->members[0].type_ctor->type;
  EXPECT_EQ(bar_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodOnStruct) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  bar @generated_name("Good") struct {
    x uint32;
  };
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  auto bar_type = foo->members[0].type_ctor->type;
  EXPECT_EQ(bar_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodOnTable) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  bar @generated_name("Good") table {
    1: x uint32;
  };
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  auto bar_type = foo->members[0].type_ctor->type;
  EXPECT_EQ(bar_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodOnUnion) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  bar @generated_name("Good") union {
    1: x uint32;
  };
};
)FIDL");
  ASSERT_COMPILED(library);
  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  auto bar_type = foo->members[0].type_ctor->type;
  EXPECT_EQ(bar_type->name.decl_name(), "Good");
}

TEST(GeneratedNameTests, GoodPreventsCollision) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  foo @generated_name("Bar") struct {};
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(GeneratedNameTests, BadOnTypeDeclaration) {
  TestLibrary library(R"FIDL(
library fidl.test;

@generated_name("Good")
type Bad = struct {};
)FIDL");
  library.ExpectFail(ErrInvalidAttributePlacement, "generated_name");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadOnTopLevelStruct) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = @generated_name("Bad") struct {};
)FIDL");
  library.ExpectFail(ErrAttributeInsideTypeDeclaration);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadOnIdentifierType) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  bar @generated_name("Bad") Bar;
};

type Bar = struct {};
)FIDL");
  library.ExpectFail(ErrCannotAttachAttributeToIdentifier);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadOnStructMember) {
  TestLibrary library;
  library.AddFile("bad/fi-0120-b.test.fidl");
  library.ExpectFail(ErrInvalidAttributePlacement, "generated_name");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadOnEnumMember) {
  TestLibrary library(R"FIDL(
library fidl.test;

type MetaVars = enum {
  FOO = 1;
  @generated_name("BAD")
  BAR = 2;
};
)FIDL");
  library.ExpectFail(ErrInvalidAttributePlacement, "generated_name");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadOnServiceMember) {
  TestLibrary library(R"FIDL(
library fidl.test;

protocol Foo {};

service Bar {
  @generated_name("One")
  bar_one client_end:Foo;
};
)FIDL");
  library.ExpectFail(ErrInvalidAttributePlacement, "generated_name");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadMissingArgument) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  bad @generated_name struct {};
};
)FIDL");
  library.ExpectFail(ErrMissingRequiredAnonymousAttributeArg, "generated_name");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadInvalidIdentifier) {
  TestLibrary library;
  library.AddFile("bad/fi-0146.test.fidl");
  library.ExpectFail(ErrInvalidGeneratedName);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadNameCollision) {
  TestLibrary library(R"FIDL(
library fidl.test;

type Foo = struct {
  foo @generated_name("Baz") struct {};
};

type Baz = struct {};
)FIDL");
  library.ExpectFail(ErrNameCollision, Element::Kind::kStruct, "Baz", Element::Kind::kStruct,
                     "example.fidl:5:30");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadNotString) {
  TestLibrary library;
  library.AddFile("bad/fi-0104.test.fidl");
  library.ExpectFail(ErrTypeCannotBeConvertedToType, "true", "bool", "string");
  library.ExpectFail(ErrCouldNotResolveAttributeArg);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(GeneratedNameTests, BadNonLiteralArgument) {
  TestLibrary library;
  library.AddFile("bad/fi-0133.test.fidl");
  library.ExpectFail(ErrAttributeArgRequiresLiteral, "value", "generated_name");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
