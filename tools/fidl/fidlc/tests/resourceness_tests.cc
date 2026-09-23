// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/source_file.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(ResourcenessTests, BadBitsResourceness) {
  TestLibrary library(R"FIDL(
library example;
type Foo = resource bits {
    BAR = 1;
};
)FIDL");
  library.ExpectFail(ErrCannotSpecifyModifier, Token::KindAndSubkind(Token::Subkind::kResource),
                     Token::KindAndSubkind(Token::Subkind::kBits));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, BadEnumResourceness) {
  TestLibrary library(R"FIDL(
library example;
type Foo = resource enum {
    BAR = 1;
};
)FIDL");
  library.ExpectFail(ErrCannotSpecifyModifier, Token::KindAndSubkind(Token::Subkind::kResource),
                     Token::KindAndSubkind(Token::Subkind::kEnum));
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, BadConstResourceness) {
  TestLibrary library(R"FIDL(
library example;

resource const BAR uint32 = 1;
)FIDL");
  library.ExpectFail(ErrExpectedDeclaration, "resource");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, BadProtocolResourceness) {
  TestLibrary library(R"FIDL(
library example;

resource protocol Foo {};
)FIDL");
  library.ExpectFail(ErrExpectedDeclaration, "resource");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, BadAliasResourceness) {
  TestLibrary library(R"FIDL(
library example;

resource alias B = bool;
)FIDL");
  library.ExpectFail(ErrExpectedDeclaration, "resource");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, BadDuplicateModifier) {
  TestLibrary library(R"FIDL(
library example;

type One = resource struct {};
type Two = resource resource struct {};
type Three = resource resource resource struct {};
)FIDL");
  library.ExpectFail(ErrDuplicateModifier, "modifier 'resource'");
  library.ExpectFail(ErrDuplicateModifier, "modifier 'resource'");
  library.ExpectFail(ErrDuplicateModifier, "modifier 'resource'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, GoodResourceSimple) {
  TestLibrary library;
  library.UseLibraryZx();
  library.AddFile("good/fi-0110-a.test.fidl");

  ASSERT_COMPILED(library);
}

TEST(ResourcenessTests, BadResourceModifierMissing) {
  TestLibrary library;
  library.UseLibraryZx();
  library.AddFile("bad/fi-0110.test.fidl");
  library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Foo", "handle",
                     "bad/fi-0110.test.fidl:9:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, GoodResourceStruct) {
  for (const std::string& definition : {
           "type Foo =  resource struct {};",
           "type Foo = resource struct { b bool; };",
           "using zx;\ntype Foo = resource struct{ h zx.Handle; };",
           "using zx;\ntype Foo = resource struct{ a array<zx.Handle, 1>; };",
           "using zx;\ntype Foo = resource struct{ v vector<zx.Handle>; };",
       }) {
    std::string fidl_library = "library example;\n" + definition;
    SCOPED_TRACE(fidl_library);
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    ASSERT_COMPILED(library);
    EXPECT_EQ(library.LookupStruct("Foo")->resourceness, Resourceness::kResource);
  }
}

TEST(ResourcenessTests, GoodResourceTable) {
  for (const std::string& definition : {
           "type Foo = resource table {};",
           "type Foo = resource table { 1: b bool; };",
           "using zx;\ntype Foo = resource table { 1: h zx.Handle; };",
           "using zx;\ntype Foo = resource table { 1: a array<zx.Handle, 1>; };",
           "using zx;\ntype Foo = resource table { 1: v vector<zx.Handle>; };",
       }) {
    std::string fidl_library = "library example;\n" + definition;
    SCOPED_TRACE(fidl_library);
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    ASSERT_COMPILED(library);
    EXPECT_EQ(library.LookupTable("Foo")->resourceness, Resourceness::kResource);
  }
}

TEST(ResourcenessTests, GoodResourceUnion) {
  for (const std::string& definition : {
           "type Foo = resource union { 1: b bool; };",
           "using zx;\ntype Foo = resource union { 1: h zx.Handle; };",
           "using zx;\ntype Foo = resource union { 1: a array<zx.Handle, 1>; };",
           "using zx;\ntype Foo = resource union { 1: v vector<zx.Handle>; };",
       }) {
    std::string fidl_library = "library example;\n" + definition;
    SCOPED_TRACE(fidl_library);
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    ASSERT_COMPILED(library);
    EXPECT_EQ(library.LookupUnion("Foo")->resourceness, Resourceness::kResource);
  }
}

TEST(ResourcenessTests, BadHandlesInValueStruct) {
  for (const std::string& definition : {
           "type Foo = struct { bad_member zx.Handle; };",
           "type Foo = struct { bad_member zx.Handle:optional; };",
           "type Foo = struct { bad_member array<zx.Handle, 1>; };",
           "type Foo = struct { bad_member vector<zx.Handle>; };",
           "type Foo = struct { bad_member vector<zx.Handle>:0; };",
       }) {
    std::string fidl_library = "library example;\nusing zx;\n\n" + definition + "\n";
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Foo", "bad_member",
                       "example.fidl:4:21");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ResourcenessTests, BadHandlesInValueTable) {
  for (const std::string& definition : {
           "type Foo = table { 1: bad_member zx.Handle; };",
           "type Foo = table { 1: bad_member array<zx.Handle, 1>; };",
           "type Foo = table { 1: bad_member vector<zx.Handle>; };",
           "type Foo = table { 1: bad_member vector<zx.Handle>:0; };",
       }) {
    std::string fidl_library = "library example;\nusing zx;\n\n" + definition + "\n";
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kTable, "Foo", "bad_member",
                       "example.fidl:4:23");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ResourcenessTests, BadHandlesInValueUnion) {
  for (const std::string& definition : {
           "type Foo = union { 1: bad_member zx.Handle; };",
           "type Foo = union { 1: bad_member array<zx.Handle, 1>; };",
           "type Foo = union { 1: bad_member vector<zx.Handle>; };",
           "type Foo = union { 1: bad_member vector<zx.Handle>:0; };",
       }) {
    std::string fidl_library = "library example;\nusing zx;\n\n" + definition + "\n";
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kUnion, "Foo", "bad_member",
                       "example.fidl:4:23");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ResourcenessTests, BadProtocolsInValueType) {
  for (const std::string& definition : {
           "type Foo = struct { bad_member client_end:Protocol; };",
           "type Foo = struct { bad_member client_end:<Protocol, optional>; };",
           "type Foo = struct { bad_member server_end:Protocol; };",
           "type Foo = struct { bad_member server_end:<Protocol, optional>; };",
       }) {
    std::string fidl_library = R"FIDL(
library example;
using zx;

protocol Protocol {};

)FIDL" + definition + "\n";
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Foo", "bad_member",
                       "example.fidl:7:21");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}
