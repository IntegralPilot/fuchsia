// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

const char* good_library_source = R"FIDL(
library example;

type S = struct {
    arr string_array<10>;
};
)FIDL";

TEST(StringArrayTests, GoodNonzeroSizeArray) {
  TestLibrary library(good_library_source);
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  ASSERT_COMPILED(library);
}

TEST(StringArrayTests, BadNoExperimentalFlag) {
  TestLibrary library(good_library_source);
  library.ExpectFail(ErrExperimentalZxCTypesDisallowed, "string_array");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(StringArrayTests, BadZeroSizeArray) {
  TestLibrary library(R"FIDL(
library example;

type S = struct {
    arr string_array<0>;
};
)FIDL");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  library.ExpectFail(ErrMustHaveNonZeroSize, "string_array");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(StringArrayTests, BadNoSizeArray) {
  TestLibrary library(R"FIDL(
library example;

type S = struct {
    arr string_array;
};
)FIDL");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  library.ExpectFail(ErrWrongNumberOfLayoutParameters, "string_array", 1, 0);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(StringArrayTests, BadOptionalArray) {
  TestLibrary library(R"FIDL(
library example;

type S = struct {
    arr string_array<10>:optional;
};
)FIDL");
  library.EnableFlag(ExperimentalFlag::kZxCTypes);
  library.ExpectFail(ErrCannotBeOptional, "string_array");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

}  // namespace
}  // namespace fidlc
