// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/versioning_types.h"
#include "tools/fidl/fidlc/tests/test_library.h"

// This file tests basic FIDL versioning behavior, such as elements being
// included in the output only between their `added` and `removed` versions.
// Tests are run for all the versions given in INSTANTIATE_TEST_SUITE_P.

namespace fidlc {
namespace {

class VersioningBasicTest : public testing::TestWithParam<TargetVersions> {};

const Version V1 = Version::From(1).value();
const Version V2 = Version::From(2).value();
const Version HEAD = Version::kHead;

const TargetVersions kParamValues[] = {{V1}, {V2}, {HEAD}, {V1, HEAD}, {V1, V2, HEAD}};

INSTANTIATE_TEST_SUITE_P(VersioningBasicTests, VersioningBasicTest, testing::ValuesIn(kParamValues),
                         [](auto info) { return info.param.ToString(); });

TEST_P(VersioningBasicTest, GoodLibraryDefault) {
  TestLibrary library(R"FIDL(
library example;
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
}

TEST_P(VersioningBasicTest, GoodLibraryAddedAtHead) {
  TestLibrary library(R"FIDL(
@available(added=HEAD)
library example;
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
}

TEST_P(VersioningBasicTest, GoodLibraryAddedAtOne) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
}

TEST_P(VersioningBasicTest, GoodLibraryAddedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1, removed=2)
library example;
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
}

TEST_P(VersioningBasicTest, GoodLibraryAddedAndDeprecatedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1, deprecated=2, removed=HEAD)
library example;
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
}

TEST_P(VersioningBasicTest, GoodDeclAddedAtHead) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=HEAD)
type Foo = struct {};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.HasStruct("Foo"), GetParam().Any() >= HEAD);
}

TEST_P(VersioningBasicTest, GoodDeclAddedAtOne) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=1)
type Foo = struct {};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_TRUE(library.HasStruct("Foo"));
}

TEST_P(VersioningBasicTest, GoodDeclAddedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=1, removed=2)
type Foo = struct {};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.HasStruct("Foo"), GetParam().Any() == V1);
}

TEST_P(VersioningBasicTest, GoodDeclAddedAndReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=1, replaced=2)
type Foo = struct {};

@available(added=2)
type Foo = resource struct {};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupStruct("Foo")->resourceness,
            GetParam().All() == V1 ? Resourceness::kValue : Resourceness::kResource);
}

TEST_P(VersioningBasicTest, GoodDeclAddedAndDeprecatedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=1, deprecated=2, removed=HEAD)
type Foo = struct {};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  bool present = GetParam().Any() < HEAD;
  ASSERT_EQ(library.HasStruct("Foo"), present);
  if (present) {
    EXPECT_EQ(library.LookupStruct("Foo")->availability.is_deprecated(), GetParam().Any() >= V2);
  }
}

TEST_P(VersioningBasicTest, GoodMemberAddedAtHead) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(added=HEAD)
    member string;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupStruct("Foo")->members.size(), GetParam().Any() >= HEAD ? 1u : 0u);
}

TEST_P(VersioningBasicTest, GoodMemberAddedAtOne) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(added=1)
    member string;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupStruct("Foo")->members.size(), 1u);
}

TEST_P(VersioningBasicTest, GoodMemberAddedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(added=1, removed=2)
    member string;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  EXPECT_EQ(library.LookupStruct("Foo")->members.size(), GetParam().Any() == V1 ? 1u : 0u);
}

TEST_P(VersioningBasicTest, GoodMemberAddedAndReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(added=1, replaced=2)
    member string;
    @available(added=2)
    member uint32;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  ASSERT_EQ(library.LookupStruct("Foo")->members.size(), 1u);
  EXPECT_EQ(library.LookupStruct("Foo")->members.front().type_ctor->type->kind,
            GetParam().All() == V1 ? Type::Kind::kString : Type::Kind::kPrimitive);
}

