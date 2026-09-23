// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

// This file tests how FIDL versioning distinguishes "removal" from
// "replacement", and validation that replacement is done correctly.
// Tests are run for all the versions given in INSTANTIATE_TEST_SUITE_P.

namespace fidlc {
namespace {

class VersioningReplacementTest : public testing::TestWithParam<Version> {};

const Version V1 = Version::From(1).value();
const Version V2 = Version::From(2).value();
const Version V3 = Version::From(3).value();

INSTANTIATE_TEST_SUITE_P(VersioningReplacementTests, VersioningReplacementTest,
                         testing::Values(V1, V2, V3),
                         [](auto info) { return info.param.ToString(); });

TEST_P(VersioningReplacementTest, GoodDeclRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(removed=2)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.HasStruct("Foo"), GetParam() == V1);
}

TEST_P(VersioningReplacementTest, BadDeclRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(removed=2)
type Foo = struct {};

@available(added=2)
type Foo = resource struct {};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "struct 'Foo'", V2, "example.fidl:9:6");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodDeclReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(replaced=2)
type Foo = struct {};

@available(added=2)
type Foo = resource struct {};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupStruct("Foo")->resourceness,
            GetParam() == V1 ? Resourceness::kValue : Resourceness::kResource);
}

TEST_P(VersioningReplacementTest, BadDeclReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(replaced=2)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidReplaced, "struct 'Foo'", V2);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodMemberRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(removed=2)
    1: bar string;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupTable("Foo")->members.size(), GetParam() == V1 ? 1u : 0u);
}

TEST_P(VersioningReplacementTest, BadMemberRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(removed=2)
    1: bar string;
    @available(added=2)
    1: bar uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "table member 'bar'", V2, "example.fidl:9:8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodMemberReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(replaced=2)
    1: bar string;
    @available(added=2)
    1: bar uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupTable("Foo")->members.front().type_ctor->type->kind,
            GetParam() == V1 ? Type::Kind::kString : Type::Kind::kPrimitive);
}

TEST_P(VersioningReplacementTest, BadMemberReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(replaced=2)
    1: bar string;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidReplaced, "table member 'bar'", V2);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodMethodRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Foo {
    @available(removed=2)
    Bar();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Foo")->methods.size(), GetParam() == V1 ? 1u : 0u);
}

TEST_P(VersioningReplacementTest, BadMethodRemoved) {
  TestLibrary library;
  library.SelectVersion("test", GetParam());
  library.AddFile("bad/fi-0205.test.fidl");
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'Bar'", V2, "bad/fi-0205.test.fidl:11:14");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodMethodReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Foo {
    @available(replaced=2)
    strict Bar();
    @available(added=2)
    flexible Bar();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Foo")->methods[0].strictness,
            GetParam() == V1 ? Strictness::kStrict : Strictness::kFlexible);
}

TEST_P(VersioningReplacementTest, BadMethodReplaced) {
  TestLibrary library;
  library.AddFile("bad/fi-0206.test.fidl");
  library.SelectVersion("test", GetParam());
  library.ExpectFail(ErrInvalidReplaced, "protocol method 'Bar'", V2);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadMethodRemovedNewCompose) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(removed=2)
  Method();

  @available(added=2)
  compose Base;
};

protocol Base {
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'Method'", V2, "example.fidl:15:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodMethodReplacedNewCompose) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(replaced=2)
  Method();

  @available(added=2)
  compose Base;
};

protocol Base {
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Protocol")->all_methods[0].composed != nullptr,
            GetParam() >= V2);
}

TEST_P(VersioningReplacementTest, BadMethodRemovedExistingCompose) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(removed=2)
  Method();

  compose Base;
};

protocol Base {
  @available(added=2)
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'Method'", V2, "example.fidl:15:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodMethodReplacedExistingCompose) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(replaced=2)
  Method();

  compose Base;
};

protocol Base {
  @available(added=2)
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Protocol")->all_methods[0].composed != nullptr,
            GetParam() >= V2);
}

