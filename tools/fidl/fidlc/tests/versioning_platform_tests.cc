// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

// This file tests the behavior of platforms in FIDL versioning.

namespace fidlc {
namespace {

TEST(VersioningPlatformTests, GoodUnversionedOneComponent) {
  TestLibrary library(R"FIDL(
library example;
)FIDL");
  ASSERT_COMPILED(library);
  EXPECT_TRUE(library.platform().is_unversioned());
}

TEST(VersioningPlatformTests, GoodUnversionedTwoComponents) {
  TestLibrary library(R"FIDL(
library example.something;
)FIDL");
  ASSERT_COMPILED(library);
  EXPECT_TRUE(library.platform().is_unversioned());
}

TEST(VersioningPlatformTests, GoodImplicitOneComponent) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;
)FIDL");
  library.SelectVersion("example", "HEAD");
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.platform(), Platform::Parse("example").value());
  EXPECT_FALSE(library.platform().is_unversioned());
}

TEST(VersioningPlatformTests, GoodImplicitTwoComponents) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example.something;
)FIDL");
  library.SelectVersion("example", "HEAD");
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.platform(), Platform::Parse("example").value());
  EXPECT_FALSE(library.platform().is_unversioned());
}

TEST(VersioningPlatformTests, GoodExplicit) {
  TestLibrary library(R"FIDL(
@available(platform="someplatform", added=HEAD)
library example;
)FIDL");
  library.SelectVersion("someplatform", "HEAD");
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.platform(), Platform::Parse("someplatform").value());
  EXPECT_FALSE(library.platform().is_unversioned());
}

TEST(VersioningPlatformTests, BadInvalid) {
  TestLibrary library;
  library.AddFile("bad/fi-0152.test.fidl");
  library.SelectVersion("test", "HEAD");
  library.ExpectFail(ErrInvalidPlatform, "Spaces are not allowed");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningPlatformTests, BadReserved) {
  TestLibrary library;
  library.AddFile("bad/fi-0208.test.fidl");
  library.ExpectFail(ErrReservedPlatform, "unversioned");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningPlatformTests, BadExplicitNoVersionSelected) {
  TestLibrary library;
  library.AddFile("bad/fi-0201.test.fidl");
  library.ExpectFail(ErrPlatformVersionNotSelected, "library 'test.bad.fi0201'",
                     Platform::Parse("foo").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningPlatformTests, BadImplicitNoVersionSelected) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example.something;
)FIDL");
  library.ExpectFail(ErrPlatformVersionNotSelected, "library 'example.something'",
                     Platform::Parse("example").value());
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningPlatformTests, GoodMultipleBasic) {
  SharedAmongstLibraries shared;
  shared.SelectVersion("dependency", "3");
  shared.SelectVersion("example", "HEAD");

  TestLibrary dependency(&shared, "dependency.fidl", R"FIDL(
@available(added=2)
library dependency;

@available(added=3, deprecated=4, removed=5)
type Foo = struct {};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary example(&shared, "example.fidl", R"FIDL(
@available(added=1)
library example;

using dependency;

type Foo = struct {
    @available(deprecated=5)
    dep dependency.Foo;
};
)FIDL");
  ASSERT_COMPILED(example);
}

TEST(VersioningPlatformTests, GoodMultipleExplicit) {
  SharedAmongstLibraries shared;
  shared.SelectVersion("xyz", "3");
  shared.SelectVersion("example", "HEAD");

  TestLibrary dependency(&shared, "dependency.fidl", R"FIDL(
@available(platform="xyz", added=1)
library dependency;

@available(added=3, removed=4)
type Foo = struct {};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary example(&shared, "example.fidl", R"FIDL(
@available(added=1)
library example;

using dependency;

alias Foo = dependency.Foo;
)FIDL");
  ASSERT_COMPILED(example);
}

TEST(VersioningPlatformTests, GoodMultipleUsesCorrectDecl) {
  SharedAmongstLibraries shared;
  shared.SelectVersion("dependency", "4");
  shared.SelectVersion("example", "1");

  TestLibrary dependency(&shared, "dependency.fidl", R"FIDL(
@available(added=2)
library dependency;

@available(deprecated=3, replaced=4)
type Foo = resource struct {};

@available(added=4, removed=5)
type Foo = struct {};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary example(&shared, "example.fidl", R"FIDL(
@available(added=1)
library example;

using dependency;

type Foo = struct {
    dep dependency.Foo;
};
)FIDL");
  ASSERT_COMPILED(example);

  auto foo = example.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  ASSERT_EQ(foo->members.size(), 1u);
  auto member_type = foo->members[0].type_ctor->type;
  ASSERT_EQ(member_type->kind, Type::Kind::kIdentifier);
  auto identifier_type = static_cast<const IdentifierType*>(member_type);
  EXPECT_EQ(identifier_type->type_decl->kind, Decl::Kind::kStruct);
  auto struct_decl = static_cast<const Struct*>(identifier_type->type_decl);
  EXPECT_EQ(struct_decl->resourceness, Resourceness::kValue);
}

TEST(VersioningPlatformTests, BadMultipleNameNotFound) {
  SharedAmongstLibraries shared;
  shared.SelectVersion("dependency", "HEAD");
  shared.SelectVersion("example", "HEAD");

  TestLibrary dependency(&shared, "dependency.fidl", R"FIDL(
@available(added=2)
library dependency;

@available(added=3, removed=5)
type Foo = struct {};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary example(&shared, "example.fidl", R"FIDL(
@available(added=1)
library example;

using dependency;

type Foo = struct {
    @available(deprecated=5)
    dep dependency.Foo;
};
)FIDL");
  example.ExpectFail(ErrNameNotFoundInVersionRange, "Foo", "library 'dependency'",
                     "at version HEAD", "from version 3 to 4 (dependency.fidl:6:6)");
  example.ExpectFail(ErrNameNotFoundInVersionRange, "Foo", "library 'dependency'",
                     "at version HEAD", "from version 3 to 4 (dependency.fidl:6:6)");
  ASSERT_COMPILER_DIAGNOSTICS(example);
}

TEST(VersioningPlatformTests, GoodMixVersionedAndUnversioned) {
  SharedAmongstLibraries shared;
  shared.SelectVersion("example", "1");

  TestLibrary versioned(&shared, "versioned.fidl", R"FIDL(
@available(added=1, removed=2)
library example.versioned;

type Foo = struct {};
)FIDL");
  ASSERT_COMPILED(versioned);

  TestLibrary not_versioned(&shared, "not_versioned.fidl", R"FIDL(
library example.notversioned;

using example.versioned;

alias Foo = example.versioned.Foo;
)FIDL");
  ASSERT_COMPILED(not_versioned);

  // The example.notversioned library is considered added=HEAD in the special
  // "unversioned" platform. (If it were instead in the "example" platform, it
  // would appear empty because we're using `--available example:1`.)
  ASSERT_NE(not_versioned.LookupAlias("Foo"), nullptr);
}

}  // namespace
}  // namespace fidlc
