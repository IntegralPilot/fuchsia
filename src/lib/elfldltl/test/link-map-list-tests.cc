// Copyright 2023 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/elfldltl/link-map-list.h>
#include <lib/elfldltl/svr4-abi.h>

#include <type_traits>

#include <gtest/gtest.h>

namespace {

using MapList = elfldltl::LinkMapList<>;

struct Module {
  elfldltl::Elf<>::LinkMap<> link_map;
};

using ModuleList = elfldltl::LinkMapList<Module, elfldltl::LinkMapListInFirstMemberTraits<Module>>;

constexpr const char* kName1 = "1";
constexpr const char* kName2 = "2";
constexpr const char* kName3 = "3";

constexpr MapList::value_type kMaps[3] = {
    {
        .name{kName1},
        .next{const_cast<MapList::value_type*>(&kMaps[1])},
    },
    {
        .name{kName2},
        .next{const_cast<MapList::value_type*>(&kMaps[2])},
        .prev{const_cast<MapList::value_type*>(&kMaps[0])},
    },
    {
        .name{kName3},
        .prev{const_cast<MapList::value_type*>(&kMaps[1])},
    },
};

constexpr ModuleList::value_type kModules[3] = {
    {.link_map =
         {
             .name{kName1},
             .next{&const_cast<Module*>(&kModules[1])->link_map},
         }},
    {.link_map =
         {
             .name{kName2},
             .next{&const_cast<Module*>(&kModules[2])->link_map},
             .prev{&const_cast<Module*>(&kModules[0])->link_map},
         }},
    {.link_map =
         {
             .name{kName3},
             .prev{&const_cast<Module*>(&kModules[1])->link_map},
         }},
};

template <typename T>
struct CheckCtorDtor {
  static_assert(std::is_default_constructible_v<T>);
  static_assert(std::is_copy_constructible_v<T>);
  static_assert(std::is_copy_assignable_v<T>);
  static_assert(std::is_trivially_destructible_v<T>);
};

template <class T>
struct CheckList {
  // Having a nontrivial constructor ensures variables are considered used.
  CheckList() { std::ignore = 0; }

