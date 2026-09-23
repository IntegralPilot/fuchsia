// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(ArrayTests, GoodNonzeroSizeArray) {
  TestLibrary library(R"FIDL(
library example;

type S = struct {
    arr array<uint8, 1>;
};
)FIDL");
  ASSERT_COMPILED(library);
}

TEST(ArrayTests, BadZeroSizeArray) {
  TestLibrary library;
  library.AddFile("bad/fi-0161.test.fidl");
  library.ExpectFail(ErrMustHaveNonZeroSize, "array");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ArrayTests, BadNoSizeArray) {
  TestLibrary library(R"FIDL(
library example;

type S = struct {
    arr array<uint8>;
};
)FIDL");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "array", 2, 1);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ArrayTests, BadNonParameterizedArray) {
  TestLibrary library(R"FIDL(
library example;

type S = struct {
    arr array;
};
)FIDL");
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "array", 2, 0);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ArrayTests, BadOptionalArray) {
  TestLibrary library(R"FIDL(
library example;

type S = struct {
    arr array<uint8, 10>:optional;
};
)FIDL");
  library.ExpectFail(ErrCannotBeOptional, "array");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(ArrayTests, BadMultipleConstraintsOnArray) {
  TestLibrary library(R"FIDL(
library example;

type S = struct {
    arr array<uint8, 10>:<1, 2, 3>;
};
)FIDL");
  library.ExpectFail(ErrTooManyConstraints, "array", 1, 3);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
