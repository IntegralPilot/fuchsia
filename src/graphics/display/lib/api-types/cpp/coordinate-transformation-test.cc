// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/coordinate-transformation.h"

#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>

#include <gtest/gtest.h>

#if __cplusplus >= 202002L
#include <format>
#endif

namespace display {

namespace {

constexpr CoordinateTransformation kReflectY2(
    fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectY);

TEST(CoordinateTransformationTest, EqualityIsReflexive) {
  EXPECT_EQ(CoordinateTransformation::kReflectY, CoordinateTransformation::kReflectY);
  EXPECT_EQ(kReflectY2, kReflectY2);
  EXPECT_EQ(CoordinateTransformation::kReflectX, CoordinateTransformation::kReflectX);
}

TEST(CoordinateTransformationTest, EqualityIsSymmetric) {
  EXPECT_EQ(CoordinateTransformation::kReflectY, kReflectY2);
  EXPECT_EQ(kReflectY2, CoordinateTransformation::kReflectY);
}

TEST(CoordinateTransformationTest, EqualityForDifferentValues) {
  EXPECT_NE(CoordinateTransformation::kReflectY, CoordinateTransformation::kReflectX);
  EXPECT_NE(CoordinateTransformation::kReflectX, CoordinateTransformation::kReflectY);
  EXPECT_NE(kReflectY2, CoordinateTransformation::kReflectX);
  EXPECT_NE(CoordinateTransformation::kReflectX, kReflectY2);
}

TEST(CoordinateTransformationTest, ToFidlCoordinateTransformation) {
  static constexpr fuchsia_hardware_display_types::wire::CoordinateTransformation
      fidl_transformation = CoordinateTransformation::kReflectY.ToFidl();
  EXPECT_EQ(fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectY,
            fidl_transformation);
}

TEST(CoordinateTransformationTest, ToCoordinateTransformationWithFidlValue) {
  static constexpr CoordinateTransformation transformation(
      fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectY);
  EXPECT_EQ(CoordinateTransformation::kReflectY, transformation);
}

TEST(CoordinateTransformationTest, ValueForLogging) {
  EXPECT_EQ(static_cast<uint32_t>(
                fuchsia_hardware_display_types::wire::CoordinateTransformation::kReflectY),
            CoordinateTransformation::kReflectY.ValueForLogging());
}

TEST(CoordinateTransformationTest, FidlConversionRoundtrip) {
  EXPECT_EQ(CoordinateTransformation::kReflectY,
            CoordinateTransformation(CoordinateTransformation::kReflectY.ToFidl()));
  EXPECT_EQ(CoordinateTransformation::kReflectX,
            CoordinateTransformation(CoordinateTransformation::kReflectX.ToFidl()));
}

TEST(CoordinateTransformation, ToString) {
  EXPECT_EQ(CoordinateTransformation::kIdentity.ToString(), "Identity");
  EXPECT_EQ(CoordinateTransformation::kReflectX.ToString(), "ReflectX");
  EXPECT_EQ(CoordinateTransformation::kReflectY.ToString(), "ReflectY");
  EXPECT_EQ(CoordinateTransformation::kRotateCcw90.ToString(), "RotateCcw90");
  EXPECT_EQ(CoordinateTransformation::kRotateCcw180.ToString(), "RotateCcw180");
  EXPECT_EQ(CoordinateTransformation::kRotateCcw270.ToString(), "RotateCcw270");
  EXPECT_EQ(CoordinateTransformation::kRotateCcw90ReflectX.ToString(), "RotateCcw90ReflectX");
  EXPECT_EQ(CoordinateTransformation::kRotateCcw90ReflectY.ToString(), "RotateCcw90ReflectY");
}

#if __cplusplus >= 202002L
TEST(CoordinateTransformation, Format) {
  EXPECT_EQ(std::format("{}", CoordinateTransformation::kIdentity), "Identity");
  EXPECT_EQ(std::format("{}", CoordinateTransformation::kReflectX), "ReflectX");
  EXPECT_EQ(std::format("{:>10}", CoordinateTransformation::kIdentity), "  Identity");
}
#endif  // __cplusplus >= 202002L

}  // namespace

}  // namespace display