  CheckCtorDtor<T> kCheckList;
  CheckCtorDtor<typename T::iterator> kCheckIterator;
  CheckCtorDtor<typename T::const_iterator> kCheckConstIterator;
  CheckCtorDtor<typename T::reverse_iterator> kCheckReverseIterator;
  CheckCtorDtor<typename T::const_reverse_iterator> kCheckConstReverseIterator;
};

CheckList<MapList> kCheckMapList;
CheckList<ModuleList> kCheckModuleList;

TEST(ElfldltlLinkMapTTests, EmptyList) {
  MapList empty_maplist;
  EXPECT_TRUE(empty_maplist.empty());
  EXPECT_EQ(empty_maplist.begin(), empty_maplist.end());

  ModuleList empty_modulelist;
  EXPECT_TRUE(empty_modulelist.empty());
  EXPECT_EQ(empty_modulelist.begin(), empty_modulelist.end());
}

TEST(ElfldltlLinkMapTTests, IteratorForward) {
  MapList maps{kMaps};
  auto mapsi = maps.begin();
  ASSERT_NE(mapsi, maps.end());
  EXPECT_EQ(mapsi->name.get(), kName1);
  ASSERT_NE(++mapsi, maps.end());
  EXPECT_EQ(mapsi++->name.get(), kName2);
  ASSERT_NE(mapsi, maps.end());
  EXPECT_EQ(mapsi++->name.get(), kName3);
  EXPECT_EQ(mapsi, maps.end());

  ModuleList modules{kModules};
  auto modulesi = modules.begin();
  ASSERT_NE(modulesi, modules.end());
  EXPECT_EQ(modulesi->link_map.name.get(), kName1);
  ASSERT_NE(++modulesi, modules.end());
  EXPECT_EQ(modulesi++->link_map.name.get(), kName2);
  ASSERT_NE(modulesi, modules.end());
  EXPECT_EQ(modulesi++->link_map.name.get(), kName3);
  EXPECT_EQ(modulesi, modules.end());
}

TEST(ElfldltlLinkMapTTests, IteratorBackward) {
  MapList maps{kMaps};
  auto mapsi = maps.end();
  ASSERT_NE(mapsi, maps.begin());
  ASSERT_NE(--mapsi, maps.begin());
  ASSERT_NE(mapsi, maps.end());
  EXPECT_EQ(mapsi->name.get(), kName3);
  ASSERT_NE(--mapsi, maps.begin());
  ASSERT_NE(mapsi, maps.end());
  EXPECT_EQ((*mapsi--).name.get(), kName2);
  ASSERT_NE(mapsi, maps.end());
  EXPECT_EQ(mapsi, maps.begin());
  EXPECT_EQ(mapsi->name.get(), kName1);

  ModuleList modules{kModules};
  auto modulesi = modules.end();
  ASSERT_NE(modulesi, modules.begin());
  ASSERT_NE(--modulesi, modules.begin());
  ASSERT_NE(modulesi, modules.end());
  EXPECT_EQ(modulesi->link_map.name.get(), kName3);
  ASSERT_NE(--modulesi, modules.begin());
  ASSERT_NE(modulesi, modules.end());
  EXPECT_EQ((*modulesi--).link_map.name.get(), kName2);
  ASSERT_NE(modulesi, modules.end());
  EXPECT_EQ(modulesi, modules.begin());
  EXPECT_EQ(modulesi->link_map.name.get(), kName1);
}

TEST(ElfldltlLinkMapTTests, ReverseIteratorForward) {
  MapList maps{kMaps};
  auto mapsi = maps.rbegin();
  ASSERT_NE(mapsi, maps.rend());
  EXPECT_EQ(mapsi->name.get(), kName3);
  ASSERT_NE(++mapsi, maps.rend());
  EXPECT_EQ(mapsi++->name.get(), kName2);
  ASSERT_NE(mapsi, maps.rend());
  EXPECT_EQ(mapsi++->name.get(), kName1);
  EXPECT_EQ(mapsi, maps.rend());

  ModuleList modules{kModules};
  auto modulesi = modules.rbegin();
  ASSERT_NE(modulesi, modules.rend());
  EXPECT_EQ(modulesi->link_map.name.get(), kName3);
  ASSERT_NE(++modulesi, modules.rend());
  EXPECT_EQ(modulesi++->link_map.name.get(), kName2);
  ASSERT_NE(modulesi, modules.rend());
  EXPECT_EQ(modulesi++->link_map.name.get(), kName1);
  EXPECT_EQ(modulesi, modules.rend());
}

TEST(ElfldltlLinkMapTTests, ReverseIteratorBackward) {
  MapList maps{kMaps};
  auto mapsi = maps.rend();
  ASSERT_NE(mapsi, maps.rbegin());
  ASSERT_NE(--mapsi, maps.rbegin());
  ASSERT_NE(mapsi, maps.rend());
  EXPECT_EQ(mapsi->name.get(), kName1);
  ASSERT_NE(--mapsi, maps.rbegin());
  ASSERT_NE(mapsi, maps.rend());
  EXPECT_EQ((*mapsi--).name.get(), kName2);
  ASSERT_NE(mapsi, maps.rend());
  EXPECT_EQ(mapsi, maps.rbegin());
  EXPECT_EQ(mapsi->name.get(), kName3);

  ModuleList modules{kModules};
  auto modulesi = modules.rend();
  ASSERT_NE(modulesi, modules.rbegin());
  ASSERT_NE(--modulesi, modules.rbegin());
  ASSERT_NE(modulesi, modules.rend());
  EXPECT_EQ(modulesi->link_map.name.get(), kName1);
  ASSERT_NE(--modulesi, modules.rbegin());
  ASSERT_NE(modulesi, modules.rend());
  EXPECT_EQ((*modulesi--).link_map.name.get(), kName2);
  ASSERT_NE(modulesi, modules.rend());
  EXPECT_EQ(modulesi, modules.rbegin());
  EXPECT_EQ(modulesi->link_map.name.get(), kName3);
}

}  // namespace
