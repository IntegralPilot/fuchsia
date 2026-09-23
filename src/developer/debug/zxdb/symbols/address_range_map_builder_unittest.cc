// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/address_range_map_builder.h"

#include <gtest/gtest.h>

namespace zxdb {

TEST(AddressRangeMapBuilder, Build) {
  // No ranges.
  AddressRangeMapBuilder build_empty;
  EXPECT_TRUE(build_empty.GetMap().empty());

  // Build several consecutive ones with a gap in the middle.
  AddressRangeMapBuilder build_cons;
  build_cons.AddRange(AddressRange(0x2000, 0x3000), 299);
  build_cons.AddRange(AddressRange(0x1000, 0x1800), 199);
  AddressRangeMap result = build_cons.GetMap();

  using Record = AddressRangeMap::Record;

  ASSERT_EQ(result.map_.size(), 4u);
  EXPECT_EQ(result.map_[0], Record(0x1000, 199));
  EXPECT_EQ(result.map_[1], Record(0x1800, 0));
  EXPECT_EQ(result.map_[2], Record(0x2000, 299));
  EXPECT_EQ(result.map_[3], Record(0x3000, 0));

  // Do some nested ones:
  //
  // A---------------------------
  //         B-----------
  // C---        D---        E---
  // 1   2   3   4   5   6   7   8
  AddressRangeMapBuilder build_nested;
  build_nested.AddRange(AddressRange(0x1000, 0x8000), 1);  // A
  build_nested.AddRange(AddressRange(0x3000, 0x6000), 2);  // B
  build_nested.AddRange(AddressRange(0x1000, 0x2000), 3);  // C
  build_nested.AddRange(AddressRange(0x4000, 0x5000), 4);  // D
  build_nested.AddRange(AddressRange(0x7000, 0x8000), 5);  // E

  result = build_nested.GetMap();
  ASSERT_EQ(result.map_.size(), 8u);
  EXPECT_EQ(result.map_[0], Record(0x1000, 3));
  EXPECT_EQ(result.map_[1], Record(0x2000, 1));
  EXPECT_EQ(result.map_[2], Record(0x3000, 2));
  EXPECT_EQ(result.map_[3], Record(0x4000, 4));
  EXPECT_EQ(result.map_[4], Record(0x5000, 2));
  EXPECT_EQ(result.map_[5], Record(0x6000, 1));
  EXPECT_EQ(result.map_[6], Record(0x7000, 5));
  EXPECT_EQ(result.map_[7], Record(0x8000, 0));
}

}  // namespace zxdb