TEST_P(VersioningBasicTest, GoodMemberAddedAndDeprecatedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(added=1, deprecated=2, removed=HEAD)
    member string;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  bool present = GetParam().Any() < HEAD;
  ASSERT_EQ(library.LookupStruct("Foo")->members.size(), present ? 1u : 0u);
  if (present) {
    EXPECT_EQ(library.LookupStruct("Foo")->members.front().availability.is_deprecated(),
              GetParam().Any() >= V2);
  }
}

TEST_P(VersioningBasicTest, GoodDeclStrictnessAddedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = strict(added=2, removed=3) flexible(added=3) enum {
    VALUE = 1;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  // Foo is flexible by default at V1, strict at V2, and explicitly flexible from V3 onwards.
  EXPECT_EQ(library.LookupEnum("Foo")->strictness.value(),
            GetParam().Any() == V2 && GetParam().All() <= V2 ? Strictness::kStrict
                                                             : Strictness::kFlexible);
}

TEST_P(VersioningBasicTest, GoodDeclResourcenessAddedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

type Foo = resource(added=2, removed=3) struct {};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  // Foo is a resource type at V2 only.
  EXPECT_EQ(library.LookupStruct("Foo")->resourceness.value(),
            GetParam().Any() == V2 && GetParam().All() <= V2 ? Resourceness::kResource
                                                             : Resourceness::kValue);
}

TEST_P(VersioningBasicTest, GoodProtocolOpennessAddedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

closed(added=2, removed=3) open(added=3) protocol Foo {};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  // Foo is open by default at V1, explicitly closed at V2, and explicitly open from V3 onwards.
  EXPECT_EQ(library.LookupProtocol("Foo")->openness.value(),
            GetParam().Any() == V2 && GetParam().All() <= V2 ? Openness::kClosed : Openness::kOpen);
}

TEST_P(VersioningBasicTest, GoodMethodStrictnessAddedAndRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

open protocol Protocol {
    strict(added=2, removed=3) flexible(added=3) Foo();
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  // Foo is flexible by default at V1, strict at V2, and explicitly flexible from V3 onwards.
  EXPECT_EQ(library.LookupProtocol("Protocol")->methods[0].strictness.value(),
            GetParam().Any() == V2 && GetParam().All() <= V2 ? Strictness::kStrict
                                                             : Strictness::kFlexible);
}

TEST_P(VersioningBasicTest, BadChangeInteractingModifiers) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

closed(removed=2) ajar(added=2, removed=3) open(added=3) protocol Protocol {
    OneWay();
    -> Event();
    TwoWay() -> () error uint32;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.ExpectFail(ErrFlexibleOneWayMethodInClosedProtocol, Protocol::Method::Kind::kOneWay);
  library.ExpectFail(ErrFlexibleOneWayMethodInClosedProtocol, Protocol::Method::Kind::kEvent);
  library.ExpectFail(ErrFlexibleTwoWayMethodRequiresOpenProtocol, Openness::kClosed);
  library.ExpectFail(ErrFlexibleTwoWayMethodRequiresOpenProtocol, Openness::kAjar);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST_P(VersioningBasicTest, GoodChangeInteractingModifiers) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

closed(removed=2) ajar(added=2, removed=3) open(added=3) protocol Protocol {
    strict(removed=2) flexible(added=2) OneWay();
    strict(removed=2) flexible(added=2) -> Event();
    strict(removed=3) flexible(added=3) TwoWay() -> () error uint32;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
}

TEST_P(VersioningBasicTest, GoodAddResourceModifierAndHandle) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

using zx;

type Foo = resource(added=2) table {
    @available(added=2)
    1: handle zx.Handle;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.UseLibraryZx();
  ASSERT_COMPILED(library);
}

// You remove the `resource` modifier and handle fields at the same time, but
// that prevents you from targeting a set with versions both before and after.
// In other words, you can't treat something as a value type if you still want
// to support older versions where it had handle fields.
TEST_P(VersioningBasicTest, RemoveResourceModifierAndHandle) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

using zx;

type Foo = resource(removed=2) table {
    @available(removed=2)
    1: handle zx.Handle;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  library.UseLibraryZx();
  bool has_handle = GetParam().Any() == V1;
  bool is_resource = GetParam().All() == V1;
  if (has_handle && !is_resource) {
    library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kTable, "Foo", "handle",
                       "example.fidl:9:8");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  } else {
    ASSERT_COMPILED(library);
  }
}

TEST_P(VersioningBasicTest, BadReferenceOutsideAvailability) {
  TestLibrary library;
  library.AddFile("bad/fi-0220.test.fidl");
  library.SelectVersions("test", GetParam());
  library.ExpectFail(ErrNameNotFoundInVersionRange, "Bar", "library 'test.bad.fi0220'",
                     VersionRange(V1, V2), "from version 2 onward (bad/fi-0220.test.fidl:10:6)");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

// TODO(https://fxbug.dev/42052719): Generalize this with more comprehensive tests in
// versioning_interleaving_tests.cc.
TEST_P(VersioningBasicTest, GoodRegularDeprecatedReferencesVersionedDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@deprecated
const FOO uint32 = BAR;
@available(deprecated=1)
const BAR uint32 = 1;
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
}

// Previously this errored due to incorrect logic in deprecation checks.
TEST_P(VersioningBasicTest, GoodDeprecationLogicRegression1) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(deprecated=1, removed=3)
type Foo = struct {};

@available(deprecated=1, removed=3)
type Bar = struct {
    foo Foo;
    @available(added=2)
    ensure_split_at_v2 string;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  bool present = GetParam().Any() <= V2;
  ASSERT_EQ(library.HasStruct("Bar"), present);
  if (present) {
    EXPECT_EQ(library.LookupStruct("Bar")->members.size(), GetParam().All() == V1 ? 1u : 2u);
  }
}

// Previously this crashed due to incorrect logic in deprecation checks.
TEST_P(VersioningBasicTest, GoodDeprecationLogicRegression2) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(deprecated=1)
type Foo = struct {};

@available(deprecated=1, removed=3)
type Bar = struct {
    foo Foo;
    @available(added=2)
    ensure_split_at_v2 string;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  bool present = GetParam().Any() <= V2;
  ASSERT_EQ(library.HasStruct("Bar"), present);
  if (present) {
    EXPECT_EQ(library.LookupStruct("Bar")->members.size(), GetParam().All() == V1 ? 1u : 2u);
  }
}

TEST_P(VersioningBasicTest, GoodMultipleFiles) {
  TestLibrary library;
  library.AddSource("overview.fidl", R"FIDL(
/// Some doc comment.
@available(added=1)
library example;
)FIDL");
  library.AddSource("first.fidl", R"FIDL(
library example;

@available(added=2)
type Foo = struct {
    bar box<Bar>;
};
)FIDL");
  library.AddSource("second.fidl", R"FIDL(
library example;

@available(added=2)
type Bar = struct {
    foo box<Foo>;
};
)FIDL");
  library.SelectVersions("example", GetParam());
  ASSERT_COMPILED(library);
  ASSERT_EQ(library.HasStruct("Foo"), GetParam().Any() >= V2);
  ASSERT_EQ(library.HasStruct("Bar"), GetParam().Any() >= V2);
}

TEST_P(VersioningBasicTest, GoodMultipleLibraries) {
  SharedAmongstLibraries shared;
  shared.SelectVersions("platform", GetParam());
  TestLibrary dependency(&shared, "dependency.fidl", R"FIDL(
@available(added=1)
library platform.dependency;

type Foo = struct {
    @available(added=2)
    member string;
};
)FIDL");
  ASSERT_COMPILED(dependency);
  TestLibrary example(&shared, "example.fidl", R"FIDL(
@available(added=1)
library platform.example;

using platform.dependency;

type ShouldBeSplit = struct {
    foo platform.dependency.Foo;
};
)FIDL");
  ASSERT_COMPILED(example);
  ASSERT_EQ(example.LookupStruct("ShouldBeSplit")->type_shape->inline_size,
            GetParam().All() == V1 ? 1u : 16u);
}

}  // namespace
}  // namespace fidlc