TEST(ResourcenessTests, BadResourceTypesInValueType) {
  for (const std::string& definition : {
           "type Foo = struct { bad_member ResourceStruct; };",
           "type Foo = struct { bad_member box<ResourceStruct>; };",
           "type Foo = struct { bad_member ResourceTable; };",
           "type Foo = struct { bad_member ResourceUnion; };",
           "type Foo = struct { bad_member ResourceUnion:optional; };",
       }) {
    std::string fidl_library = R"FIDL(
library example;

type ResourceStruct = resource struct {};
type ResourceTable = resource table {};
type ResourceUnion = resource union { 1: b bool; };

)FIDL" + definition + "\n";
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Foo", "bad_member",
                       "example.fidl:8:21");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ResourcenessTests, BadResourceAliasesInValueType) {
  for (const std::string& definition : {
           "type Foo = struct { bad_member HandleAlias; };",
           "type Foo = struct { bad_member ProtocolAlias; };",
           "type Foo = struct { bad_member ResourceStructAlias; };",
           "type Foo = struct { bad_member ResourceTableAlias; };",
           "type Foo = struct { bad_member ResourceUnionAlias; };",
       }) {
    std::string fidl_library = R"FIDL(
library example;
using zx;

alias HandleAlias = zx.Handle;
alias ProtocolAlias = client_end:Protocol;
alias ResourceStructAlias = ResourceStruct;
alias ResourceTableAlias = ResourceStruct;
alias ResourceUnionAlias = ResourceStruct;

protocol Protocol {};
type ResourceStruct = resource struct {};
type ResourceTable = resource table {};
type ResourceUnion = resource union { 1: b bool; };

)FIDL" + definition + "\n";
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Foo", "bad_member",
                       "example.fidl:16:21");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ResourcenessTests, BadResourcesInNestedContainers) {
  for (
      const std::string& definition : {
          "type Foo = struct { bad_member vector<vector<zx.Handle>>; };",
          "type Foo = struct { bad_member vector<vector<zx.Handle:optional>>; };",
          "type Foo = struct { bad_member vector<vector<client_end:Protocol>>; };",
          "type Foo = struct { bad_member vector<vector<ResourceStruct>>; };",
          "type Foo = struct { bad_member vector<vector<ResourceTable>>; };",
          "type Foo = struct { bad_member vector<vector<ResourceUnion>>; };",
          "type Foo = struct { bad_member vector<array<vector<ResourceStruct>:optional,2>>:optional; };",
      }) {
    std::string fidl_library = R"FIDL(
library example;
using zx;

protocol Protocol {};
type ResourceStruct = resource struct {};
type ResourceTable = resource table {};
type ResourceUnion = resource union { 1: b bool; };

)FIDL" + definition + "\n";
    TestLibrary library(fidl_library);
    library.UseLibraryZx();
    library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Foo", "bad_member",
                       "example.fidl:10:21");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
}

