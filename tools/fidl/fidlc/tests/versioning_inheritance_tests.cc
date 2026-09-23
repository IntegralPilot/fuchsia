// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

// This file tests the inheritance of the @available attribute and ways the
// attribute can conflict with inherited values.

namespace fidlc {
namespace {

TEST(VersioningInheritanceTests, GoodRedundantWithParent) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(added=2, deprecated=4, removed=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "2");
  ASSERT_COMPILED(library);
}

TEST(VersioningInheritanceTests, BadAddedBeforeParentAdded) {
  TestLibrary library;
  library.AddFile("bad/fi-0155-a.test.fidl");
  library.SelectVersion("test", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "added", "1", "added", "2",
                     "bad/fi-0155-a.test.fidl:4:12", "added", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, GoodAddedWhenParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(added=4)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "4");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_TRUE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, GoodAddedAfterParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(added=5)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "5");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_TRUE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, BadAddedWhenParentRemoved) {
  TestLibrary library;
  library.AddFile("bad/fi-0155-b.test.fidl");
  library.SelectVersion("test", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "added", "4", "removed", "4",
                     "bad/fi-0155-b.test.fidl:4:35", "added", "after", "removed");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadAddedWhenParentReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, replaced=6)
type Foo = struct {
    @available(added=6)
    member bool;
};

@available(added=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "added", "6", "replaced", "6",
                     "example.fidl:5:35", "added", "after", "replaced");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadAddedAfterParentRemoved) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(added=7)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "added", "7", "removed", "6",
                     "example.fidl:2:35", "added", "after", "removed");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadAddedAfterParentReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, replaced=6)
type Foo = struct {
    @available(added=7)
    member bool;
};

@available(added=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "added", "7", "replaced", "6",
                     "example.fidl:5:35", "added", "after", "replaced");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadDeprecatedBeforeParentAdded) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(deprecated=1)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "deprecated", "1", "added", "2",
                     "example.fidl:2:12", "deprecated", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, GoodDeprecatedWhenParentAdded) {
  TestLibrary library(R"FIDL(
@available(added=2, removed=6) // never deprecated
library example;

@available(deprecated=2)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "2");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_TRUE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, GoodDeprecatedBeforeParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(deprecated=3)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "3");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_TRUE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, BadDeprecatedAfterParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(deprecated=5)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "deprecated", "5", "deprecated", "4",
                     "example.fidl:2:21", "deprecated", "after", "deprecated");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadDeprecatedWhenParentRemoved) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(deprecated=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "deprecated", "6", "removed", "6",
                     "example.fidl:2:35", "deprecated", "after", "removed");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadDeprecatedWhenParentReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, replaced=6)
type Foo = struct {
    @available(deprecated=6)
    member bool;
};

@available(added=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "deprecated", "6", "replaced", "6",
                     "example.fidl:5:35", "deprecated", "after", "replaced");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadDeprecatedAfterParentRemoved) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(deprecated=7)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "deprecated", "7", "removed", "6",
                     "example.fidl:2:35", "deprecated", "after", "removed");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadDeprecatedAfterParentReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, replaced=6)
type Foo = struct {
    @available(deprecated=7)
    member bool;
};

@available(added=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "deprecated", "7", "replaced", "6",
                     "example.fidl:5:35", "deprecated", "after", "replaced");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadRemovedBeforeParentAdded) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(removed=1)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "removed", "1", "added", "2",
                     "example.fidl:2:12", "removed", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadReplacedBeforeParentAdded) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, removed=6)
type Foo = struct {
    @available(replaced=1)
    member bool;
};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "replaced", "1", "added", "2",
                     "example.fidl:5:12", "replaced", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadRemovedWhenParentAdded) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(removed=2)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "removed", "2", "added", "2",
                     "example.fidl:2:12", "removed", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadReplacedWhenParentAdded) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, removed=6)
type Foo = struct {
    @available(replaced=2)
    member bool;
};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "replaced", "2", "added", "2",
                     "example.fidl:5:12", "replaced", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, GoodRemovedBeforeParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(removed=3)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "2");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_FALSE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, GoodReplacedBeforeParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, removed=6)
type Foo = struct {
    @available(replaced=3)
    member bool;
    @available(added=3)
    member uint32;
};
)FIDL");
  library.SelectVersion("example", "2");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_FALSE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, GoodRemovedWhenParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(removed=4)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "3");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_FALSE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, GoodReplacedWhenParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, removed=6)
