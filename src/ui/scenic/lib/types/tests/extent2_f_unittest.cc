// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/extent2_f.h"

#include <limits>

#include <gtest/gtest.h>

namespace types {
namespace {

constexpr float kFpInfinity = std::numeric_limits<float>::infinity();

TEST(Extent2FTest, IsValid) {
  // Valid cases.
  EXPECT_TRUE(Extent2F::IsValid(0.f, 0.f));
  EXPECT_TRUE(Extent2F::IsValid(1.f, 1.f));
  EXPECT_TRUE(Extent2F::IsValid(1.f, 0.f));
  EXPECT_TRUE(Extent2F::IsValid(0.f, 1.f));

  // Invalid cases.
  EXPECT_FALSE(Extent2F::IsValid(-1.f, 0.f));
  EXPECT_FALSE(Extent2F::IsValid(0.f, -1.f));
  EXPECT_FALSE(Extent2F::IsValid(kFpInfinity, 0.f));
  EXPECT_FALSE(Extent2F::IsValid(0.f, kFpInfinity));
  EXPECT_FALSE(Extent2F::IsValid(std::numeric_limits<float>::quiet_NaN(), 0.f));
}

TEST(Extent2FTest, IsEmpty) {
  EXPECT_TRUE(Extent2F({.width = 0.f, .height = 0.f}).IsEmpty());
  EXPECT_TRUE(Extent2F({.width = 1.f, .height = 0.f}).IsEmpty());
  EXPECT_TRUE(Extent2F({.width = 0.f, .height = 1.f}).IsEmpty());
  EXPECT_FALSE(Extent2F({.width = 1.f, .height = 1.f}).IsEmpty());
}

TEST(Extent2FTest, FromFidl) {
  // Basic conversion from a fuchsia_math::SizeF.
  const fuchsia_math::SizeF fidl_size(1.f, 2.f);
  EXPECT_EQ(Extent2F::From(fidl_size), Extent2F({.width = 1.f, .height = 2.f}));
}

TEST(Extent2FTest, FromWire) {
  // Basic conversion from a fuchsia_math::wire::SizeF.
  const fuchsia_math::wire::SizeF wire_size = {.width = 1.f, .height = 2.f};
  EXPECT_EQ(Extent2F::From(wire_size), Extent2F({.width = 1.f, .height = 2.f}));
}

TEST(Extent2FTest, ToFidl) {
  // Basic conversion to a fuchsia_math::SizeF.
  const Extent2F e({.width = 1.f, .height = 2.f});
  const fuchsia_math::SizeF fidl_size = e.ToFidl();
  EXPECT_EQ(fidl_size.width(), 1.f);
  EXPECT_EQ(fidl_size.height(), 2.f);
}

TEST(Extent2FTest, ToWire) {
  // Basic conversion to a fuchsia_math::wire::SizeF.
  const Extent2F e({.width = 1.f, .height = 2.f});
  const fuchsia_math::wire::SizeF wire_size = e.ToWire();
  EXPECT_EQ(wire_size.width, 1.f);
  EXPECT_EQ(wire_size.height, 2.f);
}

TEST(Extent2FTest, Accessors) {
  // Test all the data accessors.
  const Extent2F e({.width = 1.f, .height = 2.f});
  EXPECT_EQ(e.width(), 1.f);
  EXPECT_EQ(e.height(), 2.f);
}

TEST(Extent2FTest, Hash) {
  // Test that the hash function is working correctly.
  const Extent2F e1({.width = 1.f, .height = 2.f});
  const Extent2F e2({.width = 1.f, .height = 2.f});
  const Extent2F e3({.width = 2.f, .height = 1.f});

  const std::hash<Extent2F> hasher;
  EXPECT_EQ(hasher(e1), hasher(e2));
  EXPECT_NE(hasher(e1), hasher(e3));
}

}  // namespace
}  // namespace types
