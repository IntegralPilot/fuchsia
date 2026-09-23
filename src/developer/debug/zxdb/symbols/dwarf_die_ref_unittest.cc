// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/developer/debug/zxdb/symbols/dwarf_die_ref.h"

#include <sstream>

#include <gtest/gtest.h>

namespace zxdb {

TEST(DwarfDieRef, Basics) {
  DwarfDieRef empty;
  EXPECT_FALSE(empty.is_valid());
  EXPECT_FALSE(static_cast<bool>(empty));
  EXPECT_EQ(DwarfDieRef::Section::kMain, empty.section());

  DwarfDieRef zero_ref = DwarfDieRef::Main(0);
  EXPECT_TRUE(zero_ref.is_valid());
  EXPECT_TRUE(static_cast<bool>(zero_ref));
  EXPECT_EQ(0u, zero_ref.offset());
  EXPECT_EQ(DwarfDieRef::Section::kMain, zero_ref.section());

  DwarfDieRef main_ref = DwarfDieRef::Main(0x1234);
  EXPECT_TRUE(main_ref.is_valid());
  EXPECT_TRUE(static_cast<bool>(main_ref));
  EXPECT_EQ(0x1234u, main_ref.offset());
  EXPECT_EQ(DwarfDieRef::Section::kMain, main_ref.section());

  DwarfDieRef type_ref = DwarfDieRef::Type(0x5678);
  EXPECT_TRUE(type_ref.is_valid());
  EXPECT_TRUE(static_cast<bool>(type_ref));
  EXPECT_EQ(0x5678u, type_ref.offset());
  EXPECT_EQ(DwarfDieRef::Section::kType, type_ref.section());

  DwarfDieRef d4_type = DwarfDieRef::ForTypeUnit(4, 0x1000);
  EXPECT_EQ(DwarfDieRef::Type(0x1000), d4_type);

  DwarfDieRef d5_type = DwarfDieRef::ForTypeUnit(5, 0x1000);
  EXPECT_EQ(DwarfDieRef::Main(0x1000), d5_type);
}

TEST(DwarfDieRef, Comparison) {
  DwarfDieRef empty;
  DwarfDieRef zero = DwarfDieRef::Main(0);
  DwarfDieRef main1 = DwarfDieRef::Main(0x10);
  DwarfDieRef main2 = DwarfDieRef::Main(0x20);
  DwarfDieRef type1 = DwarfDieRef::Type(0x10);
  DwarfDieRef type2 = DwarfDieRef::Type(0x20);

  EXPECT_LT(empty, zero);
  EXPECT_LT(zero, main1);
  EXPECT_LT(main1, main2);
  EXPECT_LT(type1, type2);
  EXPECT_EQ(main1, DwarfDieRef::Main(0x10));
  EXPECT_NE(main1, empty);
  EXPECT_NE(main1, type1);
  EXPECT_LT(main1, type1);
}

TEST(DwarfDieRef, Math) {
  DwarfDieRef main_ref = DwarfDieRef::Main(0x10);
  DwarfDieRef added_main = main_ref + 0x10;
  EXPECT_TRUE(added_main.is_valid());
  EXPECT_EQ(0x20u, added_main.offset());
  EXPECT_EQ(DwarfDieRef::Section::kMain, added_main.section());

  DwarfDieRef empty;
  DwarfDieRef added_empty = empty + 0x10;
  EXPECT_FALSE(added_empty.is_valid());

  DwarfDieRef type_ref = DwarfDieRef::Type(0x10);
  DwarfDieRef added_type = type_ref + 0x10;
  EXPECT_TRUE(added_type.is_valid());
  EXPECT_EQ(0x20u, added_type.offset());
  EXPECT_EQ(DwarfDieRef::Section::kType, added_type.section());
}

TEST(DwarfDieRef, Output) {
  std::stringstream ss_empty;
  ss_empty << DwarfDieRef();
  EXPECT_EQ("<invalid>", ss_empty.str());

  std::stringstream ss_main;
  ss_main << DwarfDieRef::Main(0x1234);
  EXPECT_EQ("offset 0x1234", ss_main.str());

  std::stringstream ss_type;
  ss_type << DwarfDieRef::Type(0x1234);
  EXPECT_EQ("offset 0x1234 (.debug_types)", ss_type.str());
}

}  // namespace zxdb