TEST_P(VersioningReplacementTest, BadComposeRemovedNewMethod) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(removed=2)
  compose Base;

  @available(added=2)
  Method();
};

protocol Base {
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'Method'", V2, "example.fidl:10:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodComposeReplacedNewMethod) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(replaced=2)
  compose Base;

  @available(added=2)
  Method();
};

protocol Base {
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Protocol")->all_methods[0].composed != nullptr,
            GetParam() == V1);
}

TEST_P(VersioningReplacementTest, BadComposeRemovedNewMethodComposeeSimultaneouslyRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(removed=2)
  compose Base;

  @available(added=2)
  Method();
};

@available(removed=2)
protocol Base {
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'Method'", V2, "example.fidl:10:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodComposeReplacedNewMethodComposeeSimultaneouslyRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(replaced=2)
  compose Base;

  @available(added=2)
  Method();
};

@available(removed=2)
protocol Base {
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Protocol")->all_methods[0].composed != nullptr,
            GetParam() == V1);
}

TEST_P(VersioningReplacementTest, BadComposeRemovedNewCompose) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(removed=2)
  compose Foo;

  @available(added=2)
  compose Bar;
};

protocol Foo {
  @selector("selector/for.Method")
  Method();
};

protocol Bar {
  @selector("selector/for.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'Method'", V2, "example.fidl:20:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodComposeReplacedNewCompose) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(replaced=2)
  compose Foo;

  @available(added=2)
  compose Bar;
};

protocol Foo {
  @selector("selector/for.Method")
  Method();
};

protocol Bar {
  @selector("selector/for.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Protocol")->all_methods[0].owning_protocol->GetName(),
            GetParam() == V1 ? "Foo" : "Bar");
}

TEST_P(VersioningReplacementTest, BadComposeRemovedNewComposeComposeeSimultaneouslyRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(removed=2)
  compose Foo;

  @available(added=2)
  compose Bar;
};

@available(removed=2)
protocol Foo {
  @selector("selector/for.Method")
  Method();
};

protocol Bar {
  @selector("selector/for.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'Method'", V2, "example.fidl:21:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodComposeReplacedNewComposeComposeeSimultaneouslyRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(replaced=2)
  compose Foo;

  @available(added=2)
  compose Bar;
};

@available(removed=2)
protocol Foo {
  @selector("selector/for.Method")
  Method();
};

protocol Bar {
  @selector("selector/for.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Protocol")->all_methods[0].owning_protocol->GetName(),
            GetParam() == V1 ? "Foo" : "Bar");
}

// No "Good" version because it would have to be replaced in Base (already
// tested by BadMethodRemoved), causing a name collision with Protocol.Method.
TEST_P(VersioningReplacementTest, BadComposedMethodRemovedNewMethod) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  compose Base;

  @available(added=2)
  Method();
};

protocol Base {
  @available(removed=2)
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'Method'", V2, "example.fidl:9:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadMethodRemovedTransitiveCompose) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(removed=2)
  Method();

  compose Intermediate;
};

protocol Intermediate {
    @available(added=2)
    compose Base;
};

protocol Base {
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'Method'", V2, "example.fidl:19:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodMethodReplacedTransitiveCompose) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(replaced=2)
  Method();

  compose Intermediate;
};

protocol Intermediate {
    @available(added=2)
    compose Base;
};

protocol Base {
  @selector("example/Protocol.Method")
  Method();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Protocol")->all_methods[0].owning_protocol->GetName(),
            GetParam() == V1 ? "Protocol" : "Base");
}

TEST_P(VersioningReplacementTest, BadMethodAndComposeRemovedHybrid) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(removed=2)
  compose AB;
  @available(removed=2)
  @selector("selector/for.C")
  C();

  @available(added=2)
  @selector("selector/for.A")
  A();
  @available(added=2)
  compose BC;
};

protocol AB {
  @selector("selector/for.A")
  A();
  @selector("selector/for.B")
  B();
};

