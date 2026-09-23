// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/point2_f.h"

#include <limits>

#include <gtest/gtest.h>

namespace types {
namespace {

constexpr float kFpInfinity = std::numeric_limits<float>::infinity();

TEST(Point2FTest, IsValid) {
  // Valid cases.
  EXPECT_TRUE(Point2F::IsValid(0.f, 0.f));
  EXPECT_TRUE(Point2F::IsValid(1.f, -1.f));

  // Invalid cases.
  EXPECT_FALSE(Point2F::IsValid(kFpInfinity, 0.f));
  EXPECT_FALSE(Point2F::IsValid(0.f, kFpInfinity));
  EXPECT_FALSE(Point2F::IsValid(-kFpInfinity, 0.f));
  EXPECT_FALSE(Point2F::IsValid(0.f, -kFpInfinity));
  EXPECT_FALSE(Point2F::IsValid(std::numeric_limits<float>::quiet_NaN(), 0.f));
}

TEST(Point2FTest, FromFidl) {
  // Basic conversion from a fuchsia_math::VecF.
  const fuchsia_math::VecF fidl_vec(1.f, 2.f);
  EXPECT_EQ(Point2F::From(fidl_vec), Point2F({.x = 1.f, .y = 2.f}));
}

TEST(Point2FTest, FromWire) {
  // Basic conversion from a fuchsia_math::wire::VecF.
  const fuchsia_math::wire::VecF wire_vec = {.x = 1.f, .y = 2.f};
  EXPECT_EQ(Point2F::From(wire_vec), Point2F({.x = 1.f, .y = 2.f}));
}

TEST(Point2FTest, ToFidl) {
  // Basic conversion to a fuchsia_math::VecF.
  const Point2F p({.x = 1.f, .y = 2.f});
  const fuchsia_math::VecF fidl_vec = p.ToFidl();
  EXPECT_EQ(fidl_vec.x(), 1.f);
  EXPECT_EQ(fidl_vec.y(), 2.f);
}

TEST(Point2FTest, ToWire) {
  // Basic conversion to a fuchsia_math::wire::VecF.
  const Point2F p({.x = 1.f, .y = 2.f});
  const fuchsia_math::wire::VecF wire_vec = p.ToWire();
  EXPECT_EQ(wire_vec.x, 1.f);
  EXPECT_EQ(wire_vec.y, 2.f);
}

TEST(Point2FTest, Accessors) {
  // Test all the data accessors.
  const Point2F p({.x = 1.f, .y = 2.f});
  EXPECT_EQ(p.x(), 1.f);
  EXPECT_EQ(p.y(), 2.f);
  EXPECT_TRUE(Point2F({.x = 0.f, .y = 0.f}).IsOrigin());
  EXPECT_FALSE(p.IsOrigin());
}

TEST(Point2FTest, Hash) {
  // Test that the hash function is working correctly.
  const Point2F p1({.x = 1.f, .y = 2.f});
  const Point2F p2({.x = 1.f, .y = 2.f});
  const Point2F p3({.x = 2.f, .y = 1.f});

  const std::hash<Point2F> hasher;
  EXPECT_EQ(hasher(p1), hasher(p2));
  EXPECT_NE(hasher(p1), hasher(p3));
}

}  // namespace
}  // namespace types
