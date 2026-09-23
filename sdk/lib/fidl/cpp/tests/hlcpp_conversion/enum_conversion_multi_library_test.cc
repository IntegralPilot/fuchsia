// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// This tests a specific regression where an empty FIDL library is included
// in front of another FIDL library with enums, and later enum conversion
// is exercised.
//
// clang-format off
#include <fidl/test.enums.abc/cpp/hlcpp_conversion.h>
#include <fidl/test.enums.xyz/cpp/hlcpp_conversion.h>
// clang-format on

#include <gtest/gtest.h>

namespace {

TEST(EnumConversionMultiLibrary, StrictHLCPPToNatural) {
  auto hlcpp = test::enums::xyz::StrictEnum::FOO;
  auto natural = fidl::HLCPPToNatural(hlcpp);
  EXPECT_EQ(natural, test_enums_xyz::StrictEnum::kFoo);
}

TEST(EnumConversionMultiLibrary, FlexibleHLCPPToNatural) {
  auto hlcpp = test::enums::xyz::FlexibleEnum::BAR;
  auto natural = fidl::HLCPPToNatural(hlcpp);
  EXPECT_EQ(natural, test_enums_xyz::FlexibleEnum::kBar);
}

TEST(EnumConversionMultiLibrary, StrictNaturalToHLCPP) {
  auto natural = test_enums_xyz::StrictEnum::kFoo;
  auto hlcpp = fidl::NaturalToHLCPP(natural);
  EXPECT_EQ(hlcpp, test::enums::xyz::StrictEnum::FOO);
}

TEST(EnumConversionMultiLibrary, FlexibleNaturalToHLCPP) {
  auto natural = test_enums_xyz::FlexibleEnum::kBar;
  auto hlcpp = fidl::NaturalToHLCPP(natural);
  EXPECT_EQ(hlcpp, test::enums::xyz::FlexibleEnum::BAR);
}

}  // namespace