protocol BC {
  @selector("selector/for.B")
  B();
  @selector("selector/for.C")
  C();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'B'", V2, "example.fidl:28:3");
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'A'", V2, "example.fidl:14:3");
  library.ExpectFail(ErrInvalidRemoved, "protocol method 'C'", V2, "example.fidl:30:3");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodMethodAndComposeReplacedHybrid) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

protocol Protocol {
  @available(replaced=2)
  compose AB;
  @available(replaced=2)
  @selector("selector/for.C")
  C();

  @available(added=2)
  @selector("selector/for.A")
  A();
  @available(added=2)
  compose BC;
};

protocol AB {
  @selector("selector/for.A")
  A();
  @selector("selector/for.B")
  B();
};

protocol BC {
  @selector("selector/for.B")
  B();
  @selector("selector/for.C")
  C();
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupProtocol("Protocol")->all_methods.size(), 3u);
}

TEST_P(VersioningReplacementTest, GoodReplacedTwice) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(replaced=2)
type Foo = struct {};

@available(added=2, replaced=3)
type Foo = struct {};

@available(added=3)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
}

TEST_P(VersioningReplacementTest, GoodAllDeclsReplacedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(replaced=2)
const CONST uint32 = 1;
@available(added=2, removed=3)
const CONST uint32 = 1;

@available(replaced=2)
alias Alias = string;
@available(added=2, removed=3)
alias Alias = string;

// TODO(https://fxbug.dev/42158155): Uncomment.
// @available(replaced=2)
// type Type = string;
// @available(added=2, removed=2)
// type Type = string;

@available(replaced=2)
type Bits = bits {};
@available(added=2, removed=3)
type Bits = bits {};

@available(replaced=2)
type Enum = enum {};
@available(added=2, removed=3)
type Enum = enum {};

@available(replaced=2)
type Struct = struct {};
@available(added=2, removed=3)
type Struct = struct {};

@available(replaced=2)
type Table = table {};
@available(added=2, removed=3)
type Table = table {};

@available(replaced=2)
type Union = union {};
@available(added=2, removed=3)
type Union = union {};

@available(replaced=2)
protocol Protocol {};
@available(added=2, removed=3)
protocol Protocol {};

@available(replaced=2)
service Service {};
@available(added=2, removed=3)
service Service {};

@available(replaced=2)
resource_definition Resource : uint32 { properties { subtype flexible enum : uint32 {}; }; };
@available(added=2, removed=3)
resource_definition Resource : uint32 { properties { subtype flexible enum : uint32 {}; }; };
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.declaration_order().size(), GetParam() <= V2 ? 11u : 0u);
}

