// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/ui/lib/escher/util/bit_ops.h"

#include <utility>
#include <vector>

#include <gtest/gtest.h>

namespace {
using namespace escher;

TEST(BitOps, CountTrailingZeros) {
  // Some easy ones.
  ASSERT_EQ(0, CountTrailingZeros(0x1));
  ASSERT_EQ(1, CountTrailingZeros(0x2));
  ASSERT_EQ(31, CountTrailingZeros(0x80000000));

  // Iterate through additional cases.
  const uint32_t kOne = 1;
  const uint32_t kAlmostMax = ~0U;
  for (size_t i = 0; i < 32; ++i) {
    EXPECT_EQ(int32_t(i), CountTrailingZeros(kOne << i));
    EXPECT_EQ(int32_t(i), CountTrailingZeros(kAlmostMax << i));
  }
}

TEST(BitOps, CountLeadingZeros) {
  // Some easy ones.
  ASSERT_EQ(0, CountLeadingZeros(0x80000000));
  ASSERT_EQ(1, CountLeadingZeros(0x40000000));
  ASSERT_EQ(31, CountLeadingZeros(0x1));

  // Iterate through additional cases.
  const uint32_t kHighestBit = 0x80000000;
  const uint32_t kAlmostMax = ~0U;
  for (size_t i = 0; i < 32; ++i) {
    EXPECT_EQ(int32_t(i), CountLeadingZeros(kHighestBit >> i));
    EXPECT_EQ(int32_t(i), CountLeadingZeros(kAlmostMax >> i));
  }
}

TEST(BitOps, CountOnes) {
  EXPECT_EQ(0u, CountOnes(0u));
  EXPECT_EQ(1u, CountOnes(1u));
  EXPECT_EQ(4u, CountOnes(0xF0000000u));
  EXPECT_EQ(8u, CountOnes(0xF000F000u));
  EXPECT_EQ(8u, CountOnes(0x000F000Fu));
  EXPECT_EQ(24u, CountOnes(0xEEEEEEEEu));
}

template <typename T>
void TestSetBitsAtAndAboveIndex() {
  size_t kNumBits = sizeof(T) * 8;

  const T kZeros = 0u;
  const T kOnes = ~kZeros;

  for (size_t i = 0; i < kNumBits; ++i) {
    using UT = std::make_unsigned_t<T>;
    const UT kAtAndAboveBits = static_cast<UT>(kOnes) << i;
    const UT kBelowBits = ~kAtAndAboveBits;

    T bits = kZeros;
    SetBitsAtAndAboveIndex(&bits, i);
    EXPECT_EQ(static_cast<UT>(bits) & kAtAndAboveBits, kAtAndAboveBits);
    EXPECT_EQ(static_cast<UT>(bits) & kBelowBits, static_cast<UT>(kZeros));

    bits = kOnes;
    SetBitsAtAndAboveIndex(&bits, i);
    EXPECT_EQ(bits, kOnes);
  }
}

TEST(BitOps, SetBitsAtAndAboveIndex) {
  TestSetBitsAtAndAboveIndex<uint16_t>();
  TestSetBitsAtAndAboveIndex<uint32_t>();
  TestSetBitsAtAndAboveIndex<uint64_t>();
  TestSetBitsAtAndAboveIndex<int16_t>();
  TestSetBitsAtAndAboveIndex<int32_t>();
  TestSetBitsAtAndAboveIndex<int64_t>();
}

template <typename T>
void TestRotateLeft() {
  using UT = std::make_unsigned_t<T>;
  EXPECT_EQ(2U, RotateLeft(1U, 1));
  EXPECT_EQ(4U, RotateLeft(1U, 2));
  EXPECT_EQ(6U, RotateLeft(3U, 1));
  EXPECT_EQ(12U, RotateLeft(3U, 2));

  const T max = std::numeric_limits<T>::max();
  const int digits = std::numeric_limits<T>::digits;

  for (int i = 1; i < digits; ++i) {
    EXPECT_EQ(max, RotateLeft(max, i));
  }

  const T high_order_bit = static_cast<T>(UT(1) << digits - 1U);
  EXPECT_EQ(1U, RotateLeft(high_order_bit, 1));
}

TEST(BitOps, RotateLeft) {
  TestRotateLeft<uint8_t>();
  TestRotateLeft<uint16_t>();
  TestRotateLeft<uint32_t>();
  TestRotateLeft<uint64_t>();
}

TEST(BitOps, ForEachBitIndex) {
  std::vector<uint32_t> indices;
  auto collect = [&indices](uint32_t bit) { indices.push_back(bit); };

  ForEachBitIndex(0u, collect);
  EXPECT_TRUE(indices.empty());

  ForEachBitIndex(0x80000001u, collect);
  EXPECT_EQ(indices, (std::vector<uint32_t>{0u, 31u}));
}

TEST(BitOps, ForEachBitRange) {
  using Ranges = std::vector<std::pair<uint32_t, uint32_t>>;
  auto collect = [](uint32_t value) {
    Ranges ranges;
    ForEachBitRange(value,
                    [&ranges](uint32_t bit, uint32_t range) { ranges.emplace_back(bit, range); });
    return ranges;
  };

  EXPECT_EQ(collect(0u), Ranges());
  EXPECT_EQ(collect(0b1011u), (Ranges{{0u, 2u}, {3u, 1u}}));

  // Ranges which extend all the way to the most-significant bit are the
  // interesting cases: they make the internal shift amount 32, which used to
  // be undefined behavior.  On x86 the shift wrapped around to zero, so the
  // range was never cleared and these calls looped forever.
  EXPECT_EQ(collect(0x80000000u), (Ranges{{31u, 1u}}));
  EXPECT_EQ(collect(0xF0000000u), (Ranges{{28u, 4u}}));
  EXPECT_EQ(collect(0xC0000003u), (Ranges{{0u, 2u}, {30u, 2u}}));
  EXPECT_EQ(collect(0xFFFFFFFFu), (Ranges{{0u, 32u}}));
}

}  // namespace
