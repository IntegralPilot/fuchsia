// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/rectangle.h"

#include <gtest/gtest.h>

namespace types {

namespace {

constexpr Rectangle kRect1({.x = 100, .y = 200, .width = 400, .height = 500});
constexpr Rectangle kRect2({.x = 100, .y = 200, .width = 400, .height = 500});
constexpr Rectangle kRect3({.x = 101, .y = 200, .width = 400, .height = 500});

TEST(RectangleTest, Equality) {
  // Reflexive property.
  EXPECT_EQ(kRect1, kRect1);

  // Symmetric property.
  EXPECT_EQ(kRect1, kRect2);
  EXPECT_EQ(kRect2, kRect1);

  // Transitive property.
  EXPECT_NE(kRect1, kRect3);
  EXPECT_NE(kRect2, kRect3);

  // Test all fields for inequality.
  EXPECT_NE(kRect1, Rectangle({.x = 0, .y = 200, .width = 400, .height = 500}));
  EXPECT_NE(kRect1, Rectangle({.x = 100, .y = 0, .width = 400, .height = 500}));
  EXPECT_NE(kRect1, Rectangle({.x = 100, .y = 200, .width = 0, .height = 500}));
  EXPECT_NE(kRect1, Rectangle({.x = 100, .y = 200, .width = 400, .height = 0}));
}

TEST(RectangleTest, FromFidl) {
  // Basic conversion from a fuchsia_math::Rect.
  const fuchsia_math::Rect fidl_rect(100, 200, 400, 500);
  EXPECT_EQ(Rectangle::From(fidl_rect), kRect1);
}

TEST(RectangleTest, FromWireRect) {
  // Basic conversion from a fuchsia_math::wire::Rect.
  const fuchsia_math::wire::Rect wire_rect = {.x = 100, .y = 200, .width = 400, .height = 500};
  EXPECT_EQ(Rectangle::From(wire_rect), kRect1);
}

TEST(RectangleTest, FromFidlRectU) {
  // Basic conversion from a fuchsia_math::RectU.
  const fuchsia_math::RectU fidl_rect_u(100, 200, 400, 500);
  EXPECT_EQ(Rectangle::From(fidl_rect_u), kRect1);
}

TEST(RectangleTest, FromWireRectU) {
  // Basic conversion from a fuchsia_math::wire::RectU.
  const fuchsia_math::wire::RectU wire_rect_u = {.x = 100, .y = 200, .width = 400, .height = 500};
  EXPECT_EQ(Rectangle::From(wire_rect_u), kRect1);
}

TEST(RectangleTest, FromPointAndExtent) {
  // Basic conversion from a Point2 and Extent2.
  const Point2 origin({.x = 100, .y = 200});
  const Extent2 extent({.width = 400, .height = 500});
  EXPECT_EQ(Rectangle::From(origin, extent), kRect1);
}

TEST(RectangleTest, IsValidFidlRect) {
  // Basic valid case.
  EXPECT_TRUE(Rectangle::IsValid(fuchsia_math::Rect(0, 0, 0, 0)));

  // Negative extents are invalid.
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(0, 0, -1, 0)));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(0, 0, 0, -1)));

  // Opposite corner would overflow.
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(INT32_MAX, 0, 1, 0)));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(0, INT32_MAX, 0, 1)));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(1, 0, INT32_MAX, 0)));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(0, 1, 0, INT32_MAX)));

  // Opposite corner would underflow.
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(INT32_MIN, 0, -1, 0)));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(0, INT32_MIN, 0, -1)));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(-1, 0, INT32_MIN, 0)));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(0, -1, 0, INT32_MIN)));
}

TEST(RectangleTest, IsValidFidlRectU) {
  // Basic valid cases.
  EXPECT_TRUE(Rectangle::IsValid(fuchsia_math::RectU(0, 0, 0, 0)));
  EXPECT_TRUE(Rectangle::IsValid(fuchsia_math::RectU(1, 2, 3, 4)));

  // Casting from unsigned to signed would overflow.
  EXPECT_TRUE(Rectangle::IsValid(fuchsia_math::RectU(static_cast<uint32_t>(INT32_MAX), 0, 0, 0)));
  EXPECT_FALSE(
      Rectangle::IsValid(fuchsia_math::RectU(static_cast<uint32_t>(INT32_MAX) + 1, 0, 0, 0)));
  EXPECT_FALSE(
      Rectangle::IsValid(fuchsia_math::RectU(0, static_cast<uint32_t>(INT32_MAX) + 1, 0, 0)));
  EXPECT_FALSE(
      Rectangle::IsValid(fuchsia_math::RectU(0, 0, static_cast<uint32_t>(INT32_MAX) + 1, 0)));
  EXPECT_FALSE(
      Rectangle::IsValid(fuchsia_math::RectU(0, 0, 0, static_cast<uint32_t>(INT32_MAX) + 1)));
}

TEST(RectangleTest, IsValidWireRectU) {
  // Basic valid cases.
  EXPECT_TRUE(Rectangle::IsValid(fuchsia_math::wire::RectU{0, 0, 0, 0}));
  EXPECT_TRUE(Rectangle::IsValid(fuchsia_math::wire::RectU{1, 2, 3, 4}));

  // Casting from unsigned to signed would overflow.
  EXPECT_TRUE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = static_cast<uint32_t>(INT32_MAX), .y = 0, .width = 0, .height = 0}));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = static_cast<uint32_t>(INT32_MAX) + 1, .y = 0, .width = 0, .height = 0}));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = 0, .y = static_cast<uint32_t>(INT32_MAX) + 1, .width = 0, .height = 0}));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = 0, .y = 0, .width = static_cast<uint32_t>(INT32_MAX) + 1, .height = 0}));
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::wire::RectU{
      .x = 0, .y = 0, .width = 0, .height = static_cast<uint32_t>(INT32_MAX) + 1}));
}

