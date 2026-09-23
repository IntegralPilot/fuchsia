// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/point2.h"

#include <gtest/gtest.h>

namespace types {

namespace {

TEST(Point2Test, FromFidl) {
  const Point2 p = Point2::From(fuchsia_math::Vec(1, 2));
  EXPECT_EQ(p.x(), 1);
  EXPECT_EQ(p.y(), 2);
}

TEST(Point2Test, FromWire) {
  const Point2 p = Point2::From(fuchsia_math::wire::Vec{.x = 1, .y = 2});
  EXPECT_EQ(p.x(), 1);
  EXPECT_EQ(p.y(), 2);
}

TEST(Point2Test, Accessors) {
  constexpr Point2 p({.x = 1, .y = 2});
  EXPECT_EQ(p.x(), 1);
  EXPECT_EQ(p.y(), 2);
}

TEST(Point2Test, IsOrigin) {
  EXPECT_TRUE(Point2({.x = 0, .y = 0}).IsOrigin());
  EXPECT_FALSE(Point2({.x = 1, .y = 0}).IsOrigin());
  EXPECT_FALSE(Point2({.x = 0, .y = 1}).IsOrigin());
  EXPECT_FALSE(Point2({.x = 1, .y = 1}).IsOrigin());
  EXPECT_FALSE(Point2({.x = -1, .y = 0}).IsOrigin());
  EXPECT_FALSE(Point2({.x = 0, .y = -1}).IsOrigin());
  EXPECT_FALSE(Point2({.x = -1, .y = -1}).IsOrigin());
}

TEST(Point2Test, Hash) {
  const Point2 p1({.x = 1, .y = 2});
  const Point2 p2({.x = 1, .y = 2});
  const Point2 p3({.x = 2, .y = 1});
  const std::hash<Point2> hasher;
  EXPECT_EQ(hasher(p1), hasher(p2));
  EXPECT_NE(hasher(p1), hasher(p3));

  // Changing each field should result in a different hash.
  EXPECT_NE(hasher(p1), hasher(Point2({.x = 0, .y = 2})));
  EXPECT_NE(hasher(p1), hasher(Point2({.x = 1, .y = 0})));
}

}  // namespace
}  // namespace types
