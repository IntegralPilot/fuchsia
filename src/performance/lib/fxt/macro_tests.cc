// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/fxt/map_macro.h>

#include <limits>
#include <ostream>
#include <vector>

#include <gtest/gtest.h>

namespace {

class Value {
 public:
  constexpr Value() = default;
  explicit constexpr Value(int value) : value_{value} {}
  constexpr Value(int a, int b) : value_{a - b} {}

  Value(const Value&) = default;
  Value& operator=(const Value&) = default;

  constexpr int value() const { return value_; }

  friend constexpr Value operator+(Value value) { return value; }
  friend constexpr Value operator+(Value a, Value b) { return Value{a.value() + b.value()}; }

  friend constexpr bool operator==(Value a, Value b) { return a.value() == b.value(); }
  friend constexpr bool operator!=(Value a, Value b) { return a.value() != b.value(); }

  friend std::ostream& operator<<(std::ostream& stream, Value value) {
    stream << "Value{" << value.value() << "}";
    return stream;
  }

 private:
  int value_{std::numeric_limits<int>::max()};
};

#define MAKE_PLUS_VALUE(...) \
  +Value {                   \
    __VA_ARGS__              \
  }

#define ADD_VALUES(a, b) a + b

TEST(Fxt, MapMacros) {
  const Value a{1};
  const Value b{2};
  const Value c{3};
  const Value d{4};

  ASSERT_TRUE(a == a);
  ASSERT_TRUE(a != b);

  // Empty sets.
  {
    const Value expected{};
    const Value actual{FXT_MAP(MAKE_PLUS_VALUE)};
    EXPECT_EQ(expected, actual);
  }

  {
    const Value expected{};
    const Value actual{FXT_MAP_ARGS(MAKE_PLUS_VALUE)};
    EXPECT_EQ(expected, actual);
  }

  {
    const std::vector<Value> expected = {};
    const std::vector<Value> actual = {FXT_MAP_LIST(MAKE_PLUS_VALUE)};
    EXPECT_EQ(expected, actual);
  }

  // Non-empty sets.
  {
    const Value expected = +a + b + c + d;
    const Value actual = FXT_MAP(MAKE_PLUS_VALUE, a, b, c, d);
    EXPECT_EQ(expected, actual);
  }

  {
    const Value expected = +Value{1, 2} + Value{3, 4} + Value{5, 6};
    const Value actual = FXT_MAP_ARGS(MAKE_PLUS_VALUE, (1, 2), (3, 4), (5, 6));
    EXPECT_EQ(expected, actual);
  }

  {
    const std::vector<Value> expected = {a, b, c, d};
    const std::vector<Value> actual = {FXT_MAP_LIST(MAKE_PLUS_VALUE, 1, 2, 3, 4)};
    EXPECT_EQ(expected, actual);
  }

  {
    const std::vector<Value> expected = {a + b, b + c, c + d};
    const std::vector<Value> actual = {FXT_MAP_LIST_ARGS(ADD_VALUES, (a, b), (b, c), (c, d))};
    EXPECT_EQ(expected, actual);
  }
}

}  // anonymous namespace
