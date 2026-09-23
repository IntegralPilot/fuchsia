// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

// This file tests ways of overlapping element availabilities.
// Tests are run for all the versions given in INSTANTIATE_TEST_SUITE_P.

namespace fidlc {
namespace {

class VersioningOverlapTest : public testing::TestWithParam<TargetVersions> {
 public:
  // TODO(https://fxbug.dev/42085274): Remove when error messages no longer reference LEGACY.
  std::optional<VersionRange> LegacyRange() {
    if (GetParam().set.size() > 1)
      return VersionRange(Version::kLegacy, Version::kPosInf);
    return std::nullopt;
  }
};

const Version V1 = Version::From(1).value();
const Version V2 = Version::From(2).value();
const Version V3 = Version::From(3).value();
const Version V4 = Version::From(4).value();
const Version V5 = Version::From(5).value();
const Version HEAD = Version::kHead;

const TargetVersions kParamValues[] = {
    {V1}, {V2}, {V3}, {V4}, {V5}, {HEAD}, {V1, V2, V3, V4, V5, HEAD}};

INSTANTIATE_TEST_SUITE_P(VersioningOverlapTests, VersioningOverlapTest,
                         testing::ValuesIn(kParamValues),
                         [](auto info) { return info.param.ToString(); });

TEST_P(VersioningOverlapTest, GoodNoGap) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(replaced=2)
type Foo = struct {};

@available(added=2)
type Foo = resource struct {};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupStruct("Foo")->resourceness,
            GetParam().All() == V1 ? Resourceness::kValue : Resourceness::kResource);
}

TEST_P(VersioningOverlapTest, GoodWithGap) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(removed=2)
type Foo = struct {};

@available(added=3)
type Foo = table {};
)FIDL");
  library.SelectVersions("example", GetParam());
  bool foo_struct = GetParam().Any() == V1;
  bool foo_table = GetParam().Any() >= V3;
  if (foo_struct && foo_table) {
    library.ExpectFail(ErrNameOverlap, Element::Kind::kTable, "Foo", Element::Kind::kStruct,
                       "example.fidl:6:6", VersionSet(LegacyRange().value()),
                       Platform::Parse("example").value());
    ASSERT_COMPILER_DIAGNOSTICS(library);
  } else {
    ASSERT_COMPILED(library);
    EXPECT_EQ(library.HasStruct("Foo"), foo_struct);
    EXPECT_EQ(library.HasTable("Foo"), foo_table);
  }
}

TEST_P(VersioningOverlapTest, GoodNoGapCanonical) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(removed=2)
type foo = struct {};

@available(added=2)
type FOO = table {};
)FIDL");
  library.SelectVersions("example", GetParam());
  bool foo_struct = GetParam().Any() == V1;
  bool foo_table = GetParam().Any() >= V2;
  if (foo_struct && foo_table) {
    library.ExpectFail(ErrNameOverlapCanonical, Element::Kind::kStruct, "foo",
                       Element::Kind::kTable, "FOO", "example.fidl:9:6", "foo",
                       VersionSet(LegacyRange().value()), Platform::Parse("example").value());
    ASSERT_COMPILER_DIAGNOSTICS(library);
  } else {
    ASSERT_COMPILED(library);
    EXPECT_EQ(library.HasStruct("foo"), foo_struct);
    EXPECT_EQ(library.HasTable("FOO"), foo_table);
  }
}

TEST_P(VersioningOverlapTest, GoodWithGapCanonical) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(removed=2)
type foo = struct {};

@available(added=3)
type FOO = table {};

)FIDL");
  library.SelectVersions("example", GetParam());
  bool foo_struct = GetParam().Any() == V1;
  bool foo_table = GetParam().Any() >= V3;
  if (foo_struct && foo_table) {
    library.ExpectFail(ErrNameOverlapCanonical, Element::Kind::kStruct, "foo",
                       Element::Kind::kTable, "FOO", "example.fidl:9:6", "foo",
                       VersionSet(LegacyRange().value()), Platform::Parse("example").value());
    ASSERT_COMPILER_DIAGNOSTICS(library);
  } else {
    ASSERT_COMPILED(library);
    EXPECT_EQ(library.HasStruct("foo"), foo_struct);
    EXPECT_EQ(library.HasTable("FOO"), foo_table);
  }
}

