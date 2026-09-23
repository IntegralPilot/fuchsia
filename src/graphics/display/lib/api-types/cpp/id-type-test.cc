// Copyright 2025 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/graphics/display/lib/api-types/cpp/id-type.h"

#include <fidl/fuchsia.hardware.display.engine/cpp/wire.h>
#include <fidl/fuchsia.hardware.display.types/cpp/wire.h>
#include <fidl/fuchsia.hardware.display/cpp/wire.h>

#include <cstdint>
#include <functional>
#include <map>
#include <unordered_map>

#if __cplusplus >= 202002L
#include <format>
#endif  // __cplusplus >= 202002L

#include <gtest/gtest.h>

namespace display::internal {

namespace {

class IdTypeTest : public ::testing::Test {
 protected:
  using TestDisplayIdTraits =
      DefaultIdTypeTraits<uint64_t, fuchsia_hardware_display_types::wire::DisplayId>;
  using TestDisplayId = IdType<TestDisplayIdTraits>;

  static constexpr TestDisplayId kOne = TestDisplayId(1);
  static constexpr TestDisplayId kAnotherOne = TestDisplayId(1);
  static constexpr TestDisplayId kTwo = TestDisplayId(2);

  static constexpr uint64_t kLargeIdValue = uint64_t{1} << 63;
  static constexpr TestDisplayId kLargeId = TestDisplayId(kLargeIdValue);
};

TEST_F(IdTypeTest, EqualityIsReflexive) {
  EXPECT_EQ(kOne, kOne);
  EXPECT_EQ(kAnotherOne, kAnotherOne);
  EXPECT_EQ(kTwo, kTwo);
}

TEST_F(IdTypeTest, EqualityIsSymmetric) {
  EXPECT_EQ(kOne, kAnotherOne);
  EXPECT_EQ(kAnotherOne, kOne);
}

TEST_F(IdTypeTest, EqualityForDifferentValues) {
  EXPECT_NE(kOne, kTwo);
  EXPECT_NE(kAnotherOne, kTwo);
  EXPECT_NE(kTwo, kOne);
  EXPECT_NE(kTwo, kAnotherOne);
}

TEST_F(IdTypeTest, OrderingForEqualValues) {
  EXPECT_FALSE(kOne < kAnotherOne);
  EXPECT_FALSE(kAnotherOne < kOne);

  EXPECT_LE(kOne, kAnotherOne);
  EXPECT_LE(kAnotherOne, kOne);

  EXPECT_FALSE(kOne > kAnotherOne);
  EXPECT_FALSE(kAnotherOne > kOne);

  EXPECT_GE(kOne, kAnotherOne);
  EXPECT_GE(kAnotherOne, kAnotherOne);
}

TEST_F(IdTypeTest, OrderingForDifferentValues) {
  EXPECT_LT(kOne, kTwo);
  EXPECT_FALSE(kTwo < kOne);

  EXPECT_LE(kOne, kTwo);
  EXPECT_FALSE(kTwo <= kOne);

  EXPECT_FALSE(kOne > kTwo);
  EXPECT_GT(kTwo, kOne);

  EXPECT_FALSE(kOne >= kTwo);
  EXPECT_GE(kTwo, kOne);
}

TEST_F(IdTypeTest, HashSpecialization) {
  EXPECT_EQ(std::hash<uint64_t>()(uint64_t{1}), std::hash<TestDisplayId>()(kOne));
  EXPECT_EQ(std::hash<uint64_t>()(uint64_t{2}), std::hash<TestDisplayId>()(kTwo));
  EXPECT_EQ(std::hash<uint64_t>()(uint64_t{kLargeIdValue}), std::hash<TestDisplayId>()(kLargeId));
}

TEST_F(IdTypeTest, OrderedMapKeyUsage) {
  std::map<TestDisplayId, int> ordered_map;
  ordered_map[kOne] = 1;
  ordered_map[kTwo] = 2;

  EXPECT_EQ(1, ordered_map[kOne]);
  EXPECT_EQ(2, ordered_map[kTwo]);
  EXPECT_EQ(0u, ordered_map.count(kLargeId));
}

TEST_F(IdTypeTest, UnorderedMapKeyUsage) {
  std::unordered_map<TestDisplayId, int> unordered_map;
  unordered_map[kOne] = 1;
  unordered_map[kTwo] = 2;

  EXPECT_EQ(1, unordered_map[kOne]);
  EXPECT_EQ(2, unordered_map[kTwo]);
  EXPECT_EQ(0u, unordered_map.count(kLargeId));
}

TEST_F(IdTypeTest, CastToUnderlyingType) {
  EXPECT_EQ(1u, static_cast<uint64_t>(kOne));
  EXPECT_EQ(2u, static_cast<uint64_t>(kTwo));
  EXPECT_EQ(kLargeIdValue, static_cast<uint64_t>(kLargeId));
}

TEST_F(IdTypeTest, Value) {
  EXPECT_EQ(1u, kOne.value());
  EXPECT_EQ(2u, kTwo.value());
  EXPECT_EQ(kLargeIdValue, kLargeId.value());
}

TEST_F(IdTypeTest, ToFidl) {
  EXPECT_EQ(1u, kOne.ToFidl().value);
  EXPECT_EQ(2u, kTwo.ToFidl().value);
  EXPECT_EQ(kLargeIdValue, kLargeId.ToFidl().value);
}

TEST_F(IdTypeTest, FromFidl) {
  EXPECT_EQ(kOne, TestDisplayId(fuchsia_hardware_display_types::wire::DisplayId{.value = 1}));
  EXPECT_EQ(kTwo, TestDisplayId(fuchsia_hardware_display_types::wire::DisplayId{.value = 2}));
  EXPECT_EQ(kLargeId,
            TestDisplayId(fuchsia_hardware_display_types::wire::DisplayId{.value = kLargeIdValue}));
}

TEST_F(IdTypeTest, FidlConversionRoundtrip) {
  EXPECT_EQ(kOne, TestDisplayId(kOne.ToFidl()));
  EXPECT_EQ(kTwo, TestDisplayId(kTwo.ToFidl()));
  EXPECT_EQ(kLargeId, TestDisplayId(kLargeId.ToFidl()));
}

TEST_F(IdTypeTest, PreIncrement) {
  TestDisplayId display_id(1);
  TestDisplayId& preincrement_result = ++display_id;

  EXPECT_EQ(2u, display_id.value());
  EXPECT_EQ(&display_id, &preincrement_result);
}

TEST_F(IdTypeTest, PostIncrement) {
  TestDisplayId display_id(1);
  TestDisplayId postincrement_result = display_id++;

  EXPECT_EQ(2u, display_id.value());
  EXPECT_EQ(1u, postincrement_result.value());
}

#if __cplusplus >= 202002L
TEST_F(IdTypeTest, FormatSpecialization) {
  EXPECT_EQ("1", std::format("{}", kOne));
  EXPECT_EQ("2", std::format("{}", kTwo));
  EXPECT_EQ("0x0002", std::format("{:#06x}", kTwo));
}
#endif  // __cplusplus >= 202002L

}  // namespace

}  // namespace display::internal
