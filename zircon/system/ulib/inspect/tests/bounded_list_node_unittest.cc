// Copyright 2013 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <lib/inspect/cpp/bounded_list_node.h>
#include <lib/inspect/cpp/inspector.h>
#include <lib/inspect/cpp/reader.h>

#include <zxtest/zxtest.h>

#include "lib/inspect/cpp/hierarchy.h"

namespace {
using inspect::BoundedListNode;
using inspect::Inspector;

TEST(BoundedListNodeTest, ConstructAList) {
  Inspector inspector;

  auto list = BoundedListNode(inspector.GetRoot().CreateChild("list"), 5);
  EXPECT_EQ(5, list.capacity());
}

TEST(BoundedListNodeTest, CreateEntries) {
  Inspector inspector;

  auto list = BoundedListNode(inspector.GetRoot().CreateChild("list"), 5);

  list.CreateEntry([](auto& n) { n.RecordInt("MyInt", 37); });

  {
    auto result = inspect::ReadFromVmo(inspector.DuplicateVmo());
    ASSERT_TRUE(result.is_ok());

    auto hierarchy = result.take_value();

    ASSERT_EQ(1u, hierarchy.children().size());
    ASSERT_EQ("list", hierarchy.children()[0].name());

    const auto& list_as_root = hierarchy.children()[0];
    ASSERT_EQ(1u, list_as_root.children().size());
    ASSERT_EQ("0", list_as_root.children()[0].name());
    ASSERT_EQ(37u, list_as_root.children()[0]
                       .node()
                       .get_property<inspect::IntPropertyValue>("MyInt")
                       ->value());
  }

  // fill the buffer the rest of the way
  list.CreateEntry([](auto& n) { n.RecordInt("Next", 5); });
  list.CreateEntry([](auto& n) { n.RecordInt("Next1", 6); });
  list.CreateEntry([](auto& n) { n.RecordInt("Next2", 7); });
  list.CreateEntry([](auto& n) { n.RecordInt("Next3", 8); });

  auto result = inspect::ReadFromVmo(inspector.DuplicateVmo());
  ASSERT_TRUE(result.is_ok());

  auto hierarchy = result.take_value();
  hierarchy.Sort();

  ASSERT_EQ(1u, hierarchy.children().size());
  ASSERT_EQ("list", hierarchy.children()[0].name());

  const auto& list_as_root = hierarchy.children()[0];
  ASSERT_EQ(5u, list_as_root.children().size());
  ASSERT_EQ("0", list_as_root.children()[0].name());
  ASSERT_EQ(
      37u,
      list_as_root.children()[0].node().get_property<inspect::IntPropertyValue>("MyInt")->value());

  ASSERT_EQ("1", list_as_root.children()[1].name());
  ASSERT_EQ(
      5u,
      list_as_root.children()[1].node().get_property<inspect::IntPropertyValue>("Next")->value());

  ASSERT_EQ("2", list_as_root.children()[2].name());
  ASSERT_EQ(
      6u,
      list_as_root.children()[2].node().get_property<inspect::IntPropertyValue>("Next1")->value());

  ASSERT_EQ("3", list_as_root.children()[3].name());
  ASSERT_EQ(
      7u,
      list_as_root.children()[3].node().get_property<inspect::IntPropertyValue>("Next2")->value());

  ASSERT_EQ("4", list_as_root.children()[4].name());
  ASSERT_EQ(
      8u,
      list_as_root.children()[4].node().get_property<inspect::IntPropertyValue>("Next3")->value());
}

TEST(BoundedListNodeTest, PushValuesOut) {
  Inspector inspector;

  auto list = BoundedListNode(inspector.GetRoot().CreateChild("list"), 5);
  list.CreateEntry([](auto& n) { n.RecordInt("Next0", 0); });
  list.CreateEntry([](auto& n) { n.RecordInt("Next1", 1); });
  list.CreateEntry([](auto& n) { n.RecordInt("Next2", 2); });
  list.CreateEntry([](auto& n) { n.RecordInt("Next3", 3); });
  list.CreateEntry([](auto& n) { n.RecordInt("Next4", 4); });
  list.CreateEntry([](auto& n) { n.RecordInt("Next5", 5); });

  auto result = inspect::ReadFromVmo(inspector.DuplicateVmo());
  ASSERT_TRUE(result.is_ok());

  auto hierarchy = result.take_value();
  hierarchy.Sort();

  // check that there is one node holding the list
  ASSERT_EQ(1u, hierarchy.children().size());
  ASSERT_EQ("list", hierarchy.children()[0].name());

  const auto& list_as_root = hierarchy.children()[0];
  // check that the list has five (capacity) children)
  ASSERT_EQ(5u, list_as_root.children().size());

  ASSERT_EQ("1", list_as_root.children()[0].name());
  ASSERT_EQ(
      1u,
      list_as_root.children()[0].node().get_property<inspect::IntPropertyValue>("Next1")->value());

  ASSERT_EQ("2", list_as_root.children()[1].name());
  ASSERT_EQ(
      2u,
      list_as_root.children()[1].node().get_property<inspect::IntPropertyValue>("Next2")->value());

  ASSERT_EQ("3", list_as_root.children()[2].name());
  ASSERT_EQ(
      3u,
      list_as_root.children()[2].node().get_property<inspect::IntPropertyValue>("Next3")->value());

  ASSERT_EQ("4", list_as_root.children()[3].name());
  ASSERT_EQ(
      4u,
      list_as_root.children()[3].node().get_property<inspect::IntPropertyValue>("Next4")->value());

  ASSERT_EQ("5", list_as_root.children()[4].name());
  ASSERT_EQ(
      5u,
      list_as_root.children()[4].node().get_property<inspect::IntPropertyValue>("Next5")->value());
}

TEST(BoundedListNodeTest, CreateEntriesThenMove) {
  Inspector inspector;

  auto list = BoundedListNode(inspector.GetRoot().CreateChild("list"), 2);

  list.CreateEntry([](auto& n) { n.RecordInt("MyInt", 37); });

  {
    auto result = inspect::ReadFromVmo(inspector.DuplicateVmo());
    ASSERT_TRUE(result.is_ok());

    auto hierarchy = result.take_value();
    hierarchy.Sort();

    ASSERT_EQ(1u, hierarchy.children().size());
    ASSERT_EQ("list", hierarchy.children()[0].name());

    const auto& list_as_root = hierarchy.children()[0];
    ASSERT_EQ(1u, list_as_root.children().size());
    ASSERT_EQ("0", list_as_root.children()[0].name());
    ASSERT_EQ(37u, list_as_root.children()[0]
                       .node()
                       .get_property<inspect::IntPropertyValue>("MyInt")
                       ->value());
  }

  auto new_list = std::move(list);
  new_list.CreateEntry([](auto& n) { n.RecordInt("MyIntOther", 5); });

  {
    auto result = inspect::ReadFromVmo(inspector.DuplicateVmo());
    ASSERT_TRUE(result.is_ok());

    auto hierarchy = result.take_value();
    hierarchy.Sort();

    ASSERT_EQ(1u, hierarchy.children().size());
    ASSERT_EQ("list", hierarchy.children()[0].name());

    const auto& list_as_root = hierarchy.children()[0];
    ASSERT_EQ(2u, list_as_root.children().size());
    ASSERT_EQ("0", list_as_root.children().at(0).name());
    ASSERT_EQ(37u, list_as_root.children()
                       .at(0)
                       .node()
                       .get_property<inspect::IntPropertyValue>("MyInt")
                       ->value());

    ASSERT_EQ(2u, list_as_root.children().size());
    ASSERT_EQ("1", list_as_root.children().at(1).name());
    ASSERT_EQ(5u, list_as_root.children()
                      .at(1)
                      .node()
                      .get_property<inspect::IntPropertyValue>("MyIntOther")
                      ->value());
  }
}
}  // namespace