TEST_P(VersioningReplacementTest, GoodAllMembersReplacedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Bits = bits {
    @available(replaced=2)
    MEMBER = 1;
    @available(added=2, removed=3)
    MEMBER = 1;
};

type Enum = enum {
    @available(replaced=2)
    MEMBER = 1;
    @available(added=2, removed=3)
    MEMBER = 1;
};

type Struct = struct {
    @available(replaced=2)
    member uint32;
    @available(added=2, removed=3)
    member uint32;
};

type Table = table {
    @available(replaced=2)
    1: member uint32;
    @available(added=2, removed=3)
    1: member uint32;
};

type Union = union {
    @available(replaced=2)
    1: member uint32;
    @available(added=2, removed=3)
    1: member uint32;
};

protocol Protocol {
    @available(replaced=2)
    Member();
    @available(added=2, removed=3)
    Member();
};

service Service {
    @available(replaced=2)
    member client_end:Protocol;
    @available(added=2, removed=3)
    member client_end:Protocol;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  auto num_members = GetParam() <= V2 ? 1u : 0u;
  EXPECT_EQ(library.LookupBits("Bits")->members.size(), num_members);
  EXPECT_EQ(library.LookupEnum("Enum")->members.size(), num_members);
  EXPECT_EQ(library.LookupStruct("Struct")->members.size(), num_members);
  EXPECT_EQ(library.LookupTable("Table")->members.size(), num_members);
  EXPECT_EQ(library.LookupUnion("Union")->members.size(), num_members);
  EXPECT_EQ(library.LookupProtocol("Protocol")->methods.size(), num_members);
  EXPECT_EQ(library.LookupService("Service")->members.size(), num_members);
}

TEST_P(VersioningReplacementTest, GoodAllMembersReplacedAndRenamed) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Bits = bits {
    @available(replaced=2, renamed="NEW")
    OLD = 1;
    @available(added=2)
    NEW = 1;
};

type Enum = enum {
    @available(replaced=2, renamed="NEW")
    OLD = 1;
    @available(added=2)
    NEW = 1;
};

type Struct = struct {
    @available(replaced=2, renamed="new")
    old uint32;
    @available(added=2)
    new uint32;
};

type Table = table {
    @available(replaced=2, renamed="new")
    1: old uint32;
    @available(added=2)
    1: new uint32;
};

type Union = union {
    @available(replaced=2, renamed="new")
    1: old uint32;
    @available(added=2)
    1: new uint32;
};

protocol Protocol {
    @available(replaced=2, renamed="New")
    Old();
    @available(added=2)
    @selector("Old")
    New();
};

service Service {
    @available(replaced=2, renamed="new")
    old client_end:Protocol;
    @available(added=2)
    new client_end:Protocol;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);

  EXPECT_EQ(library.LookupBits("Bits")->members.size(), 1u);
  EXPECT_EQ(library.LookupEnum("Enum")->members.size(), 1u);
  EXPECT_EQ(library.LookupStruct("Struct")->members.size(), 1u);
  EXPECT_EQ(library.LookupTable("Table")->members.size(), 1u);
  EXPECT_EQ(library.LookupUnion("Union")->members.size(), 1u);
  EXPECT_EQ(library.LookupProtocol("Protocol")->methods.size(), 1u);
  EXPECT_EQ(library.LookupService("Service")->members.size(), 1u);

  bool old = GetParam() == V1;
  EXPECT_EQ(library.LookupBits("Bits")->members[0].GetName(), old ? "OLD" : "NEW");
  EXPECT_EQ(library.LookupEnum("Enum")->members[0].GetName(), old ? "OLD" : "NEW");
  EXPECT_EQ(library.LookupStruct("Struct")->members[0].GetName(), old ? "old" : "new");
  EXPECT_EQ(library.LookupTable("Table")->members[0].GetName(), old ? "old" : "new");
  EXPECT_EQ(library.LookupUnion("Union")->members[0].GetName(), old ? "old" : "new");
  EXPECT_EQ(library.LookupProtocol("Protocol")->methods[0].GetName(), old ? "Old" : "New");
  EXPECT_EQ(library.LookupService("Service")->members[0].GetName(), old ? "old" : "new");
}

TEST_P(VersioningReplacementTest, BadRemovedNamedToAnonymous) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(removed=2)
type Foo = struct {};

type Bar = struct {
    @available(added=2)
    foo struct {};
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemoved, "struct 'Foo'", V2, "example.fidl:10:9");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, GoodRemovedAnonymousToNamed) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Bar = struct {
    // The anonymous type "Foo" inherits removed=2, but removed/replaced
    // does not apply to inherited availabilities.
    @available(removed=2)
    foo struct {};
};

@available(added=2)
type Foo = table {};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.HasStruct("Foo"), GetParam() == V1);
  EXPECT_EQ(library.HasTable("Foo"), GetParam() >= V2);
}

TEST_P(VersioningReplacementTest, GoodRemovedAnonymousToAnonymous) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Bar1 = struct {
    // The anonymous type "Foo" inherits removed=2, but removed/replaced
    // does not apply to inherited availabilities.
    @available(removed=2)
    foo struct {};
};

type Bar2 = struct {
    @available(added=2)
    foo table {};
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.HasStruct("Foo"), GetParam() == V1);
  EXPECT_EQ(library.HasTable("Foo"), GetParam() >= V2);
}

TEST_P(VersioningReplacementTest, GoodReplacedNamedToAnonymous) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(replaced=2)
type Foo = struct {};