type Foo = struct {
    @available(replaced=4)
    member bool;
    @available(added=4)
    member uint32;
};
)FIDL");
  library.SelectVersion("example", "3");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_FALSE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, GoodRemovedAfterParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(removed=5)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "4");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_TRUE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, GoodReplacedAfterParentDeprecated) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, removed=6)
type Foo = struct {
    @available(replaced=5)
    member bool;
    @available(added=5)
    member uint32;
};
)FIDL");
  library.SelectVersion("example", "4");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_TRUE(foo->availability.is_deprecated());
}

TEST(VersioningInheritanceTests, BadRemovedAfterParentRemoved) {
  TestLibrary library(R"FIDL(
@available(added=2, deprecated=4, removed=6)
library example;

@available(removed=7)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "removed", "7", "removed", "6",
                     "example.fidl:2:35", "removed", "after", "removed");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadRemovedAfterParentReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, replaced=6)
type Foo = struct {
    @available(removed=7)
    member bool;
};

@available(added=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "removed", "7", "replaced", "6",
                     "example.fidl:5:35", "removed", "after", "replaced");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadReplacedAfterParentRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, removed=6)
type Foo = struct {
    @available(replaced=7)
    member bool;
};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "replaced", "7", "removed", "6",
                     "example.fidl:5:35", "replaced", "after", "removed");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadReplacedAfterParentReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, replaced=6)
type Foo = struct {
    @available(replaced=7)
    member bool;
};

@available(added=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "replaced", "7", "replaced", "6",
                     "example.fidl:5:35", "replaced", "after", "replaced");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, GoodRemovedWhenParentReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, replaced=6)
type Foo = struct {
    @available(added=2, deprecated=4, removed=6)
    member bool;
};

@available(added=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "6");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_TRUE(foo->members.empty());
}

TEST(VersioningInheritanceTests, BadReplacedWhenParentRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, removed=6)
type Foo = struct {
    @available(added=2, deprecated=4, replaced=6)
    member bool;
};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "replaced", "6", "removed", "6",
                     "example.fidl:5:35", "replaced", "after", "removed");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadReplacedWhenParentReplaced) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=4, replaced=6)
type Foo = struct {
    @available(added=2, deprecated=4, replaced=6)
    member bool;
};

@available(added=6)
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "replaced", "6", "replaced", "6",
                     "example.fidl:5:35", "replaced", "after", "replaced");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, GoodMemberInheritsFromParent) {
  TestLibrary library(R"FIDL(
@available(added=1)
library example;

@available(added=2)
type Foo = struct {
    @available(deprecated=3)
    member1 bool;
};
)FIDL");
  library.SelectVersion("example", "2");
  ASSERT_COMPILED(library);

  auto foo = library.LookupStruct("Foo");
  ASSERT_NE(foo, nullptr);
  EXPECT_EQ(foo->members.size(), 1u);
}

