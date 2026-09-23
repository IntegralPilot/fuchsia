// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <algorithm>
#include <list>

#include <efi/variable/variable.h>
#include <fbl/vector.h>
#include <gtest/gtest.h>

namespace fbl {

template <typename T>
inline bool operator==(const Vector<T>& a, const Vector<T>& b) {
  return std::equal(a.begin(), a.end(), b.begin(), b.end());
}

}  // namespace fbl

namespace {

using ::testing::Test;

using namespace efi;

class VariableFixture : public ::testing::TestWithParam<Variable> {};

TEST_P(VariableFixture, VariableValueCopy) {
  Variable input = GetParam();
  const VariableValue& src = input.value;
  const VariableValue dst = Copy(src);
  EXPECT_EQ(src, dst);
}

TEST_P(VariableFixture, Constructor) {
  Variable input = GetParam();
  Variable variable(input.id, input.value);
  EXPECT_EQ(variable.id, input.id);
  EXPECT_EQ(variable.value, input.value);
}

TEST_P(VariableFixture, CopyConstructor) {
  Variable input = GetParam();
  Variable variable(input);
  EXPECT_EQ(variable.id, input.id);
  EXPECT_EQ(variable.value, input.value);
}

TEST_P(VariableFixture, MoveConstructor) {
  Variable input = GetParam();
  Variable tmp_variable(input);
  Variable variable(std::move(tmp_variable));
  EXPECT_EQ(variable.id, input.id);
  EXPECT_EQ(variable.value, input.value);
}

TEST_P(VariableFixture, CopyAssignment) {
  Variable input = GetParam();
  Variable variable = input;
  EXPECT_EQ(variable.id, input.id);
  EXPECT_EQ(variable.value, input.value);
}

TEST_P(VariableFixture, MoveAssignment) {
  Variable input = GetParam();
  Variable tmp_variable(input);
  Variable variable = std::move(tmp_variable);
  EXPECT_EQ(variable.id, input.id);
  EXPECT_EQ(variable.value, input.value);
}

constexpr efi_guid kEfiGuid0 = {0x0, 0x0, 0x0, {0x0}};
INSTANTIATE_TEST_SUITE_P(
    VariableTest, VariableFixture,
    ::testing::Values(Variable{{String(u""), kEfiGuid0}, {0x00}},
                      Variable{{String(u""), {0x1, 0x0, 0x0, {0x0}}}, {0x01}},
                      Variable{{String(u"a\tb\nc\x0001\x0002"), kEfiGuid0}, {0x00}},
                      Variable{{String(u"abcDEF123!@#"), kEfiGuid0}, {0x02}},
                      Variable{{String(u"var0"), kEfiGuid0}, {0x03}},
                      Variable{{String(u"var0"), {0x0, 0x0, 0x1, {0x0}}}, {0x04}},
                      Variable{{String(u"var0"), {0x0, 0x0, 0x1, {0x1}}}, {0x05, 0x06}},
                      Variable{{String(u"var0"), {0x0, 0x1, 0x0, {0x0}}}, {0x07, 0x08, 0x09}},
                      Variable{{String(u"var0"), {0x1, 0x0, 0x0, {0x0}}}, {0x0a}},
                      Variable{{String(u"var1"), kEfiGuid0}, {0x0c}}));

}  // unnamed namespace
