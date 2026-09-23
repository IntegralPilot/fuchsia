// Copyright 2020 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/diagnostics.h"
#include "tools/fidl/fidlc/src/flat_ast.h"
#include "tools/fidl/fidlc/tests/test_library.h"

namespace fidlc {
namespace {

TEST(FlexibleTests, BadEnumMultipleUnknown) {
  TestLibrary library;
  library.AddFile("bad/fi-0072.test.fidl");
  library.ExpectFail(ErrUnknownAttributeOnMultipleEnumMembers);
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(FlexibleTests, BadEnumMaxValueWithoutUnknownUnsigned) {
  {
    TestLibrary library;
    library.AddFile("bad/fi-0068.test.fidl");
    library.ExpectFail(ErrFlexibleEnumMemberWithMaxValue, "255");
    ASSERT_COMPILER_DIAGNOSTICS(library);
  }
  {
    TestLibrary library;
    library.AddFile("good/fi-0068-a.test.fidl");
    ASSERT_COMPILED(library);
  }
  {
    TestLibrary library;
    library.AddFile("good/fi-0068-b.test.fidl");
    ASSERT_COMPILED(library);
  }
}

TEST(FlexibleTests, BadEnumMaxValueWithoutUnknownSigned) {
  TestLibrary library(R"FIDL(
library example;

type Foo = flexible enum : int8 {
  ZERO = 0;
  ONE = 1;
  MAX = 127;
};
)FIDL");
  library.ExpectFail(ErrFlexibleEnumMemberWithMaxValue, "127");
  ASSERT_COMPILER_DIAGNOSTICS(library);
}

TEST(FlexibleTests, GoodEnumCanUseMaxValueIfOtherIsUnknownUnsigned) {
  TestLibrary library(R"FIDL(
library example;

type Foo = flexible enum : uint8 {
    ZERO = 0;
    @unknown
    ONE = 1;
    MAX = 255;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto foo_enum = library.LookupEnum("Foo");
  ASSERT_NE(foo_enum, nullptr);
  EXPECT_FALSE(foo_enum->unknown_value_signed.has_value());
  EXPECT_TRUE(foo_enum->unknown_value_unsigned.has_value());
  EXPECT_EQ(foo_enum->unknown_value_unsigned.value(), 1u);
}

TEST(FlexibleTests, GoodEnumCanUseMaxValueIfOtherIsUnknownSigned) {
  TestLibrary library(R"FIDL(
library example;

type Foo = flexible enum : int8 {
    ZERO = 0;
    @unknown
    ONE = 1;
    MAX = 127;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto foo_enum = library.LookupEnum("Foo");
  ASSERT_NE(foo_enum, nullptr);
  EXPECT_TRUE(foo_enum->unknown_value_signed.has_value());
  EXPECT_EQ(foo_enum->unknown_value_signed.value(), 1u);
  EXPECT_FALSE(foo_enum->unknown_value_unsigned.has_value());
}

TEST(FlexibleTests, GoodEnumCanUseZeroAsUnknownValue) {
  TestLibrary library(R"FIDL(
library example;

type Foo = flexible enum : int8 {
    @unknown
    ZERO = 0;
    ONE = 1;
    MAX = 127;
};
)FIDL");
  ASSERT_COMPILED(library);

  auto foo_enum = library.LookupEnum("Foo");
  ASSERT_NE(foo_enum, nullptr);
  EXPECT_TRUE(foo_enum->unknown_value_signed.has_value());
  EXPECT_EQ(foo_enum->unknown_value_signed.value(), 0u);
  EXPECT_FALSE(foo_enum->unknown_value_unsigned.has_value());
}

}  // namespace
}  // namespace fidlc
