// Copyright 2022 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <string>

#include <gtest/gtest.h>

#include "tools/fidl/fidlc/src/versioning_types.h"

namespace fidlc {
namespace {

using Legacy = Availability::Legacy;
using InheritResult = Availability::InheritResult;
using Status = InheritResult::Status;

Version v(uint32_t x) { return Version::From(x).value(); }
VersionRange range(uint32_t x, uint32_t y) { return VersionRange(v(x), v(y)); }
VersionSet set(uint32_t x, uint32_t y) { return VersionSet(range(x, y)); }
VersionSet set(std::pair<uint32_t, uint32_t> a, std::pair<uint32_t, uint32_t> b) {
  return VersionSet(range(a.first, a.second), range(b.first, b.second));
}

TEST(VersioningTypesTests, GoodPlatformParse) {
  EXPECT_EQ(Platform::Parse("foo123").value().name(), "foo123");
}

TEST(VersioningTypesTests, GoodPlatformUnversioned) {
  EXPECT_TRUE(Platform::Unversioned().is_unversioned());
  EXPECT_TRUE(Platform::Parse("unversioned").value().is_unversioned());
  EXPECT_FALSE(Platform::Parse("foo123").value().is_unversioned());
}

TEST(VersioningTypesTests, BadPlatformParseEmpty) { EXPECT_FALSE(Platform::Parse("").has_value()); }

TEST(VersioningTypesTests, BadPlatformParseInvalidChar) {
  EXPECT_FALSE(Platform::Parse("foo_bar").has_value());
}

TEST(VersioningTypesTests, GoodPlatformEquality) {
  EXPECT_EQ(Platform::Parse("foo").value(), Platform::Parse("foo").value());
  EXPECT_EQ(Platform::Unversioned(), Platform::Unversioned());
  EXPECT_EQ(Platform::Parse("unversioned").value(), Platform::Unversioned());

  EXPECT_NE(Platform::Parse("foo").value(), Platform::Parse("bar").value());
  EXPECT_NE(Platform::Parse("foo").value(), Platform::Unversioned());
}

TEST(VersioningTypesTests, GoodVersionFromMinNormal) {
  auto maybe_version = Version::From(1);
  ASSERT_TRUE(maybe_version.has_value());
  EXPECT_EQ(maybe_version.value().number(), 1u);
  EXPECT_EQ(maybe_version.value().ToString(), "1");
}

TEST(VersioningTypesTests, GoodVersionFromMaxNormal) {
  uint32_t number = (1u << 31) - 1;
  auto maybe_version = Version::From(number);
  ASSERT_TRUE(maybe_version.has_value());
  EXPECT_EQ(maybe_version.value().number(), number);
  EXPECT_EQ(maybe_version.value().ToString(), std::to_string(number));
  // Confirm this is in fact the last valid number.
  EXPECT_EQ(Version::From(number + 1), std::nullopt);
}

TEST(VersioningTypesTests, GoodVersionFromHead) {
  uint32_t number = 0xFFE00000;
  auto maybe_version = Version::From(number);
  ASSERT_TRUE(maybe_version.has_value());
  EXPECT_EQ(maybe_version.value().number(), number);
  EXPECT_EQ(maybe_version.value().ToString(), "HEAD");
}

TEST(VersioningTypesTests, GoodVersionFromLegacy) {
  uint32_t number = 0xFFF00000;
  auto maybe_version = Version::From(number);
  ASSERT_TRUE(maybe_version.has_value());
  EXPECT_EQ(maybe_version.value().number(), number);
  EXPECT_EQ(maybe_version.value().ToString(), "LEGACY");
}

TEST(VersioningTypesTests, BadVersionFrom) {
  ASSERT_EQ(Version::From(0), std::nullopt);
  ASSERT_EQ(Version::From(1u << 31), std::nullopt);
  ASSERT_EQ(Version::From(UINT32_MAX), std::nullopt);
}

TEST(VersioningTypesTests, GoodVersionParse) {
  uint32_t max_numeric = (1u << 31) - 1;
  uint32_t head = 0xFFE00000;
  uint32_t legacy = 0xFFF00000;

  EXPECT_EQ(Version::Parse("1"), v(1));
  EXPECT_EQ(Version::Parse(std::to_string(max_numeric)), v(max_numeric));
  EXPECT_EQ(Version::Parse(std::to_string(head)), v(head));
  EXPECT_EQ(Version::Parse(std::to_string(legacy)), v(legacy));
  EXPECT_EQ(Version::Parse("HEAD"), v(head));
  EXPECT_EQ(Version::Parse("LEGACY"), v(legacy));
}

TEST(VersioningTypesTests, BadVersionParse) {
  EXPECT_EQ(Version::Parse(""), std::nullopt);
  EXPECT_EQ(Version::Parse("0"), std::nullopt);
  EXPECT_EQ(Version::Parse("2147483648"), std::nullopt);  // 2^31
  EXPECT_EQ(Version::Parse("4294967296"), std::nullopt);  // 2^32
  EXPECT_EQ(Version::Parse("-1"), std::nullopt);
}

TEST(VersioningTypesTests, GoodVersionRangeComparisons) {
  EXPECT_EQ(range(1, 2), range(1, 2));
  EXPECT_EQ(range(2, 3), range(2, 3));

  EXPECT_NE(range(1, 2), range(1, 3));
  EXPECT_NE(range(1, 3), range(2, 3));
  EXPECT_NE(range(2, 3), range(1, 2));

  EXPECT_LT(range(1, 2), range(1, 3));
  EXPECT_LT(range(1, 3), range(2, 3));
  EXPECT_LT(range(1, 2), range(2, 3));

  EXPECT_GT(range(1, 3), range(1, 2));
  EXPECT_GT(range(2, 3), range(1, 3));
  EXPECT_GT(range(2, 3), range(1, 2));
}

TEST(VersioningTypesTests, GoodVersionRangeIntersect) {
  // Case #1: (empty) (empty)
  EXPECT_EQ(VersionRange::Intersect(std::nullopt, std::nullopt), std::nullopt);

  // Case #2: (empty) |---|
  EXPECT_EQ(VersionRange::Intersect(std::nullopt, range(3, 6)), std::nullopt);

  // Case #3: |---| (empty)
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), std::nullopt), std::nullopt);

  // Case #4:  |---|
  //                 |--|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(7, 9))), std::nullopt);

  // Case #5:  |---|
  //               |--|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(6, 8))), std::nullopt);

  // Case #6:  |---|
  //             |--|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(5, 7))), range(5, 6));

  // Case #7:  |---|
  //            |--|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(4, 6))), range(4, 6));

  // Case #8:  |---|
  //           |--|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(3, 5))), range(3, 5));

  // Case #9:  |---|
  //            |-|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(4, 5))), range(4, 5));

  // Case #10:  |---|
  //           |---|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(3, 6))), range(3, 6));

  // Case #11:  |---|
  //          |--|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(2, 4))), range(3, 4));

  // Case #12:  |---|
  //        |--|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(1, 3))), std::nullopt);

  // Case #13: |---|
  //      |--|
  EXPECT_EQ(VersionRange::Intersect(range(3, 6), (range(1, 2))), std::nullopt);
}