type Bar = struct {
    @available(added=2)
    foo table {};
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.HasStruct("Foo"), GetParam() == V1);
  EXPECT_EQ(library.HasTable("Foo"), GetParam() >= V2);
}

TEST_P(VersioningReplacementTest, GoodReplacedAnonymousToNamed) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Bar = struct {
    @available(replaced=2)
    foo struct {};
    @available(added=2)
    foo string;
};

@available(added=2)
type Foo = table {};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.HasStruct("Foo"), GetParam() == V1);
  EXPECT_EQ(library.HasTable("Foo"), GetParam() >= V2);
}

TEST_P(VersioningReplacementTest, GoodReplacedAnonymousToAnonymous) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Bar1 = struct {
    @available(replaced=2)
    foo struct {};
    @available(added=2)
    foo string;
};

type Bar2 = struct {
    @available(added=2)
    foo table {};
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.HasStruct("Foo"), GetParam() == V1);
  EXPECT_EQ(library.HasTable("Foo"), GetParam() >= V2);
}

TEST_P(VersioningReplacementTest, GoodReplacedAnonymousToNothing) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Bar = struct {
    // The anonymous type "Foo" inherits replaced=2, but removed/replaced
    // validation does not apply to inherited availabilities.
    @available(replaced=2)
    foo struct {};
    @available(added=2)
    foo string;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.HasStruct("Foo"), GetParam() == V1);
}

TEST_P(VersioningReplacementTest, GoodMemberRemovedAndRenamed) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(removed=2, renamed="old_bar")
    1: bar string;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  auto foo = library.LookupTable("Foo");
  EXPECT_EQ(foo->members.size(), GetParam() == V1 ? 1u : 0u);
  // TODO(https://fxbug.dev/42085274): Assert "old_bar" exists when targeting both 1 and >1.
  if (GetParam() == V1) {
    EXPECT_EQ(foo->members[0].GetName(), "bar");
  }
}

TEST_P(VersioningReplacementTest, GoodMemberRemovedAndRenamedNameReused) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(removed=2, renamed="old_bar")
    1: bar string;
    @available(added=2)
    2: bar uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  ASSERT_COMPILED(library);
  auto foo = library.LookupTable("Foo");
  EXPECT_EQ(foo->members.size(), 1u);
  // TODO(https://fxbug.dev/42085274): Assert "old_bar" exists when targeting both 1 and >1.
  EXPECT_EQ(foo->members[0].type_ctor->type->kind,
            GetParam() == V1 ? Type::Kind::kString : Type::Kind::kPrimitive);
}

