// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/address_range_map.h"

#include <gtest/gtest.h>

namespace zxdb {

TEST(AddressRangeMap, Query) {
  // Empty map should always return 0.
  AddressRangeMap empty;
  EXPECT_EQ(AddressRangeMap::kNullValue, empty.Lookup(0));
  EXPECT_EQ(AddressRangeMap::kNullValue, empty.Lookup(0x12371623));

  // One range.
  std::vector<AddressRangeMap::Record> recs;
  recs.emplace_back(0x1000, 99u);
  recs.emplace_back(0x2000, AddressRangeMap::kNullValue);  // Ending terminator.
  AddressRangeMap one_range(recs);
  EXPECT_EQ(AddressRangeMap::kNullValue, one_range.Lookup(0));
  EXPECT_EQ(AddressRangeMap::kNullValue, one_range.Lookup(0xfff));
  EXPECT_EQ(99u, one_range.Lookup(0x1000));
  EXPECT_EQ(99u, one_range.Lookup(0x1fff));
  EXPECT_EQ(AddressRangeMap::kNullValue,
            one_range.Lookup(0x2000));  // Range ends are non-inclusive.
  EXPECT_EQ(AddressRangeMap::kNullValue, one_range.Lookup(0x2001));

  // Two ranges.
  std::vector<AddressRangeMap::Record> recs2;
  recs2.emplace_back(0x1000, 199);
  recs2.emplace_back(0x1001, 299u);
  recs2.emplace_back(0x2000, AddressRangeMap::kNullValue);  // Ending terminator.
  AddressRangeMap two_ranges(recs2);
  EXPECT_EQ(AddressRangeMap::kNullValue, two_ranges.Lookup(0));
  EXPECT_EQ(AddressRangeMap::kNullValue, two_ranges.Lookup(0xfff));
  EXPECT_EQ(199u, two_ranges.Lookup(0x1000));
  EXPECT_EQ(299u, two_ranges.Lookup(0x1001));
  EXPECT_EQ(299u, two_ranges.Lookup(0x1fff));
  EXPECT_EQ(AddressRangeMap::kNullValue,
            two_ranges.Lookup(0x2000));  // Range ends are non-inclusive.
  EXPECT_EQ(AddressRangeMap::kNullValue, two_ranges.Lookup(0x2001));
}

}  // namespace zxdb