TEST(VersioningTypesTests, GoodVersionSetContains) {
  VersionSet two_three(range(2, 4));
  EXPECT_FALSE(VersionSet(two_three).Contains(v(1)));
  EXPECT_TRUE(VersionSet(two_three).Contains(v(2)));
  EXPECT_TRUE(VersionSet(two_three).Contains(v(3)));
  EXPECT_FALSE(VersionSet(two_three).Contains(v(4)));
  EXPECT_FALSE(VersionSet(two_three).Contains(v(5)));
  EXPECT_FALSE(VersionSet(two_three).Contains(v(6)));

  VersionSet two_three_five(range(2, 4), range(5, 6));
  EXPECT_FALSE(two_three_five.Contains(v(1)));
  EXPECT_TRUE(two_three_five.Contains(v(2)));
  EXPECT_TRUE(two_three_five.Contains(v(3)));
  EXPECT_FALSE(two_three_five.Contains(v(4)));
  EXPECT_TRUE(two_three_five.Contains(v(5)));
  EXPECT_FALSE(two_three_five.Contains(v(6)));
}

TEST(VersioningTypesTests, GoodVersionSetIntersect) {
  // Case #1: (empty) (empty)
  EXPECT_EQ(VersionSet::Intersect(std::nullopt, std::nullopt), std::nullopt);

  // Case #2: |---| (empty)
  EXPECT_EQ(VersionSet::Intersect(set(1, 3), std::nullopt), std::nullopt);

  // Case #3: (empty) |---|
  EXPECT_EQ(VersionSet::Intersect(std::nullopt, set(1, 3)), std::nullopt);

  // Case #4: |---|
  //              |---|
  EXPECT_EQ(VersionSet::Intersect(set(1, 3), set(3, 5)), std::nullopt);

  // Case #5: |---|
  //          |---|
  EXPECT_EQ(VersionSet::Intersect(set(1, 3), set(1, 3)), set(1, 3));

  // Case #6: |---| |---|
  //                    |---|
  EXPECT_EQ(VersionSet::Intersect(set({1, 3}, {4, 6}), set(6, 8)), std::nullopt);

  // Case #7: |---| |---|
  //                |---|
  EXPECT_EQ(VersionSet::Intersect(set({1, 3}, {4, 6}), set(4, 6)), set(4, 6));

  // Case #8: |---| |---|
  //             |---|
  EXPECT_EQ(VersionSet::Intersect(set({1, 3}, {4, 6}), set(2, 5)), set({2, 3}, {4, 5}));

  // Case #9: |---| |---|
  //          |---|
  EXPECT_EQ(VersionSet::Intersect(set({1, 3}, {4, 6}), set(1, 3)), set(1, 3));

  // Case #10:           |---|
  //           |---| |---|
  EXPECT_EQ(VersionSet::Intersect(set(6, 8), set({1, 3}, {4, 6})), std::nullopt);

  // Case #11:       |---|
  //           |---| |---|
  EXPECT_EQ(VersionSet::Intersect(set(4, 6), set({1, 3}, {4, 6})), set(4, 6));

  // Case #12:    |---|
  //           |---| |---|
  EXPECT_EQ(VersionSet::Intersect(set(2, 5), set({1, 3}, {4, 6})), set({2, 3}, {4, 5}));

  // Case #13: |---|
  //           |---| |---|
  EXPECT_EQ(VersionSet::Intersect(set(1, 3), set({1, 3}, {4, 6})), set(1, 3));

  // Case #14: |---| |---|
  //                     |---| |---|
  EXPECT_EQ(VersionSet::Intersect(set({1, 3}, {4, 6}), set({6, 8}, {9, 11})), std::nullopt);

  // Case #15: |---| |---|
  //                 |---| |---|
  EXPECT_EQ(VersionSet::Intersect(set({1, 3}, {4, 6}), set({4, 6}, {7, 9})), set(4, 6));

  // Case #16: |---| |---|
  //              |---|  |---|
  EXPECT_EQ(VersionSet::Intersect(set({1, 3}, {4, 6}), set({2, 5}, {6, 8})), set({2, 3}, {4, 5}));

  // Case #17: |---| |---|
  //           |---| |---|
  EXPECT_EQ(VersionSet::Intersect(set({1, 3}, {4, 6}), set({1, 3}, {4, 6})), set({1, 3}, {4, 6}));

  // Case #18:    |---|  |---|
  //           |---| |---|
  EXPECT_EQ(VersionSet::Intersect(set({2, 5}, {6, 8}), set({1, 3}, {4, 6})), set({2, 3}, {4, 5}));
}

