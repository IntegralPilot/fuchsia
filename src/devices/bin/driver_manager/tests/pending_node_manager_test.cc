// Copyright 2026 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

#include "src/devices/bin/driver_manager/pending_node_manager.h"

#include <fidl/fuchsia.driver.framework/cpp/fidl.h>
#include <lib/async-loop/cpp/loop.h>
#include <lib/driver/component/cpp/node_add_args.h>

#include <gtest/gtest.h>

#include "src/devices/bin/driver_manager/node.h"
#include "src/devices/bin/driver_manager/resource.h"
#include "src/devices/bin/driver_manager/testing/fake_driver_index.h"
#include "src/devices/bin/driver_manager/tests/driver_manager_test_base.h"

namespace fdfw = fuchsia_driver_framework;
using namespace driver_manager;

fdfw::NodeProperty2 MakeProperty(std::string key, std::string value) {
  return fdfw::NodeProperty2(key, fdfw::NodePropertyValue::WithStringValue(value));
}

fdfw::ResourceProperty MakeResourceProperty(std::string key, std::string value) {
  return fdfw::ResourceProperty(key, fdfw::ResourcePropertyValue::WithStringValue(value));
}

class FakeNodeManager : public TestNodeManagerBase {
 public:
  FakeNodeManager(fidl::ClientEnd<fuchsia_driver_index::DriverIndex> driver_index)
      : driver_index_(std::move(driver_index), async_get_default_dispatcher()) {}

  void RequestMatchPendingNode(
      fidl::VectorView<fuchsia_driver_framework::wire::ParentSpec2> dependencies,
      fit::callback<
          void(fidl::WireUnownedResult<fuchsia_driver_index::DriverIndex::MatchPendingNode>&)>
          match_callback) override {
    driver_index_->MatchPendingNode(dependencies).Then(std::move(match_callback));
  }

  zx::result<> StartDriver(Node& node, std::string_view url,
                           fuchsia_driver_framework::DriverPackageType package_type) override {
    start_drivers_.push_back({
        .name = node.name(),
        .url = std::string(url),
    });
    return zx::ok();
  }

  struct StartDriverCall {
    std::string name;
    std::string url;
  };

  fidl::WireClient<fuchsia_driver_index::DriverIndex> driver_index_;
  std::vector<StartDriverCall> start_drivers_;
};

class PendingNodeManagerTest : public DriverManagerTestBase {
 public:
  PendingNodeManagerTest() {
    fake_driver_index_.emplace(
        dispatcher(),
        [](fuchsia_driver_index::wire::MatchDriverArgs args) {
          return zx::error(ZX_ERR_NOT_FOUND);
        },
        [this](fidl::AnyArena& arena,
               fidl::VectorView<fuchsia_driver_framework::wire::ParentSpec2> dependencies)
            -> zx::result<fdfw::wire::CompositeDriverMatch> {
          if (match_pending_node_result_.has_value()) {
            return zx::ok(fidl::ToWire(arena, match_pending_node_result_.value()));
          }
          return zx::error(ZX_ERR_NOT_FOUND);
        });

    node_manager_ = std::make_unique<FakeNodeManager>(fake_driver_index_->Connect());
  }

  NodeManager* GetNodeManager() override { return node_manager_.get(); }

  void SetUp() override {
    DriverManagerTestBase::SetUp();
    pending_node_manager_.emplace(GetNodeManager(), dispatcher());
  }

  void TearDown() override {
    pending_node_manager_.reset();
    DriverManagerTestBase::TearDown();
    node_manager_.reset();
    fake_driver_index_.reset();
  }

 protected:
  std::optional<FakeDriverIndex> fake_driver_index_;
  std::unique_ptr<FakeNodeManager> node_manager_;
  std::optional<PendingNodeManager> pending_node_manager_;

  std::optional<fdfw::CompositeDriverMatch> match_pending_node_result_;
};

