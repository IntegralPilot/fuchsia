// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/util/enum_utils.h"

#include <gtest/gtest.h>

namespace {
using namespace escher;

enum class EnumForCycling { kZero = 0, kOne, kTwo, kThree, kEnumCount };

TEST(EnumCycle, NextAndPrevious) {
  EXPECT_EQ(EnumForCycling::kThree, EnumCycle(EnumForCycling::kTwo));
  EXPECT_EQ(EnumForCycling::kOne, EnumCycle(EnumForCycling::kTwo, true));
}

TEST(EnumCycle, Wraparound) {
  EXPECT_EQ(EnumForCycling::kZero, EnumCycle(EnumForCycling::kThree));
  EXPECT_EQ(EnumForCycling::kThree, EnumCycle(EnumForCycling::kZero, true));
}

TEST(EnumArray, Correctness) {
  std::array<EnumForCycling, 4> array = EnumArray<EnumForCycling>();
  EXPECT_EQ(array[0], EnumForCycling::kZero);
  EXPECT_EQ(array[1], EnumForCycling::kOne);
  EXPECT_EQ(array[2], EnumForCycling::kTwo);
  EXPECT_EQ(array[3], EnumForCycling::kThree);
}

enum class EnumForCountingValues {
  // Order should not matter.
  kMinusTen = -10,
  kTen = 10,
  kMinusOne = -1,
  kZero = 0,
  kOne,
};

TEST(EnumElements, Maximum) {
  auto max_element = *EnumMaxElementValue<EnumForCountingValues>();
  EXPECT_EQ(max_element, 10u);

  // Setting |Begin| argument.
  max_element = *EnumMaxElementValue<EnumForCountingValues, 0>();
  // Only kZero, kOne and kTen are counted.
  EXPECT_EQ(max_element, 10u);

  // Setting |End| argument.
  max_element = *EnumMaxElementValue<EnumForCountingValues, -10, 0>();
  // Only kMinusTen and kMinusOne are counted.
  EXPECT_EQ(max_element, static_cast<size_t>(-1));
}

}  // namespace