TEST(VersioningTypesTests, GoodAvailabilityInitNone) {
  Availability availability;
  ASSERT_TRUE(availability.Init({}));
  EXPECT_EQ(availability.Debug(), "_ _ _ _");
}

TEST(VersioningTypesTests, GoodAvailabilityInitAdded) {
  Availability availability;
  ASSERT_TRUE(availability.Init({.added = v(1)}));
  EXPECT_EQ(availability.Debug(), "1 _ _ _");
}

TEST(VersioningTypesTests, GoodAvailabilityInitAll) {
  Availability availability;
  ASSERT_TRUE(availability.Init({.added = v(1), .deprecated = v(2), .removed = v(3)}));
  EXPECT_EQ(availability.Debug(), "1 2 3 _");
}

TEST(VersioningTypesTests, BadAvailabilityInitWrongOrder) {
  Availability availability;
  EXPECT_FALSE(availability.Init({.added = v(1), .removed = v(1)}));
}

TEST(VersioningTypesTests, GoodAvailabilityInheritUnbounded) {
  Availability availability;
  ASSERT_TRUE(availability.Init({.added = v(1)}));
  ASSERT_TRUE(availability.Inherit(Availability::Unbounded()).Ok());
  EXPECT_EQ(availability.Debug(), "1 _ +inf n/a");
}

TEST(VersioningTypesTests, GoodAvailabilityInheritUnset) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(1), .deprecated = v(2), .removed = v(3)}));
  ASSERT_TRUE(child.Init({}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());
  ASSERT_TRUE(child.Inherit(parent).Ok());
  EXPECT_EQ(parent.Debug(), "1 2 3 no");
  EXPECT_EQ(child.Debug(), "1 2 3 no");
}

TEST(VersioningTypesTests, GoodAvailabilityInheritUnchanged) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(1), .deprecated = v(2), .removed = v(3)}));
  ASSERT_TRUE(child.Init({.added = v(1), .deprecated = v(2), .removed = v(3)}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());
  ASSERT_TRUE(child.Inherit(parent).Ok());
  EXPECT_EQ(parent.Debug(), "1 2 3 no");
  EXPECT_EQ(child.Debug(), "1 2 3 no");
}

