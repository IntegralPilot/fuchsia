// Copyright 2024 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include <fidl/fuchsia.driver.development/cpp/fidl.h>
#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/async-loop/loop.h>
#include <lib/async/cpp/task.h>
#include <lib/async_patterns/cpp/dispatcher_bound.h>
#include <lib/component/incoming/cpp/protocol.h>
#include <lib/sync/cpp/completion.h>

#include <utility>

#include <gtest/gtest.h>

#include "src/power/testing/system-integration/util/test_util.h"

class ExamplePowerSystemIntegrationTest : public system_integration_utils::TestLoopBase,
                                          public testing::Test {
 public:
  void SetUp() override {
    Initialize();

    auto manager = component::Connect<fuchsia_driver_development::Manager>();
    auto added = fidl::Call(*manager)->AddTestNode({{{
        .name = "power-system-test-example",
        .properties =
            {
                {
                    fuchsia_driver_framework::NodeProperty{
                        fuchsia_driver_framework::NodePropertyKey::WithStringValue(
                            "fuchsia.test.TEST_CHILD"),
                        fuchsia_driver_framework::NodePropertyValue::WithStringValue(
                            "power-system-integration-example-test-driver")},
                },
            },
    }}});
    EXPECT_EQ(true, added.is_ok());
  }

  void TearDown() override {
    auto manager = component::Connect<fuchsia_driver_development::Manager>();
    fidl::Call(*manager)->RemoveTestNode({{
        .name = "power-system-test-example",
    }});
  }
};

TEST_F(ExamplePowerSystemIntegrationTest, MyTest) {
  auto dict_ref = CreateDictionaryForTest();

  std::optional<std::string> found = std::nullopt;
  while (!found) {
    auto node_vec = GetNodeInfo("power-system-test-example");
    for (auto& node : node_vec) {
      if (node.bound_driver_url().has_value() &&
          node.bound_driver_url().value() ==
              "fuchsia-boot:///power-system-integration-example-test-driver#meta/power-system-integration-example-test-driver.cm") {
        found.emplace(node.moniker().value());
        break;
      }
    }
  }

  RunLoopWithTimeout();

  auto manager = component::Connect<fuchsia_driver_development::Manager>();
  auto result = fidl::Call(*manager)->RestartWithDictionary({*found, std::move(dict_ref)});
  EXPECT_EQ(true, result.is_ok());

  found = std::nullopt;
  while (!found) {
    auto node_vec = GetNodeInfo("power-system-test-example");
    for (auto& node : node_vec) {
      if (node.bound_driver_url().has_value() &&
          node.bound_driver_url().value() ==
              "fuchsia-boot:///power-system-integration-example-test-driver#meta/power-system-integration-example-test-driver.cm") {
        found.emplace(node.moniker().value());
        break;
      }
    }
  }

  RunLoopWithTimeout();

  {
    [[maybe_unused]] auto _ = std::move(result.value().release_fence());
  }

  RunLoopWithTimeout();

  found = std::nullopt;
  while (!found) {
    auto node_vec = GetNodeInfo("power-system-test-example");
    for (auto& node : node_vec) {
      if (node.bound_driver_url().has_value() &&
          node.bound_driver_url().value() ==
              "fuchsia-boot:///power-system-integration-example-test-driver#meta/power-system-integration-example-test-driver.cm") {
        found.emplace(node.moniker().value());
        break;
      }
    }
  }

  RunLoopWithTimeout();
}