TEST_P(VersioningReplacementTest, BadMemberRemovedAndRenamedNameAlreadyUsedExisting) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(removed=2, renamed="old_bar")
    1: bar string;
    2: old_bar uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemovedAndRenamed, "table member 'bar'", V2, "old_bar",
                     "example.fidl:8:8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadMemberRemovedAndRenamedNameAlreadyUsedNew) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(removed=2, renamed="old_bar")
    1: bar string;
    @available(added=2)
    2: old_bar uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemovedAndRenamed, "table member 'bar'", V2, "old_bar",
                     "example.fidl:9:8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadMethodRemovedAndRenamed) {
  TestLibrary library;
  library.AddFile("bad/fi-0214.test.fidl");
  library.SelectVersion("test", GetParam());
  library.ExpectFail(ErrInvalidRemovedAndRenamed, "protocol method 'OldName'", V2, "NewName",
                     "bad/fi-0214.test.fidl:12:14");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadMethodReplacedAndRenamed) {
  TestLibrary library;
  library.AddFile("bad/fi-0215.test.fidl");
  library.SelectVersion("test", GetParam());
  library.ExpectFail(ErrInvalidReplacedAndRenamed, "protocol method 'OldName'", V2, "NewName");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadBitsMemberRemovedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = bits {
    @available(removed=2)
    A = 1;
    @available(added=2)
    B = 1;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemovedAbi, "bits member 'A'", V2, AbiKind::kValue, 1,
                     "example.fidl:9:5", "B");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadBitsMemberReplacedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = bits {
    @available(replaced=2)
    A = 1;
    @available(added=2)
    A = 2;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidReplacedAbi, "bits member 'A'", V2, AbiKind::kValue, 1, 2,
                     "example.fidl:9:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadEnumMemberRemovedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = enum {
    @available(removed=2)
    A = 1;
    @available(added=2)
    B = 1;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemovedAbi, "enum member 'A'", V2, AbiKind::kValue, 1,
                     "example.fidl:9:5", "B");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadEnumMemberReplacedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = enum {
    @available(replaced=2)
    A = 1;
    @available(added=2)
    A = 2;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidReplacedAbi, "enum member 'A'", V2, AbiKind::kValue, 1, 2,
                     "example.fidl:9:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadStructMemberRemovedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(removed=2)
    bar uint32;
    @available(added=2)
    baz uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemovedAbi, "struct member 'bar'", V2, AbiKind::kOffset, 0,
                     "example.fidl:9:5", "baz");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadStructMemberReplacedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(replaced=2)
    bar uint32;
    gap uint32;
    @available(added=2)
    bar uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidReplacedAbi, "struct member 'bar'", V2, AbiKind::kOffset, 0, 4,
                     "example.fidl:10:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadTableMemberRemovedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(removed=2)
    1: bar uint32;
    @available(added=2)
    1: baz uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemovedAbi, "table member 'bar'", V2, AbiKind::kOrdinal, 1,
                     "example.fidl:9:8", "baz");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadTableMemberReplacedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = table {
    @available(replaced=2)
    1: bar uint32;
    @available(added=2)
    2: bar uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidReplacedAbi, "table member 'bar'", V2, AbiKind::kOrdinal, 1, 2,
                     "example.fidl:9:8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadUnionMemberRemovedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = union {
    @available(removed=2)
    1: bar uint32;
    @available(added=2)
    1: baz uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemovedAbi, "union member 'bar'", V2, AbiKind::kOrdinal, 1,
                     "example.fidl:9:8", "baz");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadUnionMemberReplacedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = union {
    @available(replaced=2)
    1: bar uint32;
    @available(added=2)
    2: bar uint32;
};
)FIDL");
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidReplacedAbi, "union member 'bar'", V2, AbiKind::kOrdinal, 1, 2,
                     "example.fidl:9:8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadOverlayMemberRemovedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict overlay {
    @available(removed=2)
    1: bar uint32;
    @available(added=2)
    1: baz uint32;
};
)FIDL");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidRemovedAbi, "overlay member 'bar'", V2, AbiKind::kOrdinal, 1,
                     "example.fidl:9:8", "baz");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadOverlayMemberReplacedAbi) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict overlay {
    @available(replaced=2)
    1: bar uint32;
    @available(added=2)
    2: bar uint32;
};
)FIDL");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  library.SelectVersion("example", GetParam());
  library.ExpectFail(ErrInvalidReplacedAbi, "overlay member 'bar'", V2, AbiKind::kOrdinal, 1, 2,
                     "example.fidl:9:8");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadMethodRemovedAbi) {
  TestLibrary library;
  library.AddFile("bad/fi-0216.test.fidl");
  library.SelectVersion("test", GetParam());
  library.ExpectFail(ErrInvalidRemovedAbi, "protocol method 'Bar'", V2, AbiKind::kSelector,
                     "test.bad.fi0216/Foo.Bar", "bad/fi-0216.test.fidl:13:14", "Qux");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningReplacementTest, BadMethodReplacedAbi) {
  TestLibrary library;
  library.AddFile("bad/fi-0217.test.fidl");
  library.SelectVersion("test", GetParam());
  library.ExpectFail(ErrInvalidReplacedAbi, "protocol method 'Bar'", V2, AbiKind::kSelector,
                     "test.bad.fi0217/Foo.Bar", "test.bad.fi0217/Foo.NotBar",
                     "bad/fi-0217.test.fidl:13:14");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