TEST(ResourcenessTests, BadMultipleResourceTypesInValueType) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type Foo = struct {
  first zx.Handle;
  second zx.Handle:optional;
  third ResourceStruct;
};

type ResourceStruct = resource struct {};
)FIDL");
  library.UseLibraryZx();
  library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Foo", "first",
                     "example.fidl:6:3");
  library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Foo", "second",
                     "example.fidl:7:3");
  library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Foo", "third",
                     "example.fidl:8:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, GoodTransitiveResourceMember) {
  std::string fidl_library = R"FIDL(
library example;

type Top = resource struct {
    middle Middle;
};
type Middle = resource struct {
    bottom Bottom;
};
type Bottom = resource struct {};
)FIDL";

  TestLibrary library(fidl_library);
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupStruct("Top")->resourceness, Resourceness::kResource);
}

TEST(ResourcenessTests, BadTransitiveResourceMember) {
  TestLibrary library(R"FIDL(
library example;

type Top = struct {
  middle Middle;
};
type Middle = struct {
  bottom Bottom;
};
type Bottom = resource struct {};
)FIDL");
  // We only report an error for Middle. Fixing that (if the fix is to make
  // Middle a resource, not remove the bottom field) will cause an error on Top.
  library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Middle", "bottom",
                     "example.fidl:8:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, GoodRecursiveValueTypes) {
  std::string fidl_library = R"FIDL(
library example;

type Ouro = struct {
    b box<Boros>;
};

type Boros = struct {
    o box<Ouro>;
};
)FIDL";

  TestLibrary library(fidl_library);
  ASSERT_COMPILED(library);
}

TEST(ResourcenessTests, GoodRecursiveResourceTypes) {
  std::string fidl_library = R"FIDL(
library example;

type Ouro = resource struct {
    b box<Boros>;
};

type Boros = resource struct {
    o box<Ouro>;
};
)FIDL";

  TestLibrary library(fidl_library);
  ASSERT_COMPILED(library);
}

TEST(ResourcenessTests, BadRecursiveResourceTypes) {
  TestLibrary library(R"FIDL(
library example;

type Ouro = resource struct {
  b box<Boros>;
};

type Boros = struct {
  bad_member box<Ouro>;
};
)FIDL");
  library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "Boros", "bad_member",
                     "example.fidl:9:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ResourcenessTests, GoodStrictResourceOrderIndependent) {
  TestLibrary library(R"FIDL(
library example;

type SR = strict resource union {
    1: b bool;
};
type RS = strict resource union {
    1: b bool;
};
)FIDL");
  ASSERT_COMPILED(library);

  const auto strict_resource = library.LookupUnion("SR");
  EXPECT_EQ(strict_resource->strictness, Strictness::kStrict);
  EXPECT_EQ(strict_resource->resourceness, Resourceness::kResource);

  const auto resource_strict = library.LookupUnion("RS");
  EXPECT_EQ(resource_strict->strictness, Strictness::kStrict);
  EXPECT_EQ(resource_strict->resourceness, Resourceness::kResource);
}

}  // namespace
}  // namespace fidlc