TEST(RectangleTest, IsValidOppositeCorner) {
  // Test that the opposite corner check works regardless of the input FIDL
  // type, since they all flow through the same core validation logic.

  // Overflow using fuchsia_math::RectU.
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::RectU(INT32_MAX - 10, 0, 11, 0)));
  // Underflow using fuchsia_math::Rect.
  EXPECT_FALSE(Rectangle::IsValid(fuchsia_math::Rect(0, INT32_MIN + 10, 0, -11)));
}

TEST(RectangleTest, IsValidPointAndExtent) {
  // Basic valid case.
  EXPECT_TRUE(Rectangle::IsValid(Point2({.x = 0, .y = 0}), Extent2({.width = 0, .height = 0})));

  // Opposite corner would overflow.
  EXPECT_FALSE(Rectangle::IsValid(Point2({.x = INT32_MAX - 10, .y = 0}),
                                  Extent2({.width = 11, .height = 0})));
}

TEST(RectangleTest, ToFidlRectU) {
  // Basic conversion to a fuchsia_math::RectU.
  const fuchsia_math::RectU fidl_rect_u = kRect1.ToFidlRectU();
  EXPECT_EQ(fidl_rect_u.x(), 100u);
  EXPECT_EQ(fidl_rect_u.y(), 200u);
  EXPECT_EQ(fidl_rect_u.width(), 400u);
  EXPECT_EQ(fidl_rect_u.height(), 500u);
}

TEST(RectangleTest, ToWireRectU) {
  // Basic conversion to a fuchsia_math::wire::RectU.
  const fuchsia_math::wire::RectU wire_rect_u = kRect1.ToWireRectU();
  EXPECT_EQ(wire_rect_u.x, 100u);
  EXPECT_EQ(wire_rect_u.y, 200u);
  EXPECT_EQ(wire_rect_u.width, 400u);
  EXPECT_EQ(wire_rect_u.height, 500u);
}

TEST(RectangleTest, ToFidlRectF) {
  // Basic conversion to a fuchsia_math::RectF.
  const fuchsia_math::RectF fidl_rect_f = kRect1.ToFidlRectF();
  EXPECT_EQ(fidl_rect_f.x(), 100.f);
  EXPECT_EQ(fidl_rect_f.y(), 200.f);
  EXPECT_EQ(fidl_rect_f.width(), 400.f);
  EXPECT_EQ(fidl_rect_f.height(), 500.f);
}

TEST(RectangleTest, Accessors) {
  // Test all the data accessors.
  EXPECT_EQ(kRect1.origin().x(), 100);
  EXPECT_EQ(kRect1.origin().y(), 200);
  EXPECT_EQ(kRect1.extent().width(), 400);
  EXPECT_EQ(kRect1.extent().height(), 500);
  EXPECT_EQ(kRect1.opposite().x(), 500);
  EXPECT_EQ(kRect1.opposite().y(), 700);
  EXPECT_EQ(kRect1.x(), 100);
  EXPECT_EQ(kRect1.y(), 200);
  EXPECT_EQ(kRect1.width(), 400);
  EXPECT_EQ(kRect1.height(), 500);
}

TEST(RectangleTest, Hash) {
  const std::hash<Rectangle> hasher;
  EXPECT_EQ(hasher(kRect1), hasher(kRect2));
  EXPECT_NE(hasher(kRect1), hasher(kRect3));

  // Changing each field should result in a different hash.
  EXPECT_NE(hasher(kRect1), hasher(Rectangle({.x = 0, .y = 200, .width = 400, .height = 500})));
  EXPECT_NE(hasher(kRect1), hasher(Rectangle({.x = 100, .y = 0, .width = 400, .height = 500})));
  EXPECT_NE(hasher(kRect1), hasher(Rectangle({.x = 100, .y = 200, .width = 0, .height = 500})));
  EXPECT_NE(hasher(kRect1), hasher(Rectangle({.x = 100, .y = 200, .width = 400, .height = 0})));
}

TEST(RectangleTest, Contains) {
  constexpr Rectangle rect({.x = 10, .y = 20, .width = 30, .height = 40});

  // Test points inside the rectangle.
  EXPECT_TRUE(rect.Contains(Point2({.x = 10, .y = 20})));
  EXPECT_TRUE(rect.Contains(Point2({.x = 39, .y = 59})));
  EXPECT_TRUE(rect.Contains(Point2({.x = 25, .y = 40})));

  // Test points on the edge of the rectangle. The right and bottom edges are exclusive.
  EXPECT_TRUE(rect.Contains(Point2({.x = 10, .y = 20})));
  EXPECT_FALSE(rect.Contains(Point2({.x = 40, .y = 60})));
  EXPECT_FALSE(rect.Contains(Point2({.x = 10, .y = 60})));
  EXPECT_FALSE(rect.Contains(Point2({.x = 40, .y = 20})));

  // Test points outside the rectangle.
  EXPECT_FALSE(rect.Contains(Point2({.x = 9, .y = 20})));
  EXPECT_FALSE(rect.Contains(Point2({.x = 10, .y = 19})));
  EXPECT_FALSE(rect.Contains(Point2({.x = 40, .y = 59})));
  EXPECT_FALSE(rect.Contains(Point2({.x = 39, .y = 60})));

  // Test with a zero-sized rectangle.
  constexpr Rectangle zero_rect({.x = 0, .y = 0, .width = 0, .height = 0});
  EXPECT_FALSE(zero_rect.Contains(Point2({.x = 0, .y = 0})));
}

}  // namespace
}  // namespace types
