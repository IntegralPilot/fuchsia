// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

std::vector<std::string_view> DirectAndComposedDependencies(const TestLibrary& library) {
  std::vector<std::string_view> names;
  for (auto& dep : library.direct_and_composed_dependencies())
    names.push_back(dep.library->name);
  return names;
}

TEST(DirectDependenciesTests, GoodDirectDepsSimple) {
  for (const std::string& type_usage : {
           "dep2.Type",
           "vector<dep2.Type>",
           "array<dep2.Type, 1>",
           "box<dep2.Type>",
           "client_end:dep2.Protocol",
           "server_end:dep2.Protocol",
           "vector<uint32>:dep2.Constant",
           "array<uint32, dep2.Constant>",
       }) {
    SharedAmongstLibraries shared;
    TestLibrary dep2(&shared, "dep2.fidl", R"FIDL(
library dep2;

const Constant uint32 = 50;
type Type = struct {};
protocol Protocol {};
)FIDL");
    ASSERT_COMPILED(dep2);

    TestLibrary dep1(&shared, "dep1.fidl",
                     R"FIDL(
library dep1;

using dep2;

protocol ComposedProtocol {
  UsesDep2(resource struct { data )FIDL" +
                         type_usage + R"FIDL(; });
};
)FIDL");
    ASSERT_COMPILED(dep1);

    TestLibrary lib(&shared, "example.fidl", R"FIDL(
library example;

using dep1;

protocol CapturesDependencyThroughCompose {
  compose dep1.ComposedProtocol;
};
)FIDL");
    ASSERT_COMPILED(lib);

    std::vector<std::string_view> expected = {"dep1", "dep2"};
    EXPECT_EQ(DirectAndComposedDependencies(lib), expected);
  }
}

// TODO(https://fxbug.dev/42158155): Reconsider this. Will likely need to follow aliases,
// which would mean expecting the result to include both dep1 and dep2 here.
TEST(DirectDependenciesTests, GoodDoesNotFollowAlias) {
  SharedAmongstLibraries shared;
  TestLibrary dep2(&shared, "dep2.fidl", R"FIDL(
library dep2;

type Foo = struct {};
)FIDL");
  ASSERT_COMPILED(dep2);

  TestLibrary dep1(&shared, "dep1.fidl", R"FIDL(
library dep1;

using dep2;

alias Bar = dep2.Foo;

protocol ComposedProtocol {
  UsesDep2InAlias(struct { foo vector<Bar>; });
};
)FIDL");
  ASSERT_COMPILED(dep1);

  TestLibrary lib(&shared, "example.fidl", R"FIDL(
library example;

using dep1;

protocol CapturesDependencyThroughCompose {
  compose dep1.ComposedProtocol;
};
)FIDL");
  ASSERT_COMPILED(lib);

  std::vector<std::string_view> expected = {"dep1"};
  EXPECT_EQ(DirectAndComposedDependencies(lib), expected);
}

TEST(DirectDependenciesTests, GoodDoesNotFollowNestedStruct) {
  SharedAmongstLibraries shared;
  TestLibrary dep2(&shared, "dep2.fidl", R"FIDL(
library dep2;

type Foo = struct {};
)FIDL");
  ASSERT_COMPILED(dep2);

  TestLibrary dep1(&shared, "dep1.fidl", R"FIDL(
library dep1;

using dep2;

type Bar = struct {
  foo dep2.Foo;
};

protocol ComposedProtocol {
  UsesDep2InNestedStruct(struct { foo vector<Bar>; });
};
)FIDL");
  ASSERT_COMPILED(dep1);

  TestLibrary lib(&shared, "example.fidl", R"FIDL(
library example;

using dep1;

protocol CapturesDependencyThroughCompose {
  compose dep1.ComposedProtocol;
};
)FIDL");
  ASSERT_COMPILED(lib);

  std::vector<std::string_view> expected = {"dep1"};
  EXPECT_EQ(DirectAndComposedDependencies(lib), expected);
}

TEST(DirectDependenciesTests, GoodErrorSyntaxSuccessType) {
  SharedAmongstLibraries shared;
  TestLibrary dep2(&shared, "dep2.fidl", R"FIDL(
library dep2;

type Foo = struct {};
)FIDL");
  ASSERT_COMPILED(dep2);

  TestLibrary dep1(&shared, "dep1.fidl", R"FIDL(
library dep1;

using dep2;

protocol ComposedProtocol {
  UsesDep2InSuccessType() -> (struct { foo vector<dep2.Foo>; }) error uint32;
};
)FIDL");
  ASSERT_COMPILED(dep1);

  TestLibrary lib(&shared, "example.fidl", R"FIDL(
library example;

using dep1;

protocol CapturesDependencyThroughCompose {
  compose dep1.ComposedProtocol;
};
)FIDL");
  ASSERT_COMPILED(lib);

  std::vector<std::string_view> expected = {"dep1", "dep2"};
  EXPECT_EQ(DirectAndComposedDependencies(lib), expected);
}

TEST(DirectDependenciesTests, GoodErrorSyntaxErrorType) {
  SharedAmongstLibraries shared;
  TestLibrary dep2(&shared, "dep2.fidl", R"FIDL(
library dep2;

type Foo = flexible enum : uint32 {};
)FIDL");
  ASSERT_COMPILED(dep2);

  TestLibrary dep1(&shared, "dep1.fidl", R"FIDL(
library dep1;

using dep2;

protocol ComposedProtocol {
  UsesDep2InErrorType() -> () error dep2.Foo;
};
)FIDL");
  ASSERT_COMPILED(dep1);

  TestLibrary lib(&shared, "example.fidl", R"FIDL(
library example;

using dep1;

protocol CapturesDependencyThroughCompose {
  compose dep1.ComposedProtocol;
};
)FIDL");
  ASSERT_COMPILED(lib);

  std::vector<std::string_view> expected = {"dep1", "dep2"};
  EXPECT_EQ(DirectAndComposedDependencies(lib), expected);
}

TEST(DirectDependenciesTests, GoodFlexibleResponse) {
  SharedAmongstLibraries shared;
  TestLibrary dep2(&shared, "dep2.fidl", R"FIDL(
library dep2;

type Foo = struct {};
)FIDL");
  ASSERT_COMPILED(dep2);

  TestLibrary dep1(&shared, "dep1.fidl", R"FIDL(
library dep1;

using dep2;

open protocol ComposedProtocol {
  flexible UsesDep2InFlexibleResponse() -> (struct { foo vector<dep2.Foo>; });
};
)FIDL");
  ASSERT_COMPILED(dep1);

  TestLibrary lib(&shared, "example.fidl", R"FIDL(
library example;

using dep1;

open protocol CapturesDependencyThroughCompose {
  compose dep1.ComposedProtocol;
};
)FIDL");
  ASSERT_COMPILED(lib);

  std::vector<std::string_view> expected = {"dep1", "dep2"};
  EXPECT_EQ(DirectAndComposedDependencies(lib), expected);
}

}  // namespace
}  // namespace fidlc