TEST(VersioningTypesTests, GoodAvailabilityInheritPartial) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(1)}));
  ASSERT_TRUE(child.Init({.removed = v(2)}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());
  ASSERT_TRUE(child.Inherit(parent).Ok());
  EXPECT_EQ(parent.Debug(), "1 _ +inf n/a");
  EXPECT_EQ(child.Debug(), "1 _ 2 no");
}

TEST(VersioningTypesTests, GoodAvailabilityInheritChangeDeprecation) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(1), .deprecated = v(1)}));
  ASSERT_TRUE(child.Init({.added = v(2)}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());
  ASSERT_TRUE(child.Inherit(parent).Ok());
  EXPECT_EQ(parent.Debug(), "1 1 +inf n/a");
  EXPECT_EQ(child.Debug(), "2 2 +inf n/a");
}

TEST(VersioningTypesTests, GoodAvailabilityInheritEliminateDeprecation) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(1), .deprecated = v(2)}));
  ASSERT_TRUE(child.Init({.removed = v(2)}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());
  ASSERT_TRUE(child.Inherit(parent).Ok());
  EXPECT_EQ(parent.Debug(), "1 2 +inf n/a");
  EXPECT_EQ(child.Debug(), "1 _ 2 no");
}

TEST(VersioningTypesTests, BadAvailabilityInheritBeforeParentCompletely) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(3)}));
  ASSERT_TRUE(child.Init({.added = v(1), .deprecated = v(2), .removed = v(3)}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());

  auto status = child.Inherit(parent);
  EXPECT_EQ(status.added, Status::kBeforeParentAdded);
  EXPECT_EQ(status.deprecated, Status::kBeforeParentAdded);
  EXPECT_EQ(status.removed, Status::kBeforeParentAdded);
}

TEST(VersioningTypesTests, BadAvailabilityInheritBeforeParentPartially) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(3)}));
  ASSERT_TRUE(child.Init({.added = v(1), .deprecated = v(2), .removed = v(4)}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());

  auto status = child.Inherit(parent);
  EXPECT_EQ(status.added, Status::kBeforeParentAdded);
  EXPECT_EQ(status.deprecated, Status::kBeforeParentAdded);
  EXPECT_EQ(status.removed, Status::kOk);
}

TEST(VersioningTypesTests, BadAvailabilityInheritAfterParentCompletely) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(1), .removed = v(2)}));
  ASSERT_TRUE(child.Init({.added = v(2), .deprecated = v(3), .removed = v(4)}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());

  auto status = child.Inherit(parent);
  EXPECT_EQ(status.added, Status::kAfterParentRemoved);
  EXPECT_EQ(status.deprecated, Status::kAfterParentRemoved);
  EXPECT_EQ(status.removed, Status::kAfterParentRemoved);
}

TEST(VersioningTypesTests, BadAvailabilityInheritAfterParentPartially) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(1), .removed = v(2)}));
  ASSERT_TRUE(child.Init({.added = v(1), .deprecated = v(2), .removed = v(3)}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());

  auto status = child.Inherit(parent);
  EXPECT_EQ(status.added, Status::kOk);
  EXPECT_EQ(status.deprecated, Status::kAfterParentRemoved);
  EXPECT_EQ(status.removed, Status::kAfterParentRemoved);
}

TEST(VersioningTypesTests, BadAvailabilityInheritAfterParentDeprecated) {
  Availability parent, child;
  ASSERT_TRUE(parent.Init({.added = v(1), .deprecated = v(2)}));
  ASSERT_TRUE(child.Init({.added = v(1), .deprecated = v(3), .removed = v(4)}));
  ASSERT_TRUE(parent.Inherit(Availability::Unbounded()).Ok());

  auto status = child.Inherit(parent);
  EXPECT_EQ(status.added, Status::kOk);
  EXPECT_EQ(status.deprecated, Status::kAfterParentDeprecated);
  EXPECT_EQ(status.removed, Status::kOk);
}

TEST(VersioningTypesTests, GoodAvailabilityDecomposeWhole) {
  Availability availability;
  ASSERT_TRUE(availability.Init({.added = v(1), .removed = v(2)}));
  ASSERT_TRUE(availability.Inherit(Availability::Unbounded()).Ok());

  availability.Narrow(range(1, 2));
  EXPECT_EQ(availability.Debug(), "1 _ 2 no");
}

}  // namespace
}  // namespace fidlc