TEST_P(VersioningOverlapTest, BadEqual) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {};
type Foo = table {};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameCollision, Element::Kind::kTable, "Foo", Element::Kind::kStruct,
                     "example.fidl:5:6");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadEqualCanonical) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type foo = struct {};
type FOO = table {};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStruct, "foo",
                     Element::Kind::kTable, "FOO", "example.fidl:6:6", "foo");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadExample) {
  TestLibrary library;
  library.AddFile("bad/fi-0036.test.fidl");
  library.SelectVersions("test", GetParam());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kEnum, "Color", Element::Kind::kEnum,
                     "bad/fi-0036.test.fidl:7:6", VersionSet(VersionRange(V2, Version::kPosInf)),
                     Platform::Parse("test").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadExampleCanonical) {
  TestLibrary library;
  library.AddFile("bad/fi-0037.test.fidl");
  library.SelectVersions("test", GetParam());
  library.ExpectFail(ErrNameOverlapCanonical, Element::Kind::kProtocol, "Color",
                     Element::Kind::kConst, "COLOR", "bad/fi-0037.test.fidl:7:7", "color",
                     VersionSet(VersionRange(V2, Version::kPosInf)),
                     Platform::Parse("test").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadSubset) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {};
@available(removed=2)
type Foo = table {};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kTable, "Foo", Element::Kind::kStruct,
                     "example.fidl:5:6", VersionSet(VersionRange(V1, V2), LegacyRange()),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadSubsetCanonical) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type foo = struct {};
@available(removed=2)
type FOO = table {};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlapCanonical, Element::Kind::kStruct, "foo", Element::Kind::kTable,
                     "FOO", "example.fidl:7:6", "foo",
                     VersionSet(VersionRange(V1, V2), LegacyRange()),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadIntersect) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(removed=5)
type Foo = struct {};
@available(added=3)
type Foo = table {};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kTable, "Foo", Element::Kind::kStruct,
                     "example.fidl:6:6", VersionSet(VersionRange(V3, V5), LegacyRange()),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadIntersectCanonical) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(removed=5)
type foo = struct {};
@available(added=3)
type FOO = table {};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlapCanonical, Element::Kind::kStruct, "foo", Element::Kind::kTable,
                     "FOO", "example.fidl:8:6", "foo",
                     VersionSet(VersionRange(V3, V5), LegacyRange()),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadMultiple) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {};
@available(added=3)
type Foo = table {};
@available(added=HEAD)
const Foo uint32 = 0;
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kStruct, "Foo", Element::Kind::kConst,
                     "example.fidl:9:7", VersionSet(VersionRange(Version::kHead, Version::kPosInf)),
                     Platform::Parse("example").value());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kTable, "Foo", Element::Kind::kStruct,
                     "example.fidl:5:6", VersionSet(VersionRange(V3, Version::kPosInf)),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadRecursive) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=1, removed=5)
type Foo = struct { member box<Foo>; };

