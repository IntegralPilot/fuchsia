// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/src/names.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(UsingTests, GoodUsing) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependent.fidl", R"FIDL(
library dependent;

type Bar = struct {
    s int8;
};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared);
  library.AddFile("good/fi-0178.test.fidl");

  ASSERT_COMPILED(library);
}

TEST(UsingTests, GoodUsingAlias) {
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

using dependent as the_alias;

type Foo = struct {
    dep1 the_alias.Bar;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(UsingTests, GoodUsingSwapNames) {
  SharedAmongstLibraries shared;
  TestLibrary dependency1(&shared, "dependent1.fidl", R"FIDL(library dependent1;

const C1 bool = false;
)FIDL");
  ASSERT_COMPILED(dependency1);
  TestLibrary dependency2(&shared, "dependent2.fidl", R"FIDL(library dependent2;

const C2 bool = false;
)FIDL");
  ASSERT_COMPILED(dependency2);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using dependent1 as dependent2;
using dependent2 as dependent1;

const C1 bool = dependent2.C1;
const C2 bool = dependent1.C2;
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(UsingTests, GoodDeclWithSameNameAsAliasedLibrary) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dep.fidl", R"FIDL(
library dep;

type A = struct{};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "lib.fidl",
                      R"FIDL(
library lib;

using dep as depnoconflict;

type dep = struct {};

type B = struct{a depnoconflict.A;}; // So the import is used.
)FIDL");

  ASSERT_COMPILED(library);
}

TEST(UsingTests, BadMissingUsing) {
  TestLibrary library(R"FIDL(
library example;

// missing using.

type Foo = struct {
    dep dependent.Bar;
};
)FIDL");
  library.ExpectFail(ErrNameNotFound, "dependent", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadUnknownUsing) {
  TestLibrary library;
  library.AddFile("bad/fi-0046.test.fidl");
  library.ExpectFail(ErrUnknownLibrary, "dependent");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadUsingAliasRefThroughFqn) {
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

using dependent as the_alias;

type Foo = struct {
    dep1 dependent.Bar;
};
)FIDL");
  library.ExpectFail(ErrNameNotFound, "dependent", "library 'example'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadDuplicateUsingNoAlias) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared);
  dependency.AddFile("bad/fi-0042-a.test.fidl");
  ASSERT_COMPILED(dependency);
  TestLibrary library(&shared);
  library.AddFile("bad/fi-0042-b.test.fidl");
  library.ExpectFail(ErrDuplicateLibraryImport, "test.bad.fi0042a");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadDuplicateUsingFirstAlias) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependent.fidl", R"FIDL(
library dependent;
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using dependent as alias;
using dependent; // duplicated
)FIDL");
  library.ExpectFail(ErrDuplicateLibraryImport, "dependent");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadDuplicateUsingSecondAlias) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependent.fidl", R"FIDL(
library dependent;
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using dependent;
using dependent as alias; // duplicated
)FIDL");
  library.ExpectFail(ErrDuplicateLibraryImport, "dependent");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadDuplicateUsingSameLibrarySameAlias) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependent.fidl", R"FIDL(
library dependent;
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using dependent as alias;
using dependent as alias; // duplicated
)FIDL");
  library.ExpectFail(ErrDuplicateLibraryImport, "dependent");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadDuplicateUsingSameLibraryDifferentAlias) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependent.fidl", R"FIDL(
library dependent;
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using dependent as alias1;
using dependent as alias2; // duplicated
)FIDL");
  library.ExpectFail(ErrDuplicateLibraryImport, "dependent");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadConflictingUsingLibraryAndAlias) {
  SharedAmongstLibraries shared;
  TestLibrary dependency1(&shared, "dependent1.fidl", R"FIDL(library dependent1;
)FIDL");
  ASSERT_COMPILED(dependency1);
  TestLibrary dependency2(&shared, "dependent2.fidl", R"FIDL(library dependent2;
)FIDL");
  ASSERT_COMPILED(dependency2);

  TestLibrary library(&shared, "example.fidl", R"FIDL(
library example;

using dependent1;
using dependent2 as dependent1; // conflict
)FIDL");
  library.ExpectFail(ErrConflictingLibraryImportAlias, "dependent2", "dependent1");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadConflictingUsingAliasAndLibrary) {
  SharedAmongstLibraries shared;
  TestLibrary dependency1(&shared);
  dependency1.AddFile("bad/fi-0043-a.test.fidl");
  ASSERT_COMPILED(dependency1);
  TestLibrary dependency2(&shared);
  dependency2.AddFile("bad/fi-0043-b.test.fidl");
  ASSERT_COMPILED(dependency2);

  TestLibrary library(&shared);
  library.AddFile("bad/fi-0043-c.test.fidl");
  library.ExpectFail(ErrConflictingLibraryImport, "fi0043b");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadConflictingUsingAliasAndAlias) {
  SharedAmongstLibraries shared;
  TestLibrary dependency1(&shared);
  dependency1.AddFile("bad/fi-0044-a.test.fidl");
  ASSERT_COMPILED(dependency1);
  TestLibrary dependency2(&shared);
  dependency2.AddFile("bad/fi-0044-b.test.fidl");
  ASSERT_COMPILED(dependency2);

  TestLibrary library(&shared);
  library.AddFile("bad/fi-0044-c.test.fidl");
  library.ExpectFail(ErrConflictingLibraryImportAlias, "test.bad.fi0044b", "dep");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadUnusedUsing) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dependent.fidl", R"FIDL(
library dependent;
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared);
  library.AddFile("bad/fi-0178.test.fidl");

  library.ExpectFail(ErrUnusedImport, "library 'test.bad.fi0178'", "library 'dependent'");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadUnknownDependentLibrary) {
  TestLibrary library;
  library.AddFile("bad/fi-0051.test.fidl");

  library.ExpectFail(ErrUnknownDependentLibrary, "unknown.dependent.library", "unknown.dependent");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadTooManyProvidedLibraries) {
  SharedAmongstLibraries shared;

  TestLibrary dependency(&shared, "notused.fidl", "library not.used;");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "example.fidl", "library example;");
  ASSERT_COMPILED(library);

  auto unused = shared.all_libraries()->Unused();
  ASSERT_EQ(unused.size(), 1u);
  ASSERT_EQ((*unused.begin())->name, "not.used");
}

TEST(UsingTests, BadLibraryDeclarationNameCollision) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared);
  dependency.AddFile("bad/fi-0038-a.test.fidl");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared);
  library.AddFile("bad/fi-0038-b.test.fidl");

  library.ExpectFail(ErrDeclNameConflictsWithLibraryImport, "dependency");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(UsingTests, BadAliasedLibraryDeclarationNameCollision) {
  SharedAmongstLibraries shared;
  TestLibrary dependency(&shared, "dep.fidl", R"FIDL(
library dep;

type A = struct{};
)FIDL");
  ASSERT_COMPILED(dependency);

  TestLibrary library(&shared, "lib.fidl",
                      R"FIDL(
library lib;

using dep as x;

type x = struct{};

type B = struct{a dep.A;}; // So the import is used.
)FIDL");

  library.ExpectFail(ErrDeclNameConflictsWithLibraryImport, "x");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
