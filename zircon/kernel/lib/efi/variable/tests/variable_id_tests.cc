// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <algorithm>
#include <list>

#include <efi/variable/variable_id.h>
#include <fbl/vector.h>
#include <gtest/gtest.h>

namespace {

using ::testing::Test;

using namespace efi;

TEST(VariableIdTest, DefaultInvalid) { EXPECT_FALSE(VariableId().IsValid()); }

TEST(VariableIdTest, InvalidateResult) {
  VariableId id({String(u"test"), {}});
  id.Invalidate();
  EXPECT_FALSE(id.IsValid());
}

class VariableIdFixture : public ::testing::TestWithParam<std::list<VariableId>> {};

TEST_P(VariableIdFixture, Eq) {
  std::list<VariableId> input = GetParam();
  for (const auto& a : input) {
    EXPECT_EQ(a, a);
  }
}

TEST_P(VariableIdFixture, Ne) {
  std::list<VariableId> input = GetParam();
  for (auto a = input.begin(); a != input.end(); a++) {
    for (auto b = std::next(a); b != input.end(); b++) {
      EXPECT_NE(*a, *b);
    }
  }
}

TEST_P(VariableIdFixture, LtTrue) {
  std::list<VariableId> input = GetParam();
  for (auto a = input.begin(); a != input.end(); a++) {
    for (auto b = std::next(a); b != input.end(); b++) {
      EXPECT_LT(*a, *b);
    }
  }
}

TEST_P(VariableIdFixture, LtFalse) {
  std::list<VariableId> input = GetParam();
  for (auto a = input.begin(); a != input.end(); a++) {
    EXPECT_FALSE(*a < *a);
  }
}

constexpr efi_guid kEfiGuid0 = {0x0, 0x0, 0x0, {0x0}};
INSTANTIATE_TEST_SUITE_P(VariableIdTest, VariableIdFixture,
                         ::testing::Values(std::list<VariableId>{
                             {String(u""), kEfiGuid0},
                             {String(u""), {0x1, 0x0, 0x0, {0x0}}},
                             {String(u"a\tb\nc\x0001\x0002"), kEfiGuid0},
                             {String(u"abcDEF123!@#"), kEfiGuid0},
                             {String(u"var0"), kEfiGuid0},
                             {String(u"var0"), {0x0, 0x0, 0x1, {0x0}}},
                             {String(u"var0"), {0x0, 0x0, 0x1, {0x1}}},
                             {String(u"var0"), {0x0, 0x1, 0x0, {0x0}}},
                             {String(u"var0"), {0x1, 0x0, 0x0, {0x0}}},
                             {String(u"var1"), kEfiGuid0},
                         }));

}  // unnamed namespace
