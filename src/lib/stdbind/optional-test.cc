// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/stdbind/optional.h>

#include <cstdint>
#include <optional>

#include <zxtest/zxtest.h>

namespace {

struct Point {
  int x;
  int y;

  bool operator==(const Point& other) const = default;
};

TEST(OptionalTest, DefaultConstruction) {
  stdbind::optional<int> opt;
  EXPECT_FALSE(opt);
  EXPECT_FALSE(opt.has_value());
  EXPECT_EQ(opt.to_std(), std::nullopt);
  EXPECT_EQ(std::optional<int>(opt), std::nullopt);
}

TEST(OptionalTest, NulloptConstruction) {
  stdbind::optional<int> opt(std::nullopt);
  EXPECT_FALSE(opt);
  EXPECT_FALSE(opt.has_value());
  EXPECT_EQ(opt.to_std(), std::nullopt);
  EXPECT_EQ(std::optional<int>(opt), std::nullopt);
}

TEST(OptionalTest, ValueConstruction) {
  int val = 42;
  stdbind::optional<int> from_lvalue(val);
  EXPECT_TRUE(from_lvalue);
  EXPECT_TRUE(from_lvalue.has_value());
  EXPECT_EQ(from_lvalue.to_std(), std::optional<int>(42));
  EXPECT_EQ(std::optional<int>(from_lvalue), std::optional<int>(42));

  stdbind::optional<int> from_rvalue(100);
  EXPECT_EQ(from_rvalue.to_std(), std::optional<int>(100));
  EXPECT_EQ(std::optional<int>(from_rvalue), std::optional<int>(100));
}

TEST(OptionalTest, StdOptionalConstruction) {
  std::optional<int> some = 50;
  stdbind::optional<int> from_some(some);
  EXPECT_EQ(from_some.to_std(), std::optional<int>(50));

  std::optional<int> none = std::nullopt;
  stdbind::optional<int> from_none(none);
  EXPECT_EQ(from_none.to_std(), std::nullopt);

  stdbind::optional<int> from_rvalue_some(std::optional<int>(75));
  EXPECT_EQ(from_rvalue_some.to_std(), std::optional<int>(75));

  stdbind::optional<int> from_rvalue_none(std::optional<int>{});
  EXPECT_EQ(from_rvalue_none.to_std(), std::nullopt);
}

TEST(OptionalTest, ValueAssignment) {
  stdbind::optional<int> opt;
  EXPECT_EQ(opt.to_std(), std::nullopt);

  int val = 123;
  opt = val;
  EXPECT_EQ(opt.to_std(), std::optional<int>(123));

  opt = 456;
  EXPECT_EQ(opt.to_std(), std::optional<int>(456));
}

TEST(OptionalTest, NulloptAssignment) {
  stdbind::optional<int> opt(42);
  EXPECT_EQ(opt.to_std(), std::optional<int>(42));

  opt = std::nullopt;
  EXPECT_EQ(opt.to_std(), std::nullopt);
}

TEST(OptionalTest, StdOptionalAssignment) {
  stdbind::optional<int> opt;

  std::optional<int> some = 999;
  opt = some;
  EXPECT_EQ(opt.to_std(), std::optional<int>(999));

  std::optional<int> none = std::nullopt;
  opt = none;
  EXPECT_EQ(opt.to_std(), std::nullopt);

  opt = std::optional<int>(888);
  EXPECT_EQ(opt.to_std(), std::optional<int>(888));

  opt = std::optional<int>{};
  EXPECT_EQ(opt.to_std(), std::nullopt);
}

TEST(OptionalTest, PodStruct) {
  Point p{.x = 10, .y = 20};
  stdbind::optional<Point> opt(p);
  EXPECT_EQ(opt.to_std(), std::optional<Point>(Point{.x = 10, .y = 20}));

  opt = Point{.x = 30, .y = 40};
  EXPECT_EQ(opt.to_std(), std::optional<Point>(Point{.x = 30, .y = 40}));

  opt = std::nullopt;
  EXPECT_EQ(opt.to_std(), std::nullopt);
}

TEST(OptionalTest, Comparisons) {
  stdbind::optional<int> none1;
  stdbind::optional<int> none2(std::nullopt);
  stdbind::optional<int> some1(42);
  stdbind::optional<int> some2(42);
  stdbind::optional<int> some3(100);

  EXPECT_EQ(none1, none2);
  EXPECT_EQ(some1, some2);
  EXPECT_NE(none1, some1);
  EXPECT_NE(some1, some3);
  EXPECT_LT(none1, some1);
  EXPECT_LT(some1, some3);
}

TEST(OptionalTest, AbiLayoutAndDiscriminant) {
  stdbind::optional<uint32_t> none;
  EXPECT_EQ(*reinterpret_cast<const uint64_t*>(&none), 0);

  stdbind::optional<uint32_t> some(0x12345678);
  EXPECT_EQ(*reinterpret_cast<const uint64_t*>(&some), 1);
  EXPECT_EQ(*reinterpret_cast<const uint32_t*>(reinterpret_cast<const uint8_t*>(&some) + 8),
            0x12345678);
}

static_assert(sizeof(stdbind::optional<uint8_t>) == 16);
static_assert(alignof(stdbind::optional<uint8_t>) == 8);

static_assert(sizeof(stdbind::optional<uint16_t>) == 16);
static_assert(alignof(stdbind::optional<uint16_t>) == 8);

static_assert(sizeof(stdbind::optional<uint32_t>) == 16);
static_assert(alignof(stdbind::optional<uint32_t>) == 8);

static_assert(sizeof(stdbind::optional<uint64_t>) == 16);
static_assert(alignof(stdbind::optional<uint64_t>) == 8);

static_assert(sizeof(stdbind::optional<Point>) == 16);
static_assert(alignof(stdbind::optional<Point>) == 8);

static_assert(stdbind::optional<int>().to_std() == std::nullopt);
static_assert(stdbind::optional<int>(42).to_std() == std::optional<int>(42));
static_assert(std::optional<int>(stdbind::optional<int>(42)) == std::optional<int>(42));
static_assert(static_cast<bool>(stdbind::optional<int>(42)));
static_assert(!static_cast<bool>(stdbind::optional<int>()));
static_assert(stdbind::optional<int>(42).has_value());
static_assert(!stdbind::optional<int>().has_value());

}  // namespace
