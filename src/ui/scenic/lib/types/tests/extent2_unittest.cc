// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/scenic/lib/types/extent2.h"

#include <gtest/gtest.h>

namespace types {

namespace {

TEST(Extent2Test, IsValid) {
  // Valid cases.
  EXPECT_TRUE(Extent2::IsValid(0, 0));
  EXPECT_TRUE(Extent2::IsValid(1, 1));
  EXPECT_TRUE(Extent2::IsValid(1, 0));
  EXPECT_TRUE(Extent2::IsValid(0, 1));
  EXPECT_TRUE(Extent2::IsValid(INT32_MAX, INT32_MAX));

  // Invalid cases.
  EXPECT_FALSE(Extent2::IsValid(-1, 0));
  EXPECT_FALSE(Extent2::IsValid(0, -1));
  EXPECT_FALSE(Extent2::IsValid(INT32_MIN, 0));
  EXPECT_FALSE(Extent2::IsValid(0, INT32_MIN));
}

TEST(Extent2Test, IsValidFidl) {
  // Valid cases.
  EXPECT_TRUE(Extent2::IsValid(fuchsia_math::SizeU(0, 0)));
  EXPECT_TRUE(Extent2::IsValid(fuchsia_math::SizeU(1, 1)));
  EXPECT_TRUE(Extent2::IsValid(fuchsia_math::SizeU(INT32_MAX, INT32_MAX)));

  // Invalid cases.
  EXPECT_FALSE(Extent2::IsValid(fuchsia_math::SizeU(static_cast<uint32_t>(INT32_MAX) + 1, 0)));
  EXPECT_FALSE(Extent2::IsValid(fuchsia_math::SizeU(0, static_cast<uint32_t>(INT32_MAX) + 1)));
}

TEST(Extent2Test, IsValidWire) {
  // Valid cases.
  EXPECT_TRUE(Extent2::IsValid(fuchsia_math::wire::SizeU{0, 0}));
  EXPECT_TRUE(Extent2::IsValid(fuchsia_math::wire::SizeU{1, 1}));
  EXPECT_TRUE(Extent2::IsValid(fuchsia_math::wire::SizeU{INT32_MAX, INT32_MAX}));

  // Invalid cases.
  EXPECT_FALSE(
      Extent2::IsValid(fuchsia_math::wire::SizeU{static_cast<uint32_t>(INT32_MAX) + 1, 0}));
  EXPECT_FALSE(
      Extent2::IsValid(fuchsia_math::wire::SizeU{0, static_cast<uint32_t>(INT32_MAX) + 1}));
}

TEST(Extent2Test, IsEmpty) {
  EXPECT_TRUE(Extent2({.width = 0, .height = 0}).IsEmpty());
  EXPECT_TRUE(Extent2({.width = 1, .height = 0}).IsEmpty());
  EXPECT_TRUE(Extent2({.width = 0, .height = 1}).IsEmpty());
  EXPECT_FALSE(Extent2({.width = 1, .height = 1}).IsEmpty());
}

TEST(Extent2Test, Hash) {
  const Extent2 e1({.width = 1, .height = 2});
  const Extent2 e2({.width = 1, .height = 2});
  const Extent2 e3({.width = 2, .height = 1});
  const std::hash<Extent2> hasher;
  EXPECT_EQ(hasher(e1), hasher(e2));
  EXPECT_NE(hasher(e1), hasher(e3));

  // Changing each field should result in a different hash.
  EXPECT_NE(hasher(e1), hasher(Extent2({.width = 0, .height = 2})));
  EXPECT_NE(hasher(e1), hasher(Extent2({.width = 1, .height = 0})));
}

}  // namespace
}  // namespace types