TEST(VersioningInheritanceTests, GoodComplexInheritance) {
  // The following libraries all define a struct Bar with effective availability
  // @available(added=2, deprecated=3, removed=4) in different ways.

  std::vector<const char*> sources;

  // Direct annotation.
  sources.push_back(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=3, removed=4)
type Bar = struct {};
)FIDL");

  // Fully inherit from library declaration.
  sources.push_back(R"FIDL(
@available(added=2, deprecated=3, removed=4)
library example;

type Bar = struct {};
)FIDL");

  // Partially inherit from library declaration.
  sources.push_back(R"FIDL(
@available(added=1, deprecated=3)
library example;

@available(added=2, removed=4)
type Bar = struct {};
)FIDL");

  // Inherit from parent.
  sources.push_back(R"FIDL(
@available(added=1)
library example;

@available(added=2, deprecated=3, removed=4)
type Foo = struct {
    member @generated_name("Bar") struct {};
};
)FIDL");

  // Inherit from member.
  sources.push_back(R"FIDL(
@available(added=1)
library example;

type Foo = struct {
    @available(added=2, deprecated=3, removed=4)
    member @generated_name("Bar") struct {};
};
)FIDL");

  // Inherit from multiple, forward.
  sources.push_back(R"FIDL(
@available(added=2)
library example;

@available(deprecated=3)
type Foo = struct {
    @available(removed=4)
    member @generated_name("Bar") struct {};
};
)FIDL");

  // Inherit from multiple, backward.
  sources.push_back(R"FIDL(
@available(added=1, removed=4)
library example;

@available(deprecated=3)
type Foo = struct {
    @available(added=2)
    member @generated_name("Bar") struct {};
};
)FIDL");

  // Inherit from multiple, mixed.
  sources.push_back(R"FIDL(
@available(added=1)
library example;

@available(added=2)
type Foo = struct {
    @available(deprecated=3, removed=4)
    member @generated_name("Bar") struct {};
};
)FIDL");

  // Inherit via nested layouts.
  sources.push_back(R"FIDL(
@available(added=1)
library example;

@available(added=2)
type Foo = struct {
    @available(deprecated=3)
    member1 struct {
        @available(removed=4)
        member2 struct {
            member3 @generated_name("Bar") struct {};
        };
    };
};
)FIDL");

  // Inherit via nested type constructors.
  sources.push_back(R"FIDL(
@available(added=1)
library example;

@available(added=2)
type Foo = struct {
    @available(deprecated=3, removed=4)
    member1 vector<vector<vector<@generated_name("Bar") struct{}>>>;
};
)FIDL");

  for (std::string version : {"1", "2", "3", "4"}) {
    SCOPED_TRACE(version);
    for (auto& source : sources) {
      TestLibrary library(source);
      library.SelectVersion("example", version);
      ASSERT_COMPILED(library);
      auto bar = library.LookupStruct("Bar");
      EXPECT_EQ(bar != nullptr, version == "2" || version == "3");
      if (bar) {
        EXPECT_EQ(bar->availability.is_deprecated(), version == "3");
      }
    }
  }
}

TEST(VersioningInheritanceTests, BadDeclConflictsWithParent) {
  TestLibrary library(R"FIDL( // L1
@available(added=2)           // L2
library example;              // L3
                              // L4
@available(added=1)           // L5
type Foo = struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "added", "1", "added", "2",
                     "example.fidl:2:12", "added", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.position().line, 5);
}

TEST(VersioningInheritanceTests, BadMemberConflictsWithParent) {
  TestLibrary library(R"FIDL( // L1
@available(added=1)           // L2
library example;              // L3
                              // L4
@available(added=2)           // L5
type Foo = struct {           // L6
    @available(added=1)       // L7
    member1 bool;
};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "added", "1", "added", "2",
                     "example.fidl:5:12", "added", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.position().line, 7);
}

TEST(VersioningInheritanceTests, BadMemberConflictsWithGrandParent) {
  TestLibrary library(R"FIDL( // L1
@available(added=2)           // L2
library example;              // L3
                              // L4
@available(removed=3)         // L5
type Foo = struct {           // L6
    @available(added=1)       // L7
    member1 bool;
};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "added", "1", "added", "2",
                     "example.fidl:2:12", "added", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.position().line, 7);
}

TEST(VersioningInheritanceTests, BadMemberConflictsWithGrandParentThroughAnonymous) {
  TestLibrary library(R"FIDL( // L1
@available(added=1)           // L2
library example;              // L3
                              // L4
@available(added=2)           // L5
type Foo = struct {           // L6
    member1 struct {          // L7
        @available(removed=1) // L8
        member2 bool;
    };
};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "removed", "1", "added", "2",
                     "example.fidl:5:12", "removed", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
  EXPECT_EQ(library.errors()[0]->span.position().line, 8);
}

TEST(VersioningInheritanceTests, BadModifierAddedBeforeParentAdded) {
  TestLibrary library(R"FIDL(
@available(added=2)
library example;

type Foo = resource(added=1) struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "added", "1", "added", "2",
                     "example.fidl:2:12", "added", "before", "added");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(VersioningInheritanceTests, BadModifierRemovedAfterParentRemoved) {
  TestLibrary library(R"FIDL(
@available(added=1, removed=2)
library example;

type Foo = resource(removed=3) struct {};
)FIDL");
  library.SelectVersion("example", "HEAD");
  library.ExpectFail(ErrAvailabilityConflictsWithParent, "removed", "3", "removed", "2",
                     "example.fidl:2:21", "removed", "after", "removed");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
