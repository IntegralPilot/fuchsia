// Copyright 2021 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "third_party/iwlwifi/platform/driver-inspector.h"

#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/testing/cpp/driver_runtime.h>
#include <lib/fdf/cpp/dispatcher.h>
#include <lib/inspect/cpp/hierarchy.h>
#include <lib/inspect/cpp/reader.h>

#include <cstring>
#include <vector>

#include <gtest/gtest.h>

namespace wlan::testing {
namespace {

class DriverInspectorTest : public ::testing::Test {
 public:
  DriverInspectorTest() : outgoing_(fdf::Dispatcher::GetCurrent()->async_dispatcher()) {}

  fdf_testing::DriverRuntime runtime_;
  component::OutgoingDirectory outgoing_;
};

// Test DriverInspector creation with different parameters.
TEST_F(DriverInspectorTest, CreationOptionsDefault) {
  auto inspector =
      wlan::iwlwifi::DriverInspector(fdf::Dispatcher::GetCurrent()->async_dispatcher(), outgoing_);
  EXPECT_TRUE(inspector.GetRoot());
}

TEST_F(DriverInspectorTest, CreationOptionsSizeAndCapacity) {
  auto inspector = wlan::iwlwifi::DriverInspector(
      fdf::Dispatcher::GetCurrent()->async_dispatcher(), outgoing_,
      wlan::iwlwifi::DriverInspectorOptions{.vmo_size = 4 * 1024, .core_dump_capacity = 4 * 1024});
  EXPECT_TRUE(inspector.GetRoot());
}

TEST_F(DriverInspectorTest, CreationOptionsSize) {
  auto inspector = wlan::iwlwifi::DriverInspector(
      fdf::Dispatcher::GetCurrent()->async_dispatcher(), outgoing_,
      wlan::iwlwifi::DriverInspectorOptions{.vmo_size = 4 * 1024, .core_dump_capacity = 0});
  EXPECT_TRUE(inspector.GetRoot());
}

TEST_F(DriverInspectorTest, CreationOptionsNone) {
  auto inspector = wlan::iwlwifi::DriverInspector(
      fdf::Dispatcher::GetCurrent()->async_dispatcher(), outgoing_,
      wlan::iwlwifi::DriverInspectorOptions{.vmo_size = 0, .core_dump_capacity = 0});
  EXPECT_FALSE(inspector.GetRoot());
}

// Test DriverInspector core dump functionality.
TEST_F(DriverInspectorTest, PublishCoreDump) {
  auto inspector = wlan::iwlwifi::DriverInspector(
      fdf::Dispatcher::GetCurrent()->async_dispatcher(), outgoing_,
      wlan::iwlwifi::DriverInspectorOptions{
          .root_name = "test_inspector", .vmo_size = 8 * 1024, .core_dump_capacity = 2 * 1024});
  ASSERT_TRUE(inspector.GetRoot());

  auto large_buffer = std::vector<char>(2 * 1024 + 1);
  EXPECT_NE(ZX_OK, inspector.PublishCoreDump("too large", large_buffer));

  // Create 5 buffers of 512 bytes each.
  std::vector<std::vector<char>> buffers;
  for (size_t i = 0; i < 5; ++i) {
    buffers.emplace_back(512, i);
  }

  // Insert the first 4 buffers as crash dumps, incrementally verifying that they all appear in the
  // Inspect hierarchy.
  for (size_t i = 0; i < 4; ++i) {
    char buffer_name[16];
    std::snprintf(buffer_name, sizeof(buffer_name), "buffer%zu", i);
    EXPECT_EQ(ZX_OK, inspector.PublishCoreDump(buffer_name, buffers[i]));

    auto root_hierarchy = ::inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();
    EXPECT_EQ(1, root_hierarchy.children().size());
    auto hierarchy = root_hierarchy.GetByPath({"test_inspector"});
    EXPECT_NE(nullptr, hierarchy);

    if (hierarchy != nullptr) {
      auto& node = hierarchy->node();
      EXPECT_EQ(i + 1, node.properties().size());
      for (size_t j = 0; j < node.properties().size(); ++j) {
        char property_name[16];
        std::snprintf(property_name, sizeof(property_name), "buffer%zu", j);
        auto prop = node.get_property<inspect::ByteVectorPropertyValue>(property_name);
        EXPECT_NE(nullptr, prop);
        if (prop != nullptr) {
          EXPECT_EQ(512, prop->value().size());
          EXPECT_EQ(0, std::memcmp(buffers[j].data(), prop->value().data(),
                                   std::min(buffers[j].size(), prop->value().size())));
        }
      }
    }
  }

  // Adding the fifth buffer should cause the oldest crash dump to be replaced.
  EXPECT_EQ(ZX_OK, inspector.PublishCoreDump("buffer4", buffers[4]));

  auto root_hierarchy = ::inspect::ReadFromVmo(inspector.DuplicateVmo()).take_value();
  EXPECT_EQ(1, root_hierarchy.children().size());
  auto hierarchy = root_hierarchy.GetByPath({"test_inspector"});
  EXPECT_NE(nullptr, hierarchy);

  if (hierarchy != nullptr) {
    auto& node = hierarchy->node();
    EXPECT_EQ(4, node.properties().size());
    for (size_t j = 1; j < 5; ++j) {
      char property_name[16];
      std::snprintf(property_name, sizeof(property_name), "buffer%zu", j);
      auto prop = node.get_property<inspect::ByteVectorPropertyValue>(property_name);
      EXPECT_NE(nullptr, prop);
      if (prop != nullptr) {
        EXPECT_EQ(512, prop->value().size());
        EXPECT_EQ(0, std::memcmp(buffers[j].data(), prop->value().data(),
                                 std::min(buffers[j].size(), prop->value().size())));
      }
    }
  }
}

}  // namespace
}  // namespace wlan::testing
