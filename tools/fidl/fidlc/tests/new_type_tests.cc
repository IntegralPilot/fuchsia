// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/experimental_flags.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(NewTypeTests, GoodNewTypes) {
  TestLibrary library(R"FIDL(
library example;

type Foo = struct {
  bytes vector<uint8>;
};

type OpaqueFoo = Foo;

type Bar = enum {
  PARALLEL = 0;
  PERPENDICULAR = 1;
};

type OpaqueBar = Bar;
)FIDL");

  library.EnableFlag(ExperimentalFlag::kAllowNewTypes);
  ASSERT_COMPILED(library);
}

TEST(NewTypeTests, GoodNewTypesResourceness) {
  TestLibrary library(R"FIDL(
library example;

type A = resource struct {};
type B = A;
type C = resource struct { b B; };
)FIDL");

  library.EnableFlag(ExperimentalFlag::kAllowNewTypes);
  ASSERT_COMPILED(library);
}

TEST(NewTypeTests, BadNewTypesResourceness) {
  TestLibrary library(R"FIDL(
library example;

type A = resource struct {};
type B = A;
type C = struct { b B; };
)FIDL");

  library.EnableFlag(ExperimentalFlag::kAllowNewTypes);
  library.ExpectFail(ErrTypeMustBeResource, Decl::Kind::kStruct, "C", "b", "example.fidl:6:19");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(NewTypeTests, GoodNewTypesSimple) {
  TestLibrary library(R"FIDL(
library example;

type Bits = bits { A = 1; };
type Enum = enum {
  A = 1;
  B = 15;
};
type Struct = struct { foo string; };
type Table = table {};
type Union = union { 1: b bool; };
alias Alias = Struct;

// Now for the new-types
type NewBits = Bits;
type NewEnum = Enum;
type NewStruct = Struct;
type NewTable = Table;
type NewUnion = Union;
type NewAlias = Alias;
)FIDL");

  library.EnableFlag(ExperimentalFlag::kAllowNewTypes);
  ASSERT_COMPILED(library);
}

TEST(NewTypeTests, GoodNewTypesBuiltin) {
  TestLibrary library(R"FIDL(
library example;
using zx;

type Struct = struct {};
protocol Protocol {};

type NewBool = bool;
type NewInt = int16;
type NewString = string;
type NewArray = array<int8, 3>;
type NewVector = vector<bool>;
type NewBox = box<Struct>;
type NewHandle = zx.Handle;
type NewClientEnd = client_end:Protocol;
type NewServerEnd = server_end:Protocol;
)FIDL");

  library.EnableFlag(ExperimentalFlag::kAllowNewTypes);
  library.UseLibraryZx();
  ASSERT_COMPILED(library);
}

TEST(NewTypeTests, GoodNewTypesComplex) {
  TestLibrary library(R"FIDL(
library example;

type Struct = struct {};
type NewStruct = Struct;
type DoubleNewStruct = NewStruct;
)FIDL");

  library.EnableFlag(ExperimentalFlag::kAllowNewTypes);
  ASSERT_COMPILED(library);
}

TEST(NewTypeTests, GoodNewTypesConstrained) {
  TestLibrary library(R"FIDL(
library example;

type ConstrainedVec = vector<int32>:<5, optional>;
type ConstrainedString = string:108;
)FIDL");

  library.EnableFlag(ExperimentalFlag::kAllowNewTypes);
  ASSERT_COMPILED(library);
}

TEST(NewTypeTests, BadNewTypesConstraints) {
  TestLibrary library;
  library.AddFile("bad/fi-0179.test.fidl");
  library.EnableFlag(ExperimentalFlag::kAllowNewTypes);
  library.ExpectFail(ErrNewTypeCannotHaveConstraint, "Name");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
