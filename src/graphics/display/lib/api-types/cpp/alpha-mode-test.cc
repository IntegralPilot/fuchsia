// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/alpha-mode.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>

#include <gtest/gtest.h>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

namespace {

constexpr AlphaMode kPremultiplied2(
    fuchsia_hardware_display_types::wire::AlphaMode::kPremultiplied);

TEST(AlphaModeTest, EqualityIsReflexive) {
  EXPECT_EQ(AlphaMode::kPremultiplied, AlphaMode::kPremultiplied);
  EXPECT_EQ(kPremultiplied2, kPremultiplied2);
  EXPECT_EQ(AlphaMode::kDisable, AlphaMode::kDisable);
}

TEST(AlphaModeTest, EqualityIsSymmetric) {
  EXPECT_EQ(AlphaMode::kPremultiplied, kPremultiplied2);
  EXPECT_EQ(kPremultiplied2, AlphaMode::kPremultiplied);
}

TEST(AlphaModeTest, EqualityForDifferentValues) {
  EXPECT_NE(AlphaMode::kPremultiplied, AlphaMode::kDisable);
  EXPECT_NE(AlphaMode::kDisable, AlphaMode::kPremultiplied);
  EXPECT_NE(kPremultiplied2, AlphaMode::kDisable);
  EXPECT_NE(AlphaMode::kDisable, kPremultiplied2);
}

TEST(AlphaModeTest, ToFidlAlphaMode) {
  static constexpr fuchsia_hardware_display_types::wire::AlphaMode fidl_transformation =
      AlphaMode::kPremultiplied.ToFidl();
  EXPECT_EQ(fuchsia_hardware_display_types::wire::AlphaMode::kPremultiplied, fidl_transformation);
}

TEST(AlphaModeTest, ToAlphaModeWithFidlValue) {
  static constexpr AlphaMode transformation(
      fuchsia_hardware_display_types::wire::AlphaMode::kPremultiplied);
  EXPECT_EQ(AlphaMode::kPremultiplied, transformation);
}

TEST(AlphaModeTest, ValueForLogging) {
  EXPECT_EQ(static_cast<uint32_t>(fuchsia_hardware_display_types::wire::AlphaMode::kPremultiplied),
            AlphaMode::kPremultiplied.ValueForLogging());
}

TEST(AlphaModeTest, FidlConversionRoundtrip) {
  EXPECT_EQ(AlphaMode::kPremultiplied, AlphaMode(AlphaMode::kPremultiplied.ToFidl()));
  EXPECT_EQ(AlphaMode::kDisable, AlphaMode(AlphaMode::kDisable.ToFidl()));
}

TEST(AlphaMode, ToString) {
  EXPECT_EQ(AlphaMode::kDisable.ToString(), "Disable");
  EXPECT_EQ(AlphaMode::kPremultiplied.ToString(), "Premultiplied");
  EXPECT_EQ(AlphaMode::kHwMultiply.ToString(), "HwMultiply");
}

#if __cplusplus >= 202002L
TEST(AlphaMode, Format) {
  EXPECT_EQ(std::format("{}", AlphaMode::kDisable), "Disable");
  EXPECT_EQ(std::format("{}", AlphaMode::kPremultiplied), "Premultiplied");
  EXPECT_EQ(std::format("{:>10}", AlphaMode::kDisable), "   Disable");
}
#endif  // __cplusplus >= 202002L

}  // namespace

}  // namespace display
