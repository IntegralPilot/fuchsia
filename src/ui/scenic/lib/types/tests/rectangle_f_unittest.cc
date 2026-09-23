// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/rectangle_f.h"

#include <limits>

#include <gtest/gtest.h>

#include "src/ui/scenic/lib/types/point2_f.h"

namespace types {
namespace {

constexpr float kFpInfinity = std::numeric_limits<float>::infinity();

TEST(RectangleFTest, IsValid) {
  // Basic valid case.
  EXPECT_TRUE(RectangleF::IsValid({.x = 0.f, .y = 0.f, .width = 0.f, .height = 0.f}));

  // Invalid cases.
  EXPECT_FALSE(RectangleF::IsValid({.x = kFpInfinity, .y = 0.f, .width = 0.f, .height = 0.f}));
  EXPECT_FALSE(RectangleF::IsValid({.x = 0.f, .y = 0.f, .width = -1.f, .height = 0.f}));
  // Adding a small width to `float::max()` evaluates to `float::max()` due to precision limits.
  // This is a valid geometric rectangle, even if its bounds get absorbed.
  EXPECT_TRUE(RectangleF::IsValid(
      {.x = std::numeric_limits<float>::max(), .y = 0.f, .width = 1.f, .height = 0.f}));
  EXPECT_FALSE(RectangleF::IsValid({.x = std::numeric_limits<float>::max(),
                                    .y = 0.f,
                                    .width = std::numeric_limits<float>::max(),
                                    .height = 0.f}));
}

TEST(RectangleFTest, FromFidl) {
  // Basic conversion from a fuchsia_math::RectF.
  const fuchsia_math::RectF fidl_rect(1.f, 2.f, 3.f, 4.f);
  EXPECT_EQ(RectangleF::From(fidl_rect),
            RectangleF({.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f}));

  const fuchsia_math::wire::RectF wire_rect{.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f};
  EXPECT_EQ(RectangleF::From(wire_rect),
            RectangleF({.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f}));
}

TEST(RectangleFTest, FromPointAndExtent) {
  // Basic conversion from a Point2F and Extent2F.
  const Point2F origin({.x = 1.f, .y = 2.f});
  const Extent2F extent({.width = 3.f, .height = 4.f});
  EXPECT_EQ(RectangleF::From(origin, extent),
            RectangleF({.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f}));
}

TEST(RectangleFTest, ToFidl) {
  // Basic conversion to a fuchsia_math::RectF.
  const RectangleF r({.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f});
  const fuchsia_math::RectF fidl_rect = r.ToFidl();
  EXPECT_EQ(fidl_rect.x(), 1.f);
  EXPECT_EQ(fidl_rect.y(), 2.f);
  EXPECT_EQ(fidl_rect.width(), 3.f);
  EXPECT_EQ(fidl_rect.height(), 4.f);

  const fuchsia_math::wire::RectF wire_rect = r.ToWire();
  EXPECT_EQ(wire_rect.x, 1.f);
  EXPECT_EQ(wire_rect.y, 2.f);
  EXPECT_EQ(wire_rect.width, 3.f);
  EXPECT_EQ(wire_rect.height, 4.f);
}

TEST(RectangleFTest, Accessors) {
  // Test all the data accessors.
  const RectangleF r({.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f});
  EXPECT_EQ(r.origin().x(), 1.f);
  EXPECT_EQ(r.origin().y(), 2.f);
  EXPECT_EQ(r.extent().width(), 3.f);
  EXPECT_EQ(r.extent().height(), 4.f);
  EXPECT_EQ(r.opposite().x(), 4.f);
  EXPECT_EQ(r.opposite().y(), 6.f);
  EXPECT_EQ(r.x(), 1.f);
  EXPECT_EQ(r.y(), 2.f);
  EXPECT_EQ(r.width(), 3.f);
  EXPECT_EQ(r.height(), 4.f);
}

TEST(RectangleFTest, Hash) {
  // Test that the hash function is working correctly.
  const RectangleF r1({.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f});
  const RectangleF r2({.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f});
  const RectangleF r3({.x = 4.f, .y = 3.f, .width = 2.f, .height = 1.f});

  const std::hash<RectangleF> hasher;
  EXPECT_EQ(hasher(r1), hasher(r2));
  EXPECT_NE(hasher(r1), hasher(r3));
}

TEST(RectangleFTest, Contains) {
  const RectangleF r({.x = 10.f, .y = 20.f, .width = 30.f, .height = 40.f});

  // Point inside.
  EXPECT_TRUE(r.Contains({.x = 25.f, .y = 40.f}));

  // Point on top-left corner.
  EXPECT_TRUE(r.Contains({.x = 10.f, .y = 20.f}));
  // Point on bottom-right corner.
  EXPECT_TRUE(r.Contains({.x = 40.f, .y = 60.f}));

  // Points on edges.
  EXPECT_TRUE(r.Contains({.x = 10.f, .y = 40.f}));  // Left
  EXPECT_TRUE(r.Contains({.x = 40.f, .y = 40.f}));  // Right
  EXPECT_TRUE(r.Contains({.x = 25.f, .y = 20.f}));  // Top
  EXPECT_TRUE(r.Contains({.x = 25.f, .y = 60.f}));  // Bottom

  // Points outside.
  EXPECT_FALSE(r.Contains({.x = 9.f, .y = 40.f}));   // Left
  EXPECT_FALSE(r.Contains({.x = 41.f, .y = 40.f}));  // Right
  EXPECT_FALSE(r.Contains({.x = 25.f, .y = 19.f}));  // Top
  EXPECT_FALSE(r.Contains({.x = 25.f, .y = 61.f}));  // Bottom

  // Points just within default epsilon.
  constexpr float kDefaultEpsilon = 1e-3f;
  EXPECT_TRUE(r.Contains({.x = 10.f - kDefaultEpsilon, .y = 40.f}));
  EXPECT_TRUE(r.Contains({.x = 40.f + kDefaultEpsilon, .y = 40.f}));
  EXPECT_TRUE(r.Contains({.x = 25.f, .y = 20.f - kDefaultEpsilon}));
  EXPECT_TRUE(r.Contains({.x = 25.f, .y = 60.f + kDefaultEpsilon}));

  // Points just outside default epsilon.
  EXPECT_FALSE(r.Contains({.x = 10.f - kDefaultEpsilon * 1.1f, .y = 40.f}));
  EXPECT_FALSE(r.Contains({.x = 40.f + kDefaultEpsilon * 1.1f, .y = 40.f}));
  EXPECT_FALSE(r.Contains({.x = 25.f, .y = 20.f - kDefaultEpsilon * 1.1f}));
  EXPECT_FALSE(r.Contains({.x = 25.f, .y = 60.f + kDefaultEpsilon * 1.1f}));

  // Test with a custom epsilon.
  constexpr float kCustomEpsilon = 0.5f;
  EXPECT_TRUE(r.Contains({.x = 10.f - kCustomEpsilon, .y = 40.f}, kCustomEpsilon));
  EXPECT_FALSE(r.Contains({.x = 10.f - kCustomEpsilon * 1.1f, .y = 40.f}, kCustomEpsilon));
}

TEST(RectangleFTest, ScaledBy) {
  const RectangleF r({.x = 1.f, .y = 2.f, .width = 3.f, .height = 4.f});

  // Scale by 2.
  EXPECT_EQ(r.ScaledBy(2.f), RectangleF({.x = 2.f, .y = 4.f, .width = 6.f, .height = 8.f}));

  // Scale by 0.5.
  EXPECT_EQ(r.ScaledBy(0.5f), RectangleF({.x = 0.5f, .y = 1.f, .width = 1.5f, .height = 2.f}));

  // Scale by 0.
  EXPECT_EQ(r.ScaledBy(0.f), RectangleF({.x = 0.f, .y = 0.f, .width = 0.f, .height = 0.f}));

  // Scale by 1.
  EXPECT_EQ(r.ScaledBy(1.f), r);
}

}  // namespace
}  // namespace types