@available(added=3, removed=7)
type Foo = struct { member box<Foo>; };
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kStruct, "Foo", Element::Kind::kStruct,
                     "example.fidl:6:6", VersionSet(VersionRange(V3, V5), LegacyRange()),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadMemberEqual) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    member bool;
    member bool;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameCollision, Element::Kind::kStructMember, "member",
                     Element::Kind::kStructMember, "example.fidl:6:5");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadMemberEqualCanonical) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    member bool;
    MEMBER bool;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameCollisionCanonical, Element::Kind::kStructMember, "MEMBER",
                     Element::Kind::kStructMember, "member", "example.fidl:6:5", "member");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadMemberSubset) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    member bool;
    @available(removed=2)
    member bool;
};
)FIDL");

  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kStructMember, "member",
                     Element::Kind::kStructMember, "example.fidl:6:5",
                     VersionSet(VersionRange(V1, V2), LegacyRange()),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadMemberSubsetCanonical) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    member bool;
    @available(removed=2)
    MEMBER bool;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlapCanonical, Element::Kind::kStructMember, "MEMBER",
                     Element::Kind::kStructMember, "member", "example.fidl:6:5", "member",
                     VersionSet(VersionRange(V1, V2), LegacyRange()),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadMemberIntersect) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(removed=5)
    member bool;
    @available(added=3)
    member bool;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kStructMember, "member",
                     Element::Kind::kStructMember, "example.fidl:7:5",
                     VersionSet(VersionRange(V3, V5), LegacyRange()),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadMemberIntersectCanonical) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(removed=5)
    member bool;
    @available(added=3)
    MEMBER bool;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlapCanonical, Element::Kind::kStructMember, "MEMBER",
                     Element::Kind::kStructMember, "member", "example.fidl:7:5", "member",
                     VersionSet(VersionRange(V3, V5), LegacyRange()),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadMemberMultiple) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    member bool;
    @available(added=3)
    member bool;
    @available(added=HEAD)
    member bool;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kStructMember, "member",
                     Element::Kind::kStructMember, "example.fidl:6:5",
                     VersionSet(VersionRange(V3, Version::kPosInf)),
                     Platform::Parse("example").value());
  library.ExpectFail(ErrNameOverlap, Element::Kind::kStructMember, "member",
                     Element::Kind::kStructMember, "example.fidl:6:5",
                     VersionSet(VersionRange(Version::kHead, Version::kPosInf)),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadMemberMultipleCanonical) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    member bool;
    @available(added=3)
    Member bool;
    @available(added=HEAD)
    MEMBER bool;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrNameOverlapCanonical, Element::Kind::kStructMember, "Member",
                     Element::Kind::kStructMember, "member", "example.fidl:6:5", "member",
                     VersionSet(VersionRange(V3, Version::kPosInf)),
                     Platform::Parse("example").value());
  library.ExpectFail(ErrNameOverlapCanonical, Element::Kind::kStructMember, "MEMBER",
                     Element::Kind::kStructMember, "member", "example.fidl:6:5", "member",
                     VersionSet(VersionRange(Version::kHead, Version::kPosInf)),
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadModifierEqual) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict strict enum {
    VALUE = 1;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrDuplicateModifier, "modifier 'strict'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadModifierEqualConflict) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict flexible enum {
    VALUE = 1;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrConflictingModifier, "modifier 'flexible'", "modifier 'strict'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadModifierSubset) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict strict(removed=2) enum {
    VALUE = 1;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrDuplicateModifier, "modifier 'strict'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadModifierSubsetConflict) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict flexible(removed=2) enum {
    VALUE = 1;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrConflictingModifier, "modifier 'flexible'", "modifier 'strict'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadModifierIntersect) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict(removed=5) strict(added=3) enum {
    VALUE = 1;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrDuplicateModifier, "modifier 'strict'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadModifierIntersectConflict) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict(removed=5) flexible(added=3) enum {
    VALUE = 1;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrConflictingModifier, "modifier 'flexible'", "modifier 'strict'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadModifierMultiple) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict strict(added=3) strict(added=HEAD) enum {
    VALUE = 1;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrDuplicateModifier, "modifier 'strict'");
  library.ExpectFail(ErrDuplicateModifier, "modifier 'strict'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningOverlapTest, BadModifierMultipleConflict) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict flexible(added=3) strict(added=HEAD) enum {
    VALUE = 1;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrConflictingModifier, "modifier 'flexible'", "modifier 'strict'");
  library.ExpectFail(ErrDuplicateModifier, "modifier 'strict'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