TEST_F(PendingNodeManagerTest, AddNodeAndResolveSuccessfully) {
  // 1. Setup mock match pending node result.
  fdfw::DriverInfo driver_info;
  driver_info.url("fuchsia-boot:///#meta/test_composite.cm");

  fdfw::CompositeDriverInfo composite_driver;
  composite_driver.composite_name("test_composite");
  composite_driver.driver_info(std::move(driver_info));

  fdfw::CompositeDriverMatch match;
  match.composite_driver(std::move(composite_driver));
  match.parent_names({{"parent_1"}});
  match_pending_node_result_ = std::move(match);

  // 2. Add pending node.
  fdfw::Selector selector;
  std::vector<fdfw::ResourceProperty> include_props;
  include_props.push_back(MakeResourceProperty("key_1", "val_1"));
  selector.include_properties(std::move(include_props));

  fdfw::Dependency dep;
  dep.selector(std::move(selector));

  fdfw::Node2 node;
  node.name("composite_node");
  std::vector<fdfw::Dependency> deps;
  deps.push_back(std::move(dep));
  node.dependencies(std::move(deps));

  auto controller = fidl::Endpoints<fdfw::NodeController>::Create();

  auto add_result = pending_node_manager_->AddNode(std::move(node), std::move(controller.server),
                                                   fidl::ServerEnd<fdfw::Node>{});
  ASSERT_TRUE(add_result.is_ok());

  // Wait for the async driver index match response.
  RunLoopUntilIdle();

  // Verify node is matched and pending.
  ASSERT_EQ(pending_node_manager_->pending_nodes().size(), 1u);
  auto& pending = pending_node_manager_->pending_nodes()[0];
  ASSERT_TRUE(pending->matched_driver.has_value());
  ASSERT_EQ(pending->matched_driver->composite_driver()->driver_info()->url().value(),
            "fuchsia-boot:///#meta/test_composite.cm");

  // 3. Resolve the dependencies.
  // Create resource satisfying dependency properties.
  auto owner = CreateNode("owner");
  std::vector<fdfw::NodeProperty2> properties = {
      MakeProperty("key_1", "val_1"),
  };
  auto resource = std::make_shared<Resource>(GetNodeManager()->GetNextResourceId(), owner,
                                             "my_resource", std::move(properties),
                                             std::vector<NodeOffer>{}, std::nullopt, dispatcher());

  // Trigger resolution.
  std::unordered_map<ResourceId, std::weak_ptr<Resource>> multibind_resources;
  multibind_resources[resource->id()] = resource;
  pending_node_manager_->TryResolvePendingNodes(multibind_resources);

  // Run loop to instantiate node.
  RunLoopUntilIdle();

  // Node should be resolved and removed from pending.
  EXPECT_TRUE(pending_node_manager_->pending_nodes().empty());

  // Verify that the driver has been started.
  ASSERT_EQ(node_manager_->start_drivers_.size(), 1u);
  EXPECT_EQ(node_manager_->start_drivers_[0].name, "composite_node");
  EXPECT_EQ(node_manager_->start_drivers_[0].url, "fuchsia-boot:///#meta/test_composite.cm");
}

TEST_F(PendingNodeManagerTest, MatchPendingNodesWithoutDriver) {
  // 1. Add pending node. No match returned yet.
  fdfw::Selector selector;
  std::vector<fdfw::ResourceProperty> include_props;
  include_props.push_back(MakeResourceProperty("key_1", "val_1"));
  selector.include_properties(std::move(include_props));

  fdfw::Dependency dep;
  dep.selector(std::move(selector));

  fdfw::Node2 node;
  node.name("composite_node");
  std::vector<fdfw::Dependency> deps;
  deps.push_back(std::move(dep));
  node.dependencies(std::move(deps));

  auto controller = fidl::Endpoints<fdfw::NodeController>::Create();
  auto node_ref = fidl::Endpoints<fdfw::Node>::Create();

  auto add_result = pending_node_manager_->AddNode(std::move(node), std::move(controller.server),
                                                   std::move(node_ref.server));
  ASSERT_TRUE(add_result.is_ok());

  RunLoopUntilIdle();

  // Verify node is pending but not matched to a driver.
  ASSERT_EQ(pending_node_manager_->pending_nodes().size(), 1u);
  auto& pending = pending_node_manager_->pending_nodes()[0];
  ASSERT_FALSE(pending->matched_driver.has_value());

  // 2. Setup mock match pending node result.
  fdfw::DriverInfo driver_info;
  driver_info.url("fuchsia-boot:///#meta/test_composite.cm");

  fdfw::CompositeDriverInfo composite_driver;
  composite_driver.composite_name("test_composite");
  composite_driver.driver_info(std::move(driver_info));

  fdfw::CompositeDriverMatch match;
  match.composite_driver(std::move(composite_driver));
  match.parent_names({{"parent_1"}});
  match_pending_node_result_ = std::move(match);

  // 3. Retry matching.
  pending_node_manager_->MatchPendingNodesWithoutDriver();

  RunLoopUntilIdle();

  // Verify node is now matched.
  ASSERT_EQ(pending_node_manager_->pending_nodes().size(), 1u);
  ASSERT_TRUE(pending->matched_driver.has_value());
  EXPECT_EQ(pending->matched_driver->composite_driver()->driver_info()->url().value(),
            "fuchsia-boot:///#meta/test_composite.cm");
}

TEST_F(PendingNodeManagerTest, AddNodeAndResolveFailAssembly) {
  // 1. Setup mock match pending node result.
  fdfw::DriverInfo driver_info;
  driver_info.url("fuchsia-boot:///#meta/test_composite.cm");

  fdfw::CompositeDriverInfo composite_driver;
  composite_driver.composite_name("test_composite");
  composite_driver.driver_info(std::move(driver_info));

  fdfw::CompositeDriverMatch match;
  match.composite_driver(std::move(composite_driver));
  match.parent_names({{"parent_1"}});
  // Set invalid primary parent index (should be < dependencies.size() which will be 1)
  match.primary_parent_index(1);
  match_pending_node_result_ = std::move(match);

  // 2. Add pending node.
  fdfw::Selector selector;
  std::vector<fdfw::ResourceProperty> include_props;
  include_props.push_back(MakeResourceProperty("key_1", "val_1"));
  selector.include_properties(std::move(include_props));

  fdfw::Dependency dep;
  dep.selector(std::move(selector));

  fdfw::Node2 node;
  node.name("composite_node");
  std::vector<fdfw::Dependency> deps;
  deps.push_back(std::move(dep));
  node.dependencies(std::move(deps));

  auto controller = fidl::Endpoints<fdfw::NodeController>::Create();

  auto add_result = pending_node_manager_->AddNode(std::move(node), std::move(controller.server),
                                                   fidl::ServerEnd<fdfw::Node>{});
  ASSERT_TRUE(add_result.is_ok());

  // Wait for the async driver index match response.
  RunLoopUntilIdle();

  // Verify node is matched and pending.
  ASSERT_EQ(pending_node_manager_->pending_nodes().size(), 1u);
  auto& pending = pending_node_manager_->pending_nodes()[0];
  ASSERT_TRUE(pending->matched_driver.has_value());
  ASSERT_EQ(pending->matched_driver->composite_driver()->driver_info()->url().value(),
            "fuchsia-boot:///#meta/test_composite.cm");

  // 3. Resolve the dependencies.
  // Create resource satisfying dependency properties.
  auto owner = CreateNode("owner");
  std::vector<fdfw::NodeProperty2> properties = {
      MakeProperty("key_1", "val_1"),
  };
  auto resource = std::make_shared<Resource>(GetNodeManager()->GetNextResourceId(), owner,
                                             "my_resource", std::move(properties),
                                             std::vector<NodeOffer>{}, std::nullopt, dispatcher());

  // Trigger resolution.
  std::unordered_map<ResourceId, std::weak_ptr<Resource>> multibind_resources;
  multibind_resources[resource->id()] = resource;
  pending_node_manager_->TryResolvePendingNodes(multibind_resources);

  // Run loop to instantiate node.
  RunLoopUntilIdle();

  // Node resolution should fail to assemble (because of invalid primary_parent_index),
  // and it should be removed from pending nodes.
  EXPECT_TRUE(pending_node_manager_->pending_nodes().empty());

  // Verify that the driver has NOT been started.
  EXPECT_TRUE(node_manager_->start_drivers_.empty());
}

TEST_F(PendingNodeManagerTest, AddNodeAndResolveNoDriver) {
  // 1. Setup mock match pending node result to not match any driver (by not setting
  // match_pending_node_result_).
  match_pending_node_result_ = std::nullopt;

  // 2. Add pending node.
  fdfw::Selector selector;
  std::vector<fdfw::ResourceProperty> include_props;
  include_props.push_back(MakeResourceProperty("key_1", "val_1"));
  selector.include_properties(std::move(include_props));

  fdfw::Dependency dep;
  dep.selector(std::move(selector));

  fdfw::Node2 node;
  node.name("composite_node");
  std::vector<fdfw::Dependency> deps;
  deps.push_back(std::move(dep));
  node.dependencies(std::move(deps));

  auto controller = fidl::Endpoints<fdfw::NodeController>::Create();

  auto add_result = pending_node_manager_->AddNode(std::move(node), std::move(controller.server),
                                                   fidl::ServerEnd<fdfw::Node>{});
  ASSERT_TRUE(add_result.is_ok());

  // Wait for the async driver index match response.
  RunLoopUntilIdle();

  // Verify node is pending and not matched to a driver.
  ASSERT_EQ(pending_node_manager_->pending_nodes().size(), 1u);
  auto& pending = pending_node_manager_->pending_nodes()[0];
  ASSERT_FALSE(pending->matched_driver.has_value());

  // 3. Resolve the dependencies.
  // Create resource satisfying dependency properties.
  auto owner = CreateNode("owner");
  std::vector<fdfw::NodeProperty2> properties = {
      MakeProperty("key_1", "val_1"),
  };
  auto resource = std::make_shared<Resource>(GetNodeManager()->GetNextResourceId(), owner,
                                             "my_resource", std::move(properties),
                                             std::vector<NodeOffer>{}, std::nullopt, dispatcher());

  // Trigger resolution.
  std::unordered_map<ResourceId, std::weak_ptr<Resource>> multibind_resources;
  multibind_resources[resource->id()] = resource;
  pending_node_manager_->TryResolvePendingNodes(multibind_resources);

  // Run loop to instantiate node.
  RunLoopUntilIdle();

  // Node resolution should fail to assemble (because of no matched driver),
  // but it should NOT be removed from pending nodes.
  EXPECT_EQ(pending_node_manager_->pending_nodes().size(), 1u);

  // Verify that the driver has NOT been started.
  EXPECT_TRUE(node_manager_->start_drivers_.empty());
}
